//! The fingerprint service over D-Bus: runs [`Command`]s and turns their
//! replies and the device's signals into [`Input`]s.
//!
//! Only the standard fprintd API is used (`net.reactivated.Fprint`, as
//! documented by fprintd 1.94), so any compatible service works: fprintd
//! itself, open-fprintd, validity-rs's daemon...
//!
//! The driver runs on a thread of its own ([`Service::start`]), driven by
//! `zbus::block_on`; the UI talks to it through two channels that work under
//! any executor, GLib's included.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_core::Stream;
use zbus::Connection;
use zbus::OwnedObjectPath;
use zruntime::mpmc::{Receiver, Sender, unbounded};

use crate::error::{ErrorKind, ServiceError};
use crate::machine::{Command, FINGER, Input};

/// The service's well-known bus name.
pub const BUS_NAME: &str = "net.reactivated.Fprint";
/// The device interface.
pub const DEVICE_INTERFACE: &str = "net.reactivated.Fprint.Device";

/// How long a call may take before the user is told the service does not
/// answer. Generous: fprintd opens the device on Claim, and a delete may
/// rewrite the sensor's storage.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a call that may ask the user for their password may take:
/// fprintd's default polkit policy authenticates the user (`auth_self_keep`)
/// before an enrolment or a removal, and the call waits for the answer.
pub const INTERACTIVE_TIMEOUT: Duration = Duration::from_secs(600);

#[zbus::proxy(
    interface = "net.reactivated.Fprint.Manager",
    default_service = "net.reactivated.Fprint",
    default_path = "/net/reactivated/Fprint/Manager"
)]
trait Manager {
    fn get_default_device(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "net.reactivated.Fprint.Device",
    default_service = "net.reactivated.Fprint"
)]
trait Device {
    // Every call is checked by polkit, which may ask the user for their
    // password (through the desktop's polkit agent): say that it may.
    #[zbus(allow_interactive_auth)]
    fn list_enrolled_fingers(&self, username: &str) -> zbus::Result<Vec<String>>;
    #[zbus(allow_interactive_auth)]
    fn delete_enrolled_finger(&self, finger_name: &str) -> zbus::Result<()>;
    #[zbus(allow_interactive_auth)]
    fn delete_enrolled_fingers2(&self) -> zbus::Result<()>;
    #[zbus(allow_interactive_auth)]
    fn claim(&self, username: &str) -> zbus::Result<()>;
    #[zbus(allow_interactive_auth)]
    fn release(&self) -> zbus::Result<()>;
    #[zbus(allow_interactive_auth)]
    fn enroll_start(&self, finger_name: &str) -> zbus::Result<()>;
    #[zbus(allow_interactive_auth)]
    fn enroll_stop(&self) -> zbus::Result<()>;

    #[zbus(property, name = "num-enroll-stages")]
    fn num_enroll_stages(&self) -> zbus::Result<i32>;
}

/// Which bus the service is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bus {
    /// The system bus (honours `DBUS_SYSTEM_BUS_ADDRESS`).
    System,
    /// A bus at this address (tests).
    Address(String),
}

/// The device's signals, in bus order, and the service's departures.
struct Signals {
    device: zbus::MessageStream,
    owner: zbus::fdo::NameOwnerChangedStream,
}

impl Signals {
    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<Input>> {
        loop {
            match Pin::new(&mut self.device).poll_next(cx) {
                Poll::Ready(Some(Ok(m))) => {
                    if let Some(i) = signal_input(&m) {
                        return Poll::Ready(Some(i));
                    }
                    continue;
                }
                Poll::Ready(Some(Err(_))) => continue,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => {}
            }
            return match Pin::new(&mut self.owner).poll_next(cx) {
                Poll::Ready(Some(s)) => match s.args() {
                    Ok(a) if a.new_owner().is_none() => Poll::Ready(Some(Input::ServiceLost)),
                    _ => continue,
                },
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            };
        }
    }
}

/// The input a device signal stands for (`None` for other signals).
fn signal_input(m: &zbus::Message) -> Option<Input> {
    let h = m.header();
    if h.interface().map(|i| i.as_str()) != Some(DEVICE_INTERFACE) {
        return None;
    }
    if h.member()?.as_str() != "EnrollStatus" {
        return None;
    }
    let (result, done): (String, bool) = m.body().deserialize().ok()?;
    Some(Input::EnrollStatus(result, done))
}

/// Runs commands against the service.
struct Driver {
    bus: Bus,
    deadlines: Deadlines,
    conn: Option<Connection>,
    device: Option<DeviceProxy<'static>>,
    signals: Option<Signals>,
}

fn not_connected() -> ServiceError {
    ServiceError::new(
        ErrorKind::Unavailable,
        "not connected to the fingerprint service",
    )
}

fn res<T>(r: zbus::Result<T>) -> Result<T, ServiceError> {
    r.map_err(|e| ServiceError::from_zbus(&e))
}

impl Driver {
    async fn connection(&mut self) -> Result<Connection, ServiceError> {
        if let Some(c) = &self.conn {
            return Ok(c.clone());
        }
        let builder = match &self.bus {
            Bus::System => zbus::connection::Builder::system(),
            Bus::Address(a) => zbus::connection::Builder::address(a.as_str()),
        };
        // A backstop: each command has its own deadline (exec).
        let backstop = self.deadlines.call.max(self.deadlines.interactive);
        let conn = res(builder.method_timeout(backstop).build().await)?;
        self.conn = Some(conn.clone());
        Ok(conn)
    }

    async fn connect(&mut self) -> Result<(), ServiceError> {
        let conn = self.connection().await?;
        let manager = res(ManagerProxy::new(&conn).await)?;
        let path = res(manager.get_default_device().await)?;
        let device = res(DeviceProxy::builder(&conn)
            .path(path.clone())
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await)?;
        // Signals first, so that none is missed once a call is made.
        if self.signals.is_none() {
            let rule = res(zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface(DEVICE_INTERFACE)
                .path(path.clone())
                .build())?;
            let device_stream = res(zbus::MessageStream::for_match_rule(
                rule,
                &conn,
                std::num::NonZeroUsize::new(256),
            )
            .await)?;
            let dbus = res(zbus::fdo::DBusProxy::new(&conn).await)?;
            let owner = res(dbus
                .receive_name_owner_changed_with_args(&[(0, BUS_NAME)])
                .await)?;
            self.signals = Some(Signals {
                device: device_stream,
                owner,
            });
        }
        self.device = Some(device);
        Ok(())
    }

    fn device(&self) -> Result<&DeviceProxy<'static>, ServiceError> {
        self.device.as_ref().ok_or_else(not_connected)
    }

    /// Runs `cmd` within its deadline.
    async fn exec(&mut self, cmd: Command) -> Input {
        let limit = self.deadlines.of(&cmd);
        let late = timed_out(&cmd, limit);
        match zruntime::SharedRuntime::current() {
            Ok(rt) => rt.timeout(limit, self.run(cmd)).await.unwrap_or(late),
            Err(_) => self.run(cmd).await,
        }
    }

    async fn run(&mut self, cmd: Command) -> Input {
        match cmd {
            Command::Connect => Input::Connected(self.connect().await),
            Command::List => Input::Listed(match self.device() {
                Ok(d) => match d.list_enrolled_fingers("").await {
                    Ok(names) => Ok(names),
                    Err(e) => {
                        let e = ServiceError::from_zbus(&e);
                        if e.kind == ErrorKind::NoPrints {
                            Ok(Vec::new())
                        } else {
                            Err(e)
                        }
                    }
                },
                Err(e) => Err(e),
            }),
            Command::Claim => Input::Claimed(match self.device() {
                Ok(d) => match res(d.claim("").await) {
                    // Only meaningful once claimed; -1 means unknown.
                    Ok(()) => Ok(d
                        .num_enroll_stages()
                        .await
                        .ok()
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|n| *n > 0)),
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            }),
            Command::Release => Input::Released(self.call(|d| d.release()).await),
            Command::EnrollStart => {
                Input::EnrollStarted(self.call(|d| d.enroll_start(FINGER)).await)
            }
            Command::EnrollStop => Input::EnrollStopped(self.call(|d| d.enroll_stop()).await),
            Command::DeleteFinger => {
                Input::Deleted(self.call(|d| d.delete_enrolled_finger(FINGER)).await)
            }
            Command::DeleteAll => Input::Deleted(self.call(|d| d.delete_enrolled_fingers2()).await),
        }
    }

    async fn call<'a, F, Fut>(&'a self, f: F) -> Result<(), ServiceError>
    where
        F: FnOnce(&'a DeviceProxy<'static>) -> Fut,
        Fut: Future<Output = zbus::Result<()>> + 'a,
    {
        res(f(self.device()?).await)
    }
}

/// How long the service has to answer each command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadlines {
    /// Most calls ([`CALL_TIMEOUT`]).
    pub call: Duration,
    /// Calls that may wait for the user's password ([`INTERACTIVE_TIMEOUT`]).
    pub interactive: Duration,
}

impl Default for Deadlines {
    fn default() -> Self {
        Deadlines {
            call: CALL_TIMEOUT,
            interactive: INTERACTIVE_TIMEOUT,
        }
    }
}

impl Deadlines {
    /// How long `cmd` may take.
    pub fn of(&self, cmd: &Command) -> Duration {
        match cmd {
            Command::EnrollStart | Command::DeleteFinger | Command::DeleteAll => self.interactive,
            _ => self.call,
        }
    }
}

/// The answer to `cmd` when the service did not give one within `limit`.
pub fn timed_out(cmd: &Command, limit: Duration) -> Input {
    let e = ServiceError::new(
        ErrorKind::Timeout,
        format!("no answer within {} s", limit.as_secs()),
    );
    match cmd {
        Command::Connect => Input::Connected(Err(e)),
        Command::List => Input::Listed(Err(e)),
        Command::Claim => Input::Claimed(Err(e)),
        Command::Release => Input::Released(Err(e)),
        Command::EnrollStart => Input::EnrollStarted(Err(e)),
        Command::EnrollStop => Input::EnrollStopped(Err(e)),
        Command::DeleteFinger | Command::DeleteAll => Input::Deleted(Err(e)),
    }
}

enum Event {
    Command(Command),
    Signal(Input),
    SignalsEnded,
    Closed,
}

async fn next_event(commands: &Receiver<Command>, signals: Option<&mut Signals>) -> Event {
    let mut recv = std::pin::pin!(commands.recv());
    let mut signals = signals;
    std::future::poll_fn(|cx| {
        // Signals first: one that arrived during a call is delivered before
        // the next call is made.
        if let Some(s) = signals.as_deref_mut() {
            match s.poll_next(cx) {
                Poll::Ready(Some(i)) => return Poll::Ready(Event::Signal(i)),
                Poll::Ready(None) => return Poll::Ready(Event::SignalsEnded),
                Poll::Pending => {}
            }
        }
        match recv.as_mut().poll(cx) {
            Poll::Ready(Ok(c)) => Poll::Ready(Event::Command(c)),
            Poll::Ready(Err(_)) => Poll::Ready(Event::Closed),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

/// Runs commands from `commands` until it closes, sending every answer and
/// signal to `inputs`. Each command is answered by exactly one input, in
/// order.
pub async fn run(
    bus: Bus,
    deadlines: Deadlines,
    commands: Receiver<Command>,
    inputs: Sender<Input>,
) {
    let mut d = Driver {
        bus,
        deadlines,
        conn: None,
        device: None,
        signals: None,
    };
    loop {
        let input = match next_event(&commands, d.signals.as_mut()).await {
            Event::Command(c) => d.exec(c).await,
            Event::Signal(i) => i,
            Event::SignalsEnded => {
                d.signals = None;
                continue;
            }
            Event::Closed => return,
        };
        if inputs.send(input).await.is_err() {
            return;
        }
    }
}

/// The driver on its own thread.
#[derive(Debug)]
pub struct Service {
    commands: Sender<Command>,
    inputs: Receiver<Input>,
}

impl Service {
    /// Starts the driver thread on `bus`, with the default deadlines.
    pub fn start(bus: Bus) -> std::io::Result<Service> {
        Service::start_with(bus, Deadlines::default())
    }

    /// Starts the driver thread on `bus`.
    pub fn start_with(bus: Bus, deadlines: Deadlines) -> std::io::Result<Service> {
        let (commands, command_rx) = unbounded();
        let (input_tx, inputs) = unbounded();
        std::thread::Builder::new()
            .name("fprint".into())
            .spawn(move || zbus::block_on(run(bus, deadlines, command_rx, input_tx)))?;
        Ok(Service { commands, inputs })
    }

    /// Queues a command. Its answer comes on [`Service::inputs`].
    pub fn send(&self, cmd: Command) {
        // Unbounded: only fails once the thread is gone, and then there is
        // nobody to answer anyway.
        let _ = self.commands.try_send(cmd);
    }

    /// The answers and signals.
    pub fn inputs(&self) -> &Receiver<Input> {
        &self.inputs
    }

    /// Ends the driver thread once the call in flight returns; later
    /// commands are dropped.
    pub fn close(&self) {
        self.commands.close();
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Model;

    const ALL: [Command; 8] = [
        Command::Connect,
        Command::List,
        Command::Claim,
        Command::Release,
        Command::EnrollStart,
        Command::EnrollStop,
        Command::DeleteFinger,
        Command::DeleteAll,
    ];

    #[test]
    fn deadlines() {
        let d0 = Deadlines::default();
        assert_eq!(
            (d0.call, d0.interactive),
            (CALL_TIMEOUT, INTERACTIVE_TIMEOUT)
        );
        for c in ALL {
            let d = d0.of(&c);
            // Calls that may wait for the user's password get ten minutes,
            // the others a minute.
            let interactive = matches!(
                c,
                Command::EnrollStart | Command::DeleteFinger | Command::DeleteAll
            );
            assert_eq!(
                d,
                if interactive {
                    INTERACTIVE_TIMEOUT
                } else {
                    CALL_TIMEOUT
                },
                "{c:?}"
            );
        }
        assert!(INTERACTIVE_TIMEOUT > CALL_TIMEOUT);
    }

    /// A timed-out command is answered with its own reply type, as a
    /// timeout, and the machine treats it like any failed call: the user is
    /// told the service did not answer, and a claim is still released.
    #[test]
    fn a_timeout_answers_the_command_it_belongs_to() {
        for c in ALL {
            let i = timed_out(&c, Duration::from_secs(7));
            let err = match (&c, &i) {
                (Command::Connect, Input::Connected(Err(e)))
                | (Command::List, Input::Listed(Err(e)))
                | (Command::Claim, Input::Claimed(Err(e)))
                | (Command::Release, Input::Released(Err(e)))
                | (Command::EnrollStart, Input::EnrollStarted(Err(e)))
                | (Command::EnrollStop, Input::EnrollStopped(Err(e)))
                | (Command::DeleteFinger | Command::DeleteAll, Input::Deleted(Err(e))) => e,
                _ => panic!("{c:?} answered by {i:?}"),
            };
            assert_eq!(err.kind, ErrorKind::Timeout);
            assert_eq!(err.detail, "no answer within 7 s");
        }
        // End to end in the machine: a start that never answers.
        let mut m = Model::new();
        m.update(Input::Start);
        m.update(Input::Connected(Ok(())));
        m.update(Input::Listed(Ok(vec![])));
        assert_eq!(m.update(Input::Enroll), [Command::Claim]);
        let start = Command::EnrollStart;
        assert_eq!(m.update(Input::Claimed(Ok(Some(5)))), [start]);
        assert_eq!(
            m.update(timed_out(&start, Deadlines::default().of(&start))),
            [Command::Release]
        );
        m.update(Input::Released(Ok(())));
        assert_eq!(m.message, Some(ErrorKind::Timeout.message()));
    }
}
