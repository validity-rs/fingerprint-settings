//! The application core against a real fingerprint service on a private bus:
//! validity-rs's daemon with its emulated sensor, driven through the same
//! D-Bus driver and state machine as the GTK interface, from start to
//! enrolment of the right index finger and its removal, and through the error
//! paths.
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
use fingerprint_settings::fprint::{Bus, Deadlines, Service};
use fingerprint_settings::i18n::Catalogue;
use fingerprint_settings::machine::{Activity, Command, FINGER, Input, Model, Step};
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
        }
    }

    fn input(&mut self, i: Input) {
        for c in self.m.update(i) {
            self.sent.push(c);
            self.service.send(c);
        }
        if let Activity::Enrolling(e) = &self.m.activity {
            let text = e.instruction.render(&Catalogue::default());
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
                        "timed out waiting for {what}; activity {:?}, sent {:?}",
                        self.m.activity,
                        self.sent
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("the driver is gone: {e:?}"),
            }
        }
    }

    /// Feeds inputs until nothing is in flight.
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

    /// "Register my right index finger", the emulated sensor seeing
    /// `touches`, until it is over.
    fn enroll(&mut self, fake: &FakeSensor, touches: Vec<Touch>) {
        fake.touch(touches);
        self.input(Input::Enroll);
        self.settle();
    }

    /// "Remove", until it is over.
    fn remove(&mut self) {
        self.input(Input::Remove);
        self.settle();
    }

    /// The message shown, in French.
    fn said(&self) -> String {
        self.m
            .message
            .as_ref()
            .expect("a message")
            .render(&Catalogue::builtin("fr").unwrap())
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

/// The user's enrolled fingers, as another client sees them.
fn list(c: &Connection) -> Vec<String> {
    zbus::block_on(async {
        match c
            .call_method(
                Some("net.reactivated.Fprint"),
                DEVICE.0,
                Some(DEVICE.1),
                "ListEnrolledFingers",
                &("",),
            )
            .await
        {
            Ok(reply) => reply.body().deserialize().unwrap(),
            Err(_) => Vec::new(),
        }
    })
}

/// Enrols `finger` with image `image` the way another tool would
/// (`fprintd-enroll`), straight over D-Bus.
fn enroll_other(address: &str, fake: &FakeSensor, finger: &str, image: u32) {
    let c = client(address);
    fake.touch(five_good(image));
    call(&c, DEVICE.0, DEVICE.1, "Claim", Some("")).unwrap();
    call(&c, DEVICE.0, DEVICE.1, "EnrollStart", Some(finger)).unwrap();
    let end = Instant::now() + TIMEOUT;
    while !list(&c).iter().any(|f| f == finger) {
        assert!(Instant::now() < end, "{finger} was not enrolled");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = call(&c, DEVICE.0, DEVICE.1, "EnrollStop", None);
    call(&c, DEVICE.0, DEVICE.1, "Release", None).unwrap();
}

// ------------------------------------------------------------ the tests

#[test]
fn enrol_and_remove() {
    let env = Env::new();
    let mut app = env.app();
    app.start();
    assert_eq!(app.m.enrolled, Some(false));
    assert_eq!(
        app.m
            .status()
            .unwrap()
            .render(&Catalogue::builtin("fr").unwrap()),
        "Aucune empreinte enregistrée"
    );
    assert!(app.m.can_enroll() && !app.m.shows_remove());

    // A redundant image and a failed capture on the way.
    app.enroll(
        &env.fake,
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
    assert_eq!(app.said(), "Enregistrement terminé.");
    assert_eq!(app.m.enrolled, Some(true));
    assert_eq!(
        app.m
            .status()
            .unwrap()
            .render(&Catalogue::builtin("fr").unwrap()),
        "Index droit enregistré"
    );
    assert_eq!(
        app.instructions,
        [
            "Getting the reader ready…",
            "Place your finger…",
            "Again… 1/6",
            "Again… 2/6",
            "Poorly read, place your finger again.",
            "Again… 3/6",
            "Again… 4/6",
            "Again… 5/6",
        ]
    );
    assert_eq!(
        app.sent,
        [
            Command::Connect,
            Command::List,
            Command::Claim,
            Command::EnrollStart,
            Command::EnrollStop,
            Command::Release,
            Command::List,
        ]
    );
    assert!(App::device_is_free(&env.bus.address), "released");
    // Stored as the right index finger.
    assert_eq!(list(&client(&env.bus.address)), [FINGER]);

    // A fresh start sees it, from the service.
    let mut fresh = env.app();
    fresh.start();
    assert_eq!(fresh.m.enrolled, Some(true));
    assert!(fresh.m.can_remove() && !fresh.m.can_enroll());
    drop(fresh);

    app.remove();
    assert_eq!(app.said(), "Empreinte supprimée.");
    assert_eq!(app.m.enrolled, Some(false));
    assert!(app.sent.ends_with(&[
        Command::Claim,
        Command::DeleteFinger,
        Command::Release,
        Command::List
    ]));
    assert!(!app.sent.contains(&Command::DeleteAll));
    assert!(App::device_is_free(&env.bus.address));
    assert!(list(&client(&env.bus.address)).is_empty());

    // The removed print is gone from the sensor: the same finger enrols
    // again (no duplicate).
    app.enroll(&env.fake, five_good(1));
    assert_eq!(app.said(), "Enregistrement terminé.");
    assert_eq!(app.m.enrolled, Some(true));
}

#[test]
fn duplicate_other_fingers_and_cancel() {
    let env = Env::new();
    // Another finger, enrolled with another tool: not the right index.
    enroll_other(&env.bus.address, &env.fake, "left-thumb", 3);
    let mut app = env.app();
    app.start();
    assert_eq!(app.m.enrolled, Some(false));

    // The same physical finger as the right index: a duplicate, found on
    // the first touch.
    app.enroll(&env.fake, vec![Touch::Finger(3)]);
    assert_eq!(
        app.said(),
        "Ce doigt est déjà enregistré (peut-être sur un autre compte)."
    );
    assert_eq!(app.m.enrolled, Some(false));
    assert!(
        App::device_is_free(&env.bus.address),
        "released after a failure"
    );
    assert_eq!(env.fake.pending_touches(), 0);

    // Cancel while the sensor waits for the finger.
    app.input(Input::Enroll);
    app.until(
        "scanning",
        |m| matches!(&m.activity, Activity::Enrolling(e) if e.step == Step::Scanning),
    );
    assert_eq!(app.instructions.last().unwrap(), "Place your finger…");
    app.input(Input::Cancel);
    app.settle();
    assert_eq!(app.said(), "Enregistrement annulé.");
    assert!(
        App::device_is_free(&env.bus.address),
        "released after a cancel"
    );
    assert!(app.sent.ends_with(&[
        Command::Claim,
        Command::EnrollStart,
        Command::EnrollStop,
        Command::Release
    ]));

    // And it still works afterwards.
    app.enroll(&env.fake, five_good(4));
    assert_eq!(app.m.enrolled, Some(true));
    let mut fingers = list(&client(&env.bus.address));
    fingers.sort();
    assert_eq!(fingers, ["left-thumb", FINGER]);
}

#[test]
fn busy_and_permission_denied() {
    let env = Env::new();
    let mut app = env.app();
    app.start();

    // Another program holds the reader.
    let other = client(&env.bus.address);
    call(&other, DEVICE.0, DEVICE.1, "Claim", Some("")).unwrap();
    app.enroll(&env.fake, vec![]);
    assert_eq!(app.m.message, Some(ErrorKind::Busy.message()));
    assert!(app.m.can_enroll());
    // Released by the other program: trying again works.
    call(&other, DEVICE.0, DEVICE.1, "Release", None).unwrap();
    app.enroll(&env.fake, five_good(4));
    assert_eq!(app.m.enrolled, Some(true));

    // polkit refuses removal: nothing removed, the device released.
    env.authority.deny(ENROLL);
    app.remove();
    assert_eq!(app.said(), "Autorisation refusée.");
    assert_eq!(app.m.enrolled, Some(true));
    assert!(App::device_is_free(&env.bus.address));

    // polkit refuses enrolment: the claim works (verify is allowed), the
    // start does not, and the device is released.
    env.authority.allow_all();
    app.remove();
    assert_eq!(app.m.enrolled, Some(false));
    env.authority.deny(ENROLL);
    app.enroll(&env.fake, vec![]);
    assert_eq!(app.said(), "Autorisation refusée.");
    assert!(
        app.sent
            .ends_with(&[Command::Claim, Command::EnrollStart, Command::Release])
    );
    assert!(App::device_is_free(&env.bus.address));

    // Not even allowed to list: said at start, and the app stays usable.
    env.authority.deny(VERIFY);
    let mut denied = env.app();
    denied.start();
    assert_eq!(denied.said(), "Autorisation refusée.");
    assert_eq!(denied.m.enrolled, None);
    env.authority.allow_all();
    denied.start();
    assert_eq!(denied.m.enrolled, Some(false));
    assert_eq!(denied.m.message, None);
}

#[test]
fn service_unavailable_no_reader_and_service_lost() {
    // Nothing on the bus.
    let bus = PrivateBus::start();
    let mut app = App::new(&bus.address);
    app.start();
    assert_eq!(app.said(), "Le service d'empreintes n'est pas disponible.");
    assert!(app.m.can_enroll());
    // No bus at all.
    let mut nobus = App::new("unix:path=/nonexistent/fingerprint-settings-test");
    nobus.start();
    assert_eq!(nobus.m.message, Some(ErrorKind::Unavailable.message()));

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
    assert_eq!(app.said(), "Aucun lecteur d'empreintes détecté.");
    // Plugged back: the button finds it and enrols.
    env.fake.plug();
    wait_ready(&env.bus.address);
    app.enroll(&env.fake, five_good(1));
    assert_eq!(app.said(), "Enregistrement terminé.");
    assert_eq!(app.m.enrolled, Some(true));
    app.remove();
    assert_eq!(app.m.enrolled, Some(false));

    // The service goes away in the middle of an enrolment.
    app.input(Input::Enroll);
    app.until(
        "scanning",
        |m| matches!(&m.activity, Activity::Enrolling(e) if e.step == Step::Scanning),
    );
    env.daemon.take().unwrap().stop().unwrap();
    app.until("the loss to be noticed", |m| !m.busy());
    assert_eq!(app.said(), "Le service d'empreintes n'est pas disponible.");
    assert!(app.m.can_enroll());
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

/// fprintd before DeleteEnrolledFinger: prints can only be deleted all at
/// once.
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
    #[zbus(property, name = "num-enroll-stages")]
    fn num_enroll_stages(&self) -> i32 {
        -1
    }
}

#[test]
fn older_service_removes_all() {
    let bus = PrivateBus::start();
    let fingers = Arc::new(Mutex::new(vec![FINGER.to_string()]));
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
    assert_eq!(app.m.enrolled, Some(true));

    app.remove();
    assert_eq!(app.said(), "Empreinte supprimée.");
    assert_eq!(app.m.enrolled, Some(false));
    assert!(fingers.lock().unwrap().is_empty());
    assert_eq!(
        app.sent,
        [
            Command::Connect,
            Command::List,
            Command::Claim,
            Command::DeleteFinger,
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
    assert_eq!(app.m.enrolled, Some(false));
    let t0 = Instant::now();
    app.input(Input::Enroll);
    app.settle();
    let waited = t0.elapsed();
    assert!(
        waited >= Duration::from_millis(500) && waited < Duration::from_secs(5),
        "{waited:?}"
    );
    assert_eq!(
        app.said(),
        "Le service d'empreintes ne répond pas. Réessayez."
    );
    // The claim may have happened after all: released anyway.
    assert!(
        app.sent.ends_with(&[Command::Claim, Command::Release]),
        "{:?}",
        app.sent
    );
    // The driver is still usable afterwards.
    app.start();
    assert_eq!(app.m.enrolled, Some(false));
    assert_eq!(app.m.message, None);
}
