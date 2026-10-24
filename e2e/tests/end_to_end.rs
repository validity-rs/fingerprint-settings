//! The application core against a real fingerprint service on a private bus:
//! validity-rs's daemon with its emulated sensor, driven through the same
//! D-Bus driver and state machine as the GTK interface, from start to
//! enrolment, listing, test and removal, and through the error paths.
//!
//! Needs `dbus-daemon` on the PATH. A missing binary fails the test: it is a
//! requirement, not something to skip.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fingerprint_settings::error::ErrorKind;
use fingerprint_settings::fingers::Finger;
use fingerprint_settings::fprint::{Bus, Deadlines, Service};
use fingerprint_settings::i18n::Catalogue;
use fingerprint_settings::labels::{Labels, Store};
use fingerprint_settings::machine::{
    Command, Dialog, EnrollOutcome, Input, Model, Page, Step, TestOutcome,
};
use fingerprint_settings::status::ScanType;
use validity_daemon::fake::{FakeSensor, Touch, blobs};
use validity_daemon::polkit::{Authority, Check, ENROLL, VERIFY};
use validity_daemon::service::{self, Deps, Options, Setup};
use validity_daemon::users::{SidMap, User, UserDb};
use zbus::Connection;

// ---------------------------------------------------------------- the bus

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!(
            "fingerprint-settings-test-{}-{tag}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A private `dbus-daemon`, killed on drop.
struct PrivateBus {
    child: Child,
    address: String,
    _dir: TempDir,
}

impl PrivateBus {
    fn start() -> Self {
        let dir = TempDir::new("bus");
        let config = dir.0.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>fingerprint-settings-test</type>
  <listen>unix:path={}/socket</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                dir.0.display()
            ),
        )
        .unwrap();
        let mut child = Process::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .arg("--print-address=1")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("dbus-daemon must be installed to run these tests");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let address = line.trim().to_string();
        assert!(address.starts_with("unix:"), "dbus-daemon said {line:?}");
        PrivateBus {
            child,
            address,
            _dir: dir,
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ------------------------------------------------- polkit and accounts

/// Allows everything but the actions denied.
#[derive(Default)]
struct TestAuthority {
    denied: Mutex<BTreeSet<&'static str>>,
}

impl TestAuthority {
    fn deny(&self, action: &'static str) {
        self.denied.lock().unwrap().insert(action);
    }
    fn allow_all(&self) {
        self.denied.lock().unwrap().clear();
    }
}

impl Authority for TestAuthority {
    fn check<'a>(&'a self, _: &'a Connection, _: &'a str, action: &'a str) -> Check<'a> {
        Box::pin(async move { Ok(!self.denied.lock().unwrap().contains(action)) })
    }
}

/// The test process runs as "alice".
struct TestUsers {
    me: u32,
}

impl UserDb for TestUsers {
    fn by_uid(&self, uid: u32) -> Option<User> {
        (uid == self.me).then(|| User {
            name: "alice".into(),
            uid,
        })
    }
    fn by_name(&self, name: &str) -> Option<User> {
        (name == "alice").then(|| User {
            name: name.into(),
            uid: self.me,
        })
    }
}

// ---------------------------------------------------------- the daemon

/// Short sensor delays, so that tests run fast; everything else is the
/// daemon's default.
fn fast_options() -> Options {
    let mut o = Options::default();
    o.timing.attempts = [0, 20, 40, 80, 160]
        .into_iter()
        .map(Duration::from_millis)
        .collect();
    o.timing.absent_first = Duration::from_millis(20);
    o.timing.absent_max = Duration::from_millis(200);
    o.timing.failed_cooldown = Duration::from_secs(1);
    o.timing.reappear = Duration::from_secs(1);
    o.timing.reappear_poll = Duration::from_millis(20);
    o
}

struct Daemon {
    stop: zruntime::mpmc::Sender<()>,
    thread: Option<std::thread::JoinHandle<Result<(), String>>>,
    _state: TempDir,
}

impl Daemon {
    fn start(address: &str, fake: &FakeSensor, authority: Arc<TestAuthority>) -> Self {
        let state = TempDir::new("state");
        let (stop, stopped) = zruntime::mpmc::bounded::<()>(std::num::NonZeroUsize::MIN);
        let hw = fake.hardware();
        let bus = service::Bus::Address(address.to_string());
        let setup = Setup {
            blobs: Ok(blobs()),
            calibration: state.0.join("calibration.bin"),
        };
        let deps = Deps {
            authority,
            users: Arc::new(TestUsers {
                me: nix::unistd::getuid().as_raw(),
            }),
            sids: SidMap::default(),
        };
        let thread = std::thread::spawn(move || {
            zbus::block_on(service::run(
                bus,
                hw,
                setup,
                deps,
                fast_options(),
                async move {
                    let _ = stopped.recv().await;
                },
            ))
        });
        Daemon {
            stop,
            thread: Some(thread),
            _state: state,
        }
    }

    fn stop(mut self) -> Result<(), String> {
        let _ = self.stop.try_send(());
        self.thread.take().unwrap().join().unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A raw client of the bus.
fn client(address: &str) -> Connection {
    zbus::block_on(async {
        zbus::connection::Builder::address(address)
            .build()
            .await
            .unwrap()
    })
}

fn call(
    c: &Connection,
    path: &str,
    iface: &str,
    method: &str,
    arg: Option<&str>,
) -> zbus::Result<()> {
    zbus::block_on(async {
        match arg {
            Some(a) => c
                .call_method(
                    Some("net.reactivated.Fprint"),
                    path,
                    Some(iface),
                    method,
                    &(a,),
                )
                .await
                .map(|_| ()),
            None => c
                .call_method(
                    Some("net.reactivated.Fprint"),
                    path,
                    Some(iface),
                    method,
                    &(),
                )
                .await
                .map(|_| ()),
        }
    })
}

const MANAGER: (&str, &str) = (
    "/net/reactivated/Fprint/Manager",
    "net.reactivated.Fprint.Manager",
);
const DEVICE: (&str, &str) = (
    "/net/reactivated/Fprint/Device/0",
    "net.reactivated.Fprint.Device",
);

/// Waits until the daemon has its sensor up: the default device exists and
/// answers a list.
fn wait_ready(address: &str) {
    let c = client(address);
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let dev = call(&c, MANAGER.0, MANAGER.1, "GetDefaultDevice", None);
        let list = call(&c, DEVICE.0, DEVICE.1, "ListEnrolledFingers", Some(""));
        let listed = match list {
            Ok(()) => true,
            Err(zbus::Error::MethodError(n, _, _)) => {
                n.as_str() == "net.reactivated.Fprint.Error.NoEnrolledPrints"
            }
            Err(_) => false,
        };
        if dev.is_ok() && listed {
            return;
        }
        assert!(
            Instant::now() < end,
            "the daemon did not get ready: {dev:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Everything a test needs: bus, emulated sensor, daemon.
struct Env {
    bus: PrivateBus,
    fake: FakeSensor,
    authority: Arc<TestAuthority>,
    daemon: Option<Daemon>,
}

impl Env {
    fn new() -> Self {
        let bus = PrivateBus::start();
        let fake = FakeSensor::new();
        let authority = Arc::new(TestAuthority::default());
        let daemon = Daemon::start(&bus.address, &fake, authority.clone());
        wait_ready(&bus.address);
        Env {
            bus,
            fake,
            authority,
            daemon: Some(daemon),
        }
    }

    fn app(&self) -> App {
        App::new(&self.bus.address)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Some(d) = self.daemon.take() {
            let r = d.stop();
            if !std::thread::panicking() {
                r.expect("the daemon stopped cleanly");
            }
        }
    }
}

// ---------------------------------------------------------- the app

/// The application without its window: the model, and the D-Bus driver
/// executing its commands, as the GTK layer wires them.
struct App {
    m: Model,
    service: Service,
    /// Every command sent, in order.
    sent: Vec<Command>,
    /// Every enrolment instruction shown, in English.
    instructions: Vec<String>,
    /// Where the names are saved, as the window does.
    store: Option<Store>,
}

const TIMEOUT: Duration = Duration::from_secs(30);

impl App {
    fn new(address: &str) -> Self {
        Self::with(address, Deadlines::default())
    }

    fn with(address: &str, deadlines: Deadlines) -> Self {
        App {
            m: Model::new(),
            service: Service::start_with(Bus::Address(address.to_string()), deadlines).unwrap(),
            sent: Vec::new(),
            instructions: Vec::new(),
            store: None,
        }
    }

    /// With the names kept in `store`, loaded first, as the window does.
    fn with_store(address: &str, mut store: Store) -> Self {
        let loaded = store.load();
        let mut app = Self::new(address);
        app.m = Model::with_labels(loaded.labels);
        app.store = Some(store);
        app
    }

    fn input(&mut self, i: Input) {
        for c in self.m.update(i) {
            self.sent.push(c.clone());
            self.service.send(c);
        }
        if let Some(labels) = self.m.take_labels_to_save()
            && let Some(store) = &mut self.store
        {
            store.save(&labels).unwrap();
        }
        let shown = match &self.m.page {
            Page::Enroll(e) => Some(&e.instruction),
            Page::Test(t) => Some(&t.instruction),
            _ => None,
        };
        if let Some(i) = shown {
            let text = i.render(&Catalogue::default());
            if self.instructions.last() != Some(&text) {
                self.instructions.push(text);
            }
        }
    }

    /// Feeds the service's inputs to the model until `done` holds.
    fn until(&mut self, what: &str, done: impl Fn(&Model) -> bool) {
        let end = Instant::now() + TIMEOUT;
        while !done(&self.m) {
            match self.service.inputs().try_recv() {
                Ok(i) => self.input(i),
                Err(e) if e.is_empty() => {
                    assert!(
                        Instant::now() < end,
                        "timed out waiting for {what}; page {:?}, sent {:?}",
                        self.m.page,
                        self.sent
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("the driver is gone: {e:?}"),
            }
        }
    }

    /// Feeds inputs until nothing is in flight and the page is not loading.
    fn settle(&mut self) {
        self.until("the app to settle", |m| !m.busy());
        // And whatever refresh was queued last.
        let end = Instant::now() + Duration::from_millis(300);
        while Instant::now() < end {
            match self.service.inputs().try_recv() {
                Ok(i) => self.input(i),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    fn start(&mut self) {
        self.input(Input::Start);
        self.settle();
    }

    fn enroll(&self) -> &fingerprint_settings::machine::Enroll {
        match &self.m.page {
            Page::Enroll(e) => e,
            p => panic!("not enrolling: {p:?}"),
        }
    }

    fn test(&self) -> &fingerprint_settings::machine::Test {
        match &self.m.page {
            Page::Test(t) => t,
            p => panic!("not testing: {p:?}"),
        }
    }

    /// Adds `finger` with its standard name, the emulated sensor seeing
    /// `touches`.
    fn add(&mut self, fake: &FakeSensor, finger: Finger, touches: Vec<Touch>) {
        self.add_named(fake, Some(finger), None, touches);
    }

    /// Adds a fingerprint as the add page does: a finger or none, a name or
    /// none.
    fn add_named(
        &mut self,
        fake: &FakeSensor,
        finger: Option<Finger>,
        label: Option<&str>,
        touches: Vec<Touch>,
    ) {
        self.input(Input::AddFinger);
        assert_eq!(self.m.page, Page::Add);
        fake.touch(touches);
        self.input(Input::StartEnroll {
            finger,
            label: label.map(String::from),
        });
        self.until(
            "the enrolment to end",
            |m| matches!(&m.page, Page::Enroll(e) if e.step == Step::Done),
        );
        self.settle();
    }

    fn test_finger(&mut self, fake: &FakeSensor, touches: Vec<Touch>) -> TestOutcome {
        fake.touch(touches);
        self.input(Input::TestFinger);
        self.until(
            "the test to end",
            |m| matches!(&m.page, Page::Test(t) if t.step == Step::Done),
        );
        self.settle();
        self.test().outcome.clone().unwrap()
    }

    fn back(&mut self) {
        self.input(Input::Cancel);
        assert_eq!(self.m.page, Page::Fingers);
    }

    /// Whether the device is free: a second client can claim and release it.
    fn device_is_free(address: &str) -> bool {
        let c = client(address);
        let ok = call(&c, DEVICE.0, DEVICE.1, "Claim", Some("")).is_ok();
        if ok {
            call(&c, DEVICE.0, DEVICE.1, "Release", None).unwrap();
        }
        ok
    }
}

fn five_good(f: u32) -> Vec<Touch> {
    vec![Touch::Finger(f); 6]
}

// ------------------------------------------------------------ the tests

#[test]
fn enrol_list_test_remove() {
    let env = Env::new();
    let mut app = env.app();
    app.start();
    assert_eq!(app.m.page, Page::Fingers);
    assert!(app.m.fingers.is_empty());
    let dev = app.m.device.clone().unwrap();
    assert_eq!(dev.scan, ScanType::Press);
    assert_eq!(
        dev.name,
        "Synaptics/Validity fingerprint sensor (06cb:009a)"
    );

    // Enrol the right index finger: a redundant image and a failed capture
    // on the way.
    app.add(
        &env.fake,
        Finger::RightIndex,
        vec![
            Touch::Finger(1),
            Touch::Finger(1),
            Touch::Redundant(1),
            Touch::ScanError,
            Touch::Finger(1),
            Touch::Finger(1),
            Touch::Finger(1),
            Touch::Finger(1),
        ],
    );
    let e = app.enroll();
    assert_eq!(e.outcome, Some(EnrollOutcome::Completed));
    assert_eq!(e.stages, Some(6), "num-enroll-stages read after Claim");
    assert_eq!(e.passed, 6);
    assert_eq!(e.fraction(), Some(1.0));
    assert_eq!(
        app.instructions,
        [
            "Getting the reader ready…",
            "Place your right index finger flat on the reader.",
            "Good. Lift your finger, then place it on the reader again.",
            "Lift your finger completely off the reader, then place it again.",
            "That did not work. Lift your finger, then place it on the reader again.",
            "Good. Lift your finger, then place it on the reader again.",
        ]
    );
    assert_eq!(
        app.sent,
        [
            Command::Connect,
            Command::List,
            Command::Claim,
            Command::EnrollStart(Finger::RightIndex),
            Command::EnrollStop,
            Command::Release,
            Command::List,
        ]
    );
    assert_eq!(app.m.fingers, [Finger::RightIndex]);
    assert!(App::device_is_free(&env.bus.address), "released");
    app.back();

    // A second finger.
    app.add(&env.fake, Finger::LeftThumb, five_good(2));
    assert_eq!(app.enroll().outcome, Some(EnrollOutcome::Completed));
    app.back();
    assert_eq!(app.m.fingers, [Finger::LeftThumb, Finger::RightIndex]);

    // A fresh start lists both, from the service.
    let mut fresh = env.app();
    fresh.start();
    assert_eq!(fresh.m.fingers, [Finger::LeftThumb, Finger::RightIndex]);
    drop(fresh);

    // Test: two prints, so any of them ("any"): recognised, not named.
    assert_eq!(
        app.test_finger(&env.fake, vec![Touch::Finger(2)]),
        TestOutcome::Match(None)
    );
    // Again, with an unknown finger after a failed capture.
    app.instructions.clear();
    env.fake.touch([Touch::ScanError, Touch::Finger(7)]);
    app.input(Input::TryAgain);
    app.until(
        "the second test",
        |m| matches!(&m.page, Page::Test(t) if t.step == Step::Done),
    );
    app.settle();
    assert_eq!(app.test().outcome, Some(TestOutcome::NoMatch));
    assert!(
        app.instructions.contains(
            &"That did not work. Lift your finger, then place it on the reader again.".into()
        ),
        "{:?}",
        app.instructions
    );
    assert!(App::device_is_free(&env.bus.address));
    app.back();

    // Remove the right index finger, after a change of mind.
    app.input(Input::RemoveFinger(Finger::RightIndex));
    assert_eq!(
        app.m.dialog,
        Some(Dialog::ConfirmRemove(Finger::RightIndex))
    );
    app.input(Input::Dismiss);
    assert_eq!(app.m.fingers.len(), 2);
    app.input(Input::RemoveFinger(Finger::RightIndex));
    app.input(Input::Confirm);
    app.settle();
    assert_eq!(app.m.page, Page::Fingers);
    assert!(app.m.dialog.is_none());
    assert_eq!(app.m.fingers, [Finger::LeftThumb]);
    assert_eq!(
        app.m
            .take_notice()
            .unwrap()
            .render(&Catalogue::builtin("fr").unwrap()),
        "Empreinte supprimée\u{a0}: Index droit"
    );
    assert!(
        app.sent
            .contains(&Command::DeleteFinger(Finger::RightIndex))
    );
    assert!(!app.sent.contains(&Command::DeleteAll));
    assert!(App::device_is_free(&env.bus.address));

    // One print left: the service names the finger it checks.
    let outcome = app.test_finger(&env.fake, vec![Touch::Finger(2)]);
    assert_eq!(outcome, TestOutcome::Match(Some(Finger::LeftThumb)));
    let (title, _) = outcome.texts(&app.m.labels).unwrap();
    assert_eq!(
        title.render(&Catalogue::default()),
        "Recognised: Left thumb"
    );
    app.back();

    // The removed print is gone from the sensor: enrolling the same finger
    // image again as another finger works (no duplicate).
    app.add(&env.fake, Finger::RightMiddle, five_good(1));
    assert_eq!(app.enroll().outcome, Some(EnrollOutcome::Completed));
    app.back();
    assert_eq!(app.m.fingers, [Finger::LeftThumb, Finger::RightMiddle]);
}

#[test]
fn duplicate_and_cancel() {
    let env = Env::new();
    let mut app = env.app();
    app.start();
    app.add(&env.fake, Finger::LeftIndex, five_good(3));
    assert_eq!(app.enroll().outcome, Some(EnrollOutcome::Completed));
    app.back();

    // The same physical finger as another finger: a duplicate, found on the
    // first touch.
    app.add(&env.fake, Finger::RightIndex, vec![Touch::Finger(3)]);
    match &app.enroll().outcome {
        Some(EnrollOutcome::Failed { failure, duplicate }) => {
            assert!(duplicate);
            assert!(
                failure
                    .description
                    .render(&Catalogue::default())
                    .starts_with("This fingerprint is already registered")
            );
        }
        o => panic!("{o:?}"),
    }
    assert!(
        App::device_is_free(&env.bus.address),
        "released after a failure"
    );
    // "Try again" with the same finger is not offered for a duplicate.
    app.input(Input::TryAgain);
    assert!(matches!(app.m.page, Page::Enroll(_)));
    app.back();
    assert_eq!(app.m.fingers, [Finger::LeftIndex]);
    assert_eq!(env.fake.pending_touches(), 0);

    // Cancel while the sensor waits for the finger.
    app.input(Input::AddFinger);
    app.input(Input::StartEnroll {
        finger: Some(Finger::RightThumb),
        label: None,
    });
    app.until(
        "scanning",
        |m| matches!(&m.page, Page::Enroll(e) if e.step == Step::Scanning),
    );
    assert_eq!(
        app.enroll().instruction.render(&Catalogue::default()),
        "Place your right thumb flat on the reader."
    );
    app.input(Input::Cancel);
    app.settle();
    assert_eq!(app.m.page, Page::Fingers);
    assert!(
        App::device_is_free(&env.bus.address),
        "released after a cancel"
    );
    assert_eq!(app.m.fingers, [Finger::LeftIndex]);

    // Cancel a test while it waits.
    app.input(Input::TestFinger);
    // The service says which finger it checks just after its reply.
    app.until(
        "the selected finger",
        |m| matches!(&m.page, Page::Test(t) if t.step == Step::Scanning && t.selected.is_some()),
    );
    assert_eq!(
        app.test().instruction.render(&Catalogue::default()),
        "Place the finger \u{201c}Left index finger\u{201d} on the reader."
    );
    app.input(Input::Cancel);
    app.settle();
    assert_eq!(app.m.page, Page::Fingers);
    assert!(App::device_is_free(&env.bus.address));
    let n = app.sent.len();
    assert_eq!(
        app.sent[n - 4..],
        [
            Command::Claim,
            Command::VerifyStart,
            Command::VerifyStop,
            Command::Release
        ]
    );
}

#[test]
fn busy_and_permission_denied() {
    let env = Env::new();
    let mut app = env.app();
    app.start();

    // Another program holds the reader.
    let other = client(&env.bus.address);
    call(&other, DEVICE.0, DEVICE.1, "Claim", Some("")).unwrap();
    app.input(Input::AddFinger);
    app.input(Input::StartEnroll {
        finger: Some(Finger::LeftThumb),
        label: None,
    });
    app.settle();
    match &app.enroll().outcome {
        Some(EnrollOutcome::Failed { failure, .. }) => {
            assert_eq!(failure.title, ErrorKind::Busy.title());
            assert!(
                failure
                    .detail
                    .as_deref()
                    .unwrap()
                    .starts_with("net.reactivated.Fprint.Error.AlreadyInUse")
            );
        }
        o => panic!("{o:?}"),
    }
    // Released by the other program: trying again works.
    call(&other, DEVICE.0, DEVICE.1, "Release", None).unwrap();
    env.fake.touch(five_good(4));
    app.input(Input::TryAgain);
    app.until(
        "the enrolment",
        |m| matches!(&m.page, Page::Enroll(e) if e.step == Step::Done),
    );
    app.settle();
    assert_eq!(app.enroll().outcome, Some(EnrollOutcome::Completed));
    app.back();

    // polkit refuses enrolment: the claim works (verify is allowed), the
    // start does not, and the device is released.
    env.authority.deny(ENROLL);
    app.input(Input::AddFinger);
    app.input(Input::StartEnroll {
        finger: Some(Finger::RightThumb),
        label: None,
    });
    app.settle();
    match &app.enroll().outcome {
        Some(EnrollOutcome::Failed { failure, .. }) => {
            assert_eq!(failure.title, ErrorKind::PermissionDenied.title());
        }
        o => panic!("{o:?}"),
    }
    assert!(app.sent.ends_with(&[
        Command::Claim,
        Command::EnrollStart(Finger::RightThumb),
        Command::Release
    ]));
    assert!(App::device_is_free(&env.bus.address));
    app.back();

    // Removal refused too: an error dialog, nothing removed.
    app.input(Input::RemoveFinger(Finger::LeftThumb));
    app.input(Input::Confirm);
    app.settle();
    match &app.m.dialog {
        Some(Dialog::Error(f)) => assert_eq!(f.title, ErrorKind::PermissionDenied.title()),
        d => panic!("{d:?}"),
    }
    app.input(Input::Confirm);
    assert_eq!(app.m.fingers, [Finger::LeftThumb]);
    assert!(App::device_is_free(&env.bus.address));

    // Not even allowed to list: a problem page at start.
    env.authority.deny(VERIFY);
    let mut denied = env.app();
    denied.start();
    match &denied.m.page {
        Page::Problem(f) => assert_eq!(f.title, ErrorKind::PermissionDenied.title()),
        p => panic!("{p:?}"),
    }
    env.authority.allow_all();
    denied.input(Input::TryAgain);
    denied.settle();
    assert_eq!(denied.m.page, Page::Fingers);
    assert_eq!(denied.m.fingers, [Finger::LeftThumb]);
}

#[test]
fn service_unavailable_no_reader_and_service_lost() {
    // Nothing on the bus.
    let bus = PrivateBus::start();
    let mut app = App::new(&bus.address);
    app.start();
    match &app.m.page {
        Page::Problem(f) => {
            assert_eq!(f.title, ErrorKind::Unavailable.title());
            assert!(
                f.detail
                    .as_deref()
                    .unwrap()
                    .contains("org.freedesktop.DBus.Error.ServiceUnknown"),
                "{:?}",
                f.detail
            );
        }
        p => panic!("{p:?}"),
    }
    // No bus at all.
    let mut nobus = App::new("unix:path=/nonexistent/fingerprint-settings-test");
    nobus.start();
    match &nobus.m.page {
        Page::Problem(f) => assert_eq!(f.title, ErrorKind::Unavailable.title(), "{:?}", f.detail),
        p => panic!("{p:?}"),
    }

    // The daemon, with its sensor unplugged: no reader.
    let mut env = Env::new();
    env.fake.unplug();
    let c = client(&env.bus.address);
    let end = Instant::now() + Duration::from_secs(10);
    while call(&c, MANAGER.0, MANAGER.1, "GetDefaultDevice", None).is_ok() {
        assert!(Instant::now() < end, "the daemon did not notice the unplug");
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut app = env.app();
    app.start();
    match &app.m.page {
        Page::Problem(f) => assert_eq!(f.title, ErrorKind::NoReader.title()),
        p => panic!("{p:?}"),
    }
    // Plugged back: "try again" finds it.
    env.fake.plug();
    wait_ready(&env.bus.address);
    app.input(Input::TryAgain);
    app.settle();
    assert_eq!(app.m.page, Page::Fingers);

    // The service goes away in the middle of an enrolment.
    app.input(Input::AddFinger);
    app.input(Input::StartEnroll {
        finger: Some(Finger::LeftThumb),
        label: None,
    });
    app.until(
        "scanning",
        |m| matches!(&m.page, Page::Enroll(e) if e.step == Step::Scanning),
    );
    env.daemon.take().unwrap().stop().unwrap();
    app.until(
        "the loss to be noticed",
        |m| matches!(&m.page, Page::Enroll(e) if e.step == Step::Done),
    );
    match &app.enroll().outcome {
        Some(EnrollOutcome::Failed { failure, .. }) => {
            assert_eq!(failure.title, ErrorKind::Unavailable.title());
        }
        o => panic!("{o:?}"),
    }
    assert!(!app.m.busy());
}

// ------------------------------------------- an older fprintd, for the fallback

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "net.reactivated.Fprint.Error")]
enum OldError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NoEnrolledPrints(String),
}

struct OldManager;

#[zbus::interface(name = "net.reactivated.Fprint.Manager")]
impl OldManager {
    fn get_default_device(&self) -> zbus::OwnedObjectPath {
        zbus::OwnedObjectPath::try_from(DEVICE.0).unwrap()
    }
}

/// fprintd before DeleteEnrolledFinger: a swipe reader with an unknown number
/// of stages, whose prints can only be deleted all at once.
struct OldDevice {
    fingers: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "net.reactivated.Fprint.Device")]
impl OldDevice {
    fn list_enrolled_fingers(&self, _username: &str) -> Result<Vec<String>, OldError> {
        let f = self.fingers.lock().unwrap().clone();
        if f.is_empty() {
            return Err(OldError::NoEnrolledPrints("none".into()));
        }
        Ok(f)
    }
    fn claim(&self, _username: &str) {}
    fn release(&self) {}
    fn delete_enrolled_fingers2(&self) {
        self.fingers.lock().unwrap().clear();
    }
    #[zbus(property, name = "name")]
    fn name(&self) -> String {
        "Old reader".into()
    }
    #[zbus(property, name = "scan-type")]
    fn scan_type(&self) -> String {
        "swipe".into()
    }
    #[zbus(property, name = "num-enroll-stages")]
    fn num_enroll_stages(&self) -> i32 {
        -1
    }
}

#[test]
fn older_service_removes_all_after_asking() {
    let bus = PrivateBus::start();
    let fingers = Arc::new(Mutex::new(vec![
        "left-thumb".to_string(),
        "right-index-finger".to_string(),
    ]));
    let _service = zbus::block_on(async {
        zbus::connection::Builder::address(bus.address.as_str())
            .serve_at(MANAGER.0, OldManager)
            .serve_at(
                DEVICE.0,
                OldDevice {
                    fingers: fingers.clone(),
                },
            )
            .name("net.reactivated.Fprint")
            .build()
            .await
            .unwrap()
    });
    let mut app = App::new(&bus.address);
    app.start();
    assert_eq!(app.m.page, Page::Fingers);
    assert_eq!(app.m.fingers, [Finger::LeftThumb, Finger::RightIndex]);
    let dev = app.m.device.clone().unwrap();
    assert_eq!(
        (dev.name.as_str(), dev.scan),
        ("Old reader", ScanType::Swipe)
    );

    app.input(Input::RemoveFinger(Finger::LeftThumb));
    app.input(Input::Confirm);
    app.settle();
    // Asked before anything is lost.
    assert_eq!(
        app.m.dialog,
        Some(Dialog::ConfirmRemoveAll(Finger::LeftThumb))
    );
    assert_eq!(fingers.lock().unwrap().len(), 2);
    assert!(app.sent.contains(&Command::DeleteFinger(Finger::LeftThumb)));
    app.input(Input::Confirm);
    app.settle();
    assert!(app.m.dialog.is_none());
    assert!(app.m.fingers.is_empty());
    assert!(fingers.lock().unwrap().is_empty());
    assert_eq!(
        app.m.take_notice().unwrap().render(&Catalogue::default()),
        "All your fingerprints were removed."
    );
    assert_eq!(
        app.sent,
        [
            Command::Connect,
            Command::List,
            Command::Claim,
            Command::DeleteFinger(Finger::LeftThumb),
            Command::Release,
            Command::Claim,
            Command::DeleteAll,
            Command::Release,
            Command::List,
        ]
    );
}

// ------------------------------------------- a service that never answers

struct StuckDevice;

#[zbus::interface(name = "net.reactivated.Fprint.Device")]
impl StuckDevice {
    fn list_enrolled_fingers(&self, _username: &str) -> Vec<String> {
        vec!["left-thumb".into()]
    }
    /// Never answers, like a service waiting on a hung reader.
    async fn claim(&self, _username: &str) {
        std::future::pending::<()>().await;
    }
}

#[test]
fn a_call_without_answer_times_out() {
    let bus = PrivateBus::start();
    let _service = zbus::block_on(async {
        zbus::connection::Builder::address(bus.address.as_str())
            .serve_at(MANAGER.0, OldManager)
            .serve_at(DEVICE.0, StuckDevice)
            .name("net.reactivated.Fprint")
            .build()
            .await
            .unwrap()
    });
    let deadlines = Deadlines {
        call: Duration::from_millis(500),
        interactive: Duration::from_secs(5),
    };
    let mut app = App::with(&bus.address, deadlines);
    app.start();
    assert_eq!(app.m.fingers, [Finger::LeftThumb]);
    let t0 = Instant::now();
    app.input(Input::TestFinger);
    app.until(
        "the claim to time out",
        |m| matches!(&m.page, Page::Test(t) if t.step == Step::Done),
    );
    let waited = t0.elapsed();
    assert!(
        waited >= Duration::from_millis(500) && waited < Duration::from_secs(5),
        "{waited:?}"
    );
    match &app.test().outcome {
        Some(TestOutcome::Failed(f)) => {
            assert_eq!(f.title, ErrorKind::Timeout.title());
            assert_eq!(f.detail.as_deref(), Some("no answer within 0 s"));
        }
        o => panic!("{o:?}"),
    }
    // The claim may have happened after all: released anyway.
    assert!(
        app.sent.ends_with(&[Command::Claim, Command::Release]),
        "{:?}",
        app.sent
    );
    // The driver is still usable afterwards.
    app.back();
    app.input(Input::Start);
    app.settle();
    assert_eq!(app.m.page, Page::Fingers);
}

// ------------------------------------------------- named fingerprints

/// A temporary configuration directory.
struct ConfigDir(TempDir);

impl ConfigDir {
    fn new() -> Self {
        ConfigDir(TempDir::new("config"))
    }
    fn store(&self) -> Store {
        Store::new(self.0.0.join("fingerprint-settings").join("names.toml"))
    }
}

#[test]
fn named_fingerprints_follow_their_slot() {
    let env = Env::new();
    let config = ConfigDir::new();
    // A name left over for a fingerprint that no longer exists.
    let mut store = config.store();
    store
        .save(&Labels::from([(Finger::LeftRing, "Old".to_string())]))
        .unwrap();
    let mut app = App::with_store(&env.bus.address, config.store());
    assert_eq!(app.m.labels.len(), 1);
    app.start();
    // Nothing enrolled: the stale name is dropped, from the file too.
    assert!(app.m.labels.is_empty());
    assert!(config.store().load().labels.is_empty());

    // "Just a name, any finger": the first free slot, the right index.
    let bed = "Pouce, quand je suis au lit";
    app.add_named(&env.fake, None, Some(bed), vec![Touch::Finger(1); 6]);
    assert_eq!(app.enroll().outcome, Some(EnrollOutcome::Completed));
    assert_eq!(app.enroll().slot, Finger::RightIndex);
    assert!(app.sent.contains(&Command::EnrollStart(Finger::RightIndex)));
    // No finger named in the instructions: the user did not say which.
    assert!(
        app.instructions
            .contains(&"Place your finger flat on the reader.".to_string()),
        "{:?}",
        app.instructions
    );
    assert_eq!(
        app.enroll()
            .title()
            .render(&Catalogue::builtin("fr").unwrap()),
        "Ajout de «\u{a0}Pouce, quand je suis au lit\u{a0}»"
    );
    app.back();
    assert_eq!(app.m.fingers, [Finger::RightIndex]);
    assert_eq!(
        config.store().load().labels,
        Labels::from([(Finger::RightIndex, bed.to_string())])
    );

    // One print: the test names it.
    app.instructions.clear();
    let outcome = app.test_finger(&env.fake, vec![Touch::Finger(1)]);
    assert_eq!(outcome, TestOutcome::Match(Some(Finger::RightIndex)));
    let (title, _) = outcome.texts(&app.m.labels).unwrap();
    assert_eq!(
        title.render(&Catalogue::builtin("fr").unwrap()),
        "Reconnu\u{a0}: Pouce, quand je suis au lit"
    );
    assert!(
        app.instructions.contains(
            &"Place the finger \u{201c}Pouce, quand je suis au lit\u{201d} on the reader."
                .to_string()
        ),
        "{:?}",
        app.instructions
    );
    app.back();

    // A chosen finger with its standard name: no name stored.
    app.add(&env.fake, Finger::LeftIndex, vec![Touch::Finger(2); 6]);
    assert_eq!(app.enroll().outcome, Some(EnrollOutcome::Completed));
    app.back();
    // Another nameless choice: indexes are taken, so the right thumb.
    app.add_named(&env.fake, None, Some("Travail"), vec![Touch::Finger(3); 6]);
    assert_eq!(app.enroll().slot, Finger::RightThumb);
    app.back();
    assert_eq!(
        app.m.fingers,
        [Finger::LeftIndex, Finger::RightThumb, Finger::RightIndex]
    );
    assert_eq!(
        config.store().load().labels,
        Labels::from([
            (Finger::RightIndex, bed.to_string()),
            (Finger::RightThumb, "Travail".to_string()),
        ])
    );

    // Rename, then back to the standard name.
    app.input(Input::RenameFinger(Finger::LeftIndex));
    assert_eq!(app.m.dialog, Some(Dialog::Rename(Finger::LeftIndex)));
    app.input(Input::SetLabel(
        Finger::LeftIndex,
        Some("  Index de travail ".into()),
    ));
    assert!(app.m.dialog.is_none());
    assert_eq!(app.m.labels[&Finger::LeftIndex], "Index de travail");
    assert_eq!(
        config.store().load().labels[&Finger::LeftIndex],
        "Index de travail"
    );
    app.input(Input::RenameFinger(Finger::LeftIndex));
    app.input(Input::SetLabel(Finger::LeftIndex, None));
    assert!(
        !config
            .store()
            .load()
            .labels
            .contains_key(&Finger::LeftIndex)
    );

    // Removing a fingerprint removes its name.
    app.input(Input::RemoveFinger(Finger::RightIndex));
    app.input(Input::Confirm);
    app.settle();
    assert_eq!(
        app.m.take_notice().unwrap().render(&Catalogue::default()),
        "Fingerprint removed: Pouce, quand je suis au lit"
    );
    assert!(!app.m.labels.contains_key(&Finger::RightIndex));
    assert_eq!(
        config.store().load().labels,
        Labels::from([(Finger::RightThumb, "Travail".to_string())])
    );
    // The slot is free again for the next nameless fingerprint.
    app.add_named(&env.fake, None, Some("Encore"), vec![Touch::Finger(4); 6]);
    assert_eq!(app.enroll().slot, Finger::RightIndex);
    app.back();
    assert_eq!(config.store().load().labels, app.m.labels);
}
