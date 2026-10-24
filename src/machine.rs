//! The application's state machine, free of any UI and of D-Bus itself.
//!
//! The application manages one fingerprint: the user's right index finger.
//! [`Model::update`] takes an [`Input`] (something the user did, or a reply or
//! signal from the fingerprint service) and returns the [`Command`]s to send
//! to the service. The model is the view state: the UI shows
//! [`Model::status`], [`Model::activity`] and [`Model::message`] as they are.
//!
//! The service is used one call at a time: a command is only issued once the
//! input answering the previous one arrived. Inputs that do not fit the
//! current state (a late signal, a double click) are ignored.
//!
//! Every path that claimed the device releases it, whatever happens next
//! (cancel, failure, error), unless the service itself went away. A Claim
//! that timed out is released too: the service may have claimed the device
//! after the application stopped waiting. Whatever fails, the model goes
//! back to [`Activity::Idle`] with a sentence in [`Model::message`]: the
//! application stays usable.

use crate::error::{ErrorKind, ServiceError};
use crate::message::{Message, msg};
use crate::status::{self, EnrollResult};

/// The fprintd name of the one finger the application manages.
pub const FINGER: &str = "right-index-finger";

/// A call to make on the fingerprint service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Find the default reader (`Manager.GetDefaultDevice`); answered by
    /// [`Input::Connected`].
    Connect,
    /// `ListEnrolledFingers("")`; answered by [`Input::Listed`].
    List,
    /// `Claim("")`, then read `num-enroll-stages`; answered by [`Input::Claimed`].
    Claim,
    /// `Release()`; answered by [`Input::Released`].
    Release,
    /// `EnrollStart(FINGER)`; answered by [`Input::EnrollStarted`].
    EnrollStart,
    /// `EnrollStop()`; answered by [`Input::EnrollStopped`].
    EnrollStop,
    /// `DeleteEnrolledFinger(FINGER)`; answered by [`Input::Deleted`].
    DeleteFinger,
    /// `DeleteEnrolledFingers2()`, for services without
    /// `DeleteEnrolledFinger`; answered by [`Input::Deleted`].
    DeleteAll,
}

/// Something that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    // ---- the user
    /// The window opened: find the reader and read what is enrolled.
    Start,
    /// "Register my right index finger".
    Enroll,
    /// "Remove".
    Remove,
    /// "Cancel" during an enrolment (or the window is closing).
    Cancel,

    // ---- the service
    /// The answer to [`Command::Connect`].
    Connected(Result<(), ServiceError>),
    /// The answer to [`Command::List`]: the enrolled fingers' fprintd names
    /// (no enrolled prints is an empty list).
    Listed(Result<Vec<String>, ServiceError>),
    /// The answer to [`Command::Claim`]: the number of enrolment stages, when
    /// the device says (`num-enroll-stages` > 0).
    Claimed(Result<Option<u32>, ServiceError>),
    /// The answer to [`Command::Release`].
    Released(Result<(), ServiceError>),
    /// The answer to [`Command::EnrollStart`].
    EnrollStarted(Result<(), ServiceError>),
    /// The `EnrollStatus` signal: result and done.
    EnrollStatus(String, bool),
    /// The answer to [`Command::EnrollStop`].
    EnrollStopped(Result<(), ServiceError>),
    /// The answer to [`Command::DeleteFinger`] or [`Command::DeleteAll`].
    Deleted(Result<(), ServiceError>),
    /// The service left the bus (it crashed, was stopped, or exited when
    /// idle, as fprintd does).
    ServiceLost,
}

/// Where an operation on a claimed device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `Claim` sent.
    Claiming,
    /// `EnrollStart` or a delete sent.
    Starting,
    /// Waiting for the finger.
    Scanning,
    /// `EnrollStop` sent.
    Stopping,
    /// `Release` sent.
    Releasing,
}

/// How an enrolment ended (decided before the release).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The finger is enrolled.
    Completed,
    /// The user cancelled.
    Cancelled,
    /// It failed: what to tell the user.
    Failed(Message),
}

/// An enrolment in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enroll {
    /// The number of good scans needed, when the device says.
    pub stages: Option<u32>,
    /// Good scans so far.
    pub passed: u32,
    /// Where it is.
    pub step: Step,
    /// What to tell the user now.
    pub instruction: Message,
    /// How it ended (set from the moment it is decided).
    pub outcome: Option<Outcome>,
    cancel: bool,
}

impl Enroll {
    /// Progress from 0 to 1 (`None`: the number of stages is unknown).
    pub fn fraction(&self) -> Option<f64> {
        if self.outcome == Some(Outcome::Completed) {
            return Some(1.0);
        }
        self.stages
            .map(|n| (f64::from(self.passed) / f64::from(n)).clamp(0.0, 1.0))
    }

    /// Whether "Cancel" still means something: the enrolment is not stopping
    /// or over already.
    pub fn cancellable(&self) -> bool {
        self.outcome.is_none()
    }
}

/// What a removal ended with, before the release.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Removed {
    Yes,
    Failed(ServiceError),
}

/// A removal in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remove {
    /// Every print of the user is being removed (services without
    /// `DeleteEnrolledFinger`).
    pub all: bool,
    /// Where it is (`Claiming`, `Starting` = deleting, `Releasing`).
    pub step: Step,
    result: Option<Removed>,
}

/// What the application is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    /// Looking for the reader and reading what is enrolled. `then_enroll`:
    /// the user asked to enrol while no reader was found, so the enrolment
    /// starts once it is.
    Loading {
        /// Enrol once connected.
        then_enroll: bool,
    },
    /// Nothing: waiting for the user.
    Idle,
    /// Enrolling the right index finger.
    Enrolling(Enroll),
    /// Removing it.
    Removing(Remove),
}

/// The application state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    /// What it is doing.
    pub activity: Activity,
    /// A reader was found (the service may still exit when idle; it is
    /// started again on the next call).
    pub connected: bool,
    /// Whether the right index finger is enrolled (`None`: not known yet).
    pub enrolled: Option<bool>,
    /// The last result or error, in one sentence.
    pub message: Option<Message>,
}

impl Default for Model {
    fn default() -> Self {
        Model::new()
    }
}

impl Model {
    /// The state before [`Input::Start`].
    pub fn new() -> Self {
        Model {
            activity: Activity::Loading { then_enroll: false },
            connected: false,
            enrolled: None,
            message: None,
        }
    }

    /// The status line (`None` while it is not known).
    pub fn status(&self) -> Option<Message> {
        self.enrolled.map(|e| {
            if e {
                msg("Right index finger registered")
            } else {
                msg("No fingerprint registered")
            }
        })
    }

    /// Whether "Register my right index finger" can be used now.
    pub fn can_enroll(&self) -> bool {
        self.activity == Activity::Idle && self.enrolled != Some(true)
    }

    /// Whether "Remove" is shown: the finger is enrolled.
    pub fn shows_remove(&self) -> bool {
        self.enrolled == Some(true)
    }

    /// Whether "Remove" can be used now.
    pub fn can_remove(&self) -> bool {
        self.activity == Activity::Idle && self.shows_remove()
    }

    /// Whether a call is in flight or the device is claimed.
    pub fn busy(&self) -> bool {
        self.activity != Activity::Idle
    }

    /// Whether the device is claimed, or may be: an enrolment or a removal
    /// is under way. Closing the window then cancels first and waits for the
    /// release.
    pub fn holds_reader(&self) -> bool {
        matches!(
            self.activity,
            Activity::Enrolling(_) | Activity::Removing(_)
        )
    }

    /// Applies `input`; returns the calls to make, in order.
    pub fn update(&mut self, input: Input) -> Vec<Command> {
        match input {
            Input::Start => {
                if self.holds_reader() {
                    return vec![];
                }
                self.activity = Activity::Loading { then_enroll: false };
                self.message = None;
                return vec![Command::Connect];
            }
            Input::Connected(r) => return self.connected(r),
            Input::Listed(r) => return self.listed(r),
            Input::ServiceLost => {
                self.service_lost();
                return vec![];
            }
            _ => {}
        }
        let activity = std::mem::replace(&mut self.activity, Activity::Idle);
        let (activity, cmds) = match activity {
            Activity::Idle => self.idle(input),
            Activity::Enrolling(e) => self.enroll(e, input),
            Activity::Removing(r) => self.removing(r, input),
            loading @ Activity::Loading { .. } => (loading, vec![]),
        };
        self.activity = activity;
        cmds
    }

    fn connected(&mut self, r: Result<(), ServiceError>) -> Vec<Command> {
        if !matches!(self.activity, Activity::Loading { .. }) {
            return vec![];
        }
        match r {
            Ok(()) => {
                self.connected = true;
                vec![Command::List]
            }
            Err(e) => {
                self.connected = false;
                self.activity = Activity::Idle;
                self.message = Some(e.message());
                vec![]
            }
        }
    }

    fn listed(&mut self, r: Result<Vec<String>, ServiceError>) -> Vec<Command> {
        let then_enroll = match self.activity {
            Activity::Loading { then_enroll } => then_enroll,
            // A refresh after an operation: the answer only updates the status.
            _ => {
                if let Ok(names) = r {
                    self.enrolled = Some(names.iter().any(|n| n == FINGER));
                }
                return vec![];
            }
        };
        match r {
            Ok(names) => {
                let enrolled = names.iter().any(|n| n == FINGER);
                self.enrolled = Some(enrolled);
                if then_enroll && !enrolled {
                    let (a, cmds) = start_enroll();
                    self.activity = a;
                    return cmds;
                }
            }
            Err(e) if e.kind == ErrorKind::NoPrints => self.enrolled = Some(false),
            Err(e) => self.message = Some(e.message()),
        }
        self.activity = Activity::Idle;
        vec![]
    }

    fn service_lost(&mut self) {
        let lost = ServiceError::new(ErrorKind::Unavailable, "the service left the bus").message();
        match &self.activity {
            // The claim went with the service: nothing to release.
            Activity::Enrolling(e) => {
                self.message = match &e.outcome {
                    Some(Outcome::Cancelled) => Some(msg("Registration cancelled.")),
                    // Completed stays completed: the print is stored.
                    Some(Outcome::Completed) => {
                        self.enrolled = Some(true);
                        Some(msg("Registration complete."))
                    }
                    Some(Outcome::Failed(m)) => Some(m.clone()),
                    None => Some(lost),
                };
                self.activity = Activity::Idle;
            }
            Activity::Removing(_) => {
                self.message = Some(lost);
                self.activity = Activity::Idle;
            }
            // Idle: fprintd exits when unused and is started again on demand.
            Activity::Idle | Activity::Loading { .. } => {}
        }
    }

    fn idle(&mut self, input: Input) -> (Activity, Vec<Command>) {
        match input {
            Input::Enroll if self.can_enroll() => {
                self.message = None;
                if !self.connected {
                    // No reader found so far: look again, then enrol.
                    return (
                        Activity::Loading { then_enroll: true },
                        vec![Command::Connect],
                    );
                }
                start_enroll()
            }
            Input::Remove if self.can_remove() && self.connected => {
                self.message = None;
                (
                    Activity::Removing(Remove {
                        all: false,
                        step: Step::Claiming,
                        result: None,
                    }),
                    vec![Command::Claim],
                )
            }
            _ => (Activity::Idle, vec![]),
        }
    }

    fn enroll(&mut self, mut e: Enroll, input: Input) -> (Activity, Vec<Command>) {
        let release = |mut e: Enroll| {
            e.step = Step::Releasing;
            (Activity::Enrolling(e), vec![Command::Release])
        };
        let stop = |mut e: Enroll| {
            e.step = Step::Stopping;
            (Activity::Enrolling(e), vec![Command::EnrollStop])
        };
        match (e.step, input) {
            (Step::Claiming, Input::Claimed(Ok(stages))) => {
                e.stages = stages;
                if e.cancel {
                    return release(e);
                }
                e.step = Step::Starting;
                (Activity::Enrolling(e), vec![Command::EnrollStart])
            }
            (Step::Claiming, Input::Claimed(Err(err))) => {
                if !e.cancel {
                    e.outcome = Some(Outcome::Failed(err.message()));
                }
                if may_hold_claim(&err) {
                    return release(e);
                }
                self.finish_enroll(e)
            }
            (Step::Starting, Input::EnrollStarted(Ok(()))) => {
                if e.cancel {
                    return stop(e);
                }
                e.step = Step::Scanning;
                e.instruction = status::enroll_first();
                (Activity::Enrolling(e), vec![])
            }
            (Step::Starting, Input::EnrollStarted(Err(err))) => {
                if !e.cancel {
                    e.outcome = Some(Outcome::Failed(err.message()));
                }
                release(e)
            }
            (Step::Starting | Step::Scanning, Input::EnrollStatus(result, done)) => {
                let r = EnrollResult::parse(&result);
                if !(done || r.is_final()) {
                    if r == EnrollResult::StagePassed {
                        e.passed += 1;
                        if let Some(n) = e.stages {
                            // The last stage is reported as "completed".
                            e.passed = e.passed.min(n.saturating_sub(1));
                        }
                    }
                    if let Some(i) = r.instruction(e.passed, e.stages) {
                        e.instruction = i;
                    }
                    return (Activity::Enrolling(e), vec![]);
                }
                if e.cancel {
                    // Still starting, and a stop is already decided: it goes
                    // out with the reply to EnrollStart.
                    return (Activity::Enrolling(e), vec![]);
                }
                e.outcome = Some(match r.failure() {
                    None if r == EnrollResult::Completed => {
                        if let Some(n) = e.stages {
                            e.passed = n;
                        }
                        Outcome::Completed
                    }
                    other => Outcome::Failed(other.unwrap_or_else(status::unknown_error)),
                });
                if r == EnrollResult::Disconnected {
                    // fprintd: nothing more should be done with the device but
                    // letting go of it.
                    return release(e);
                }
                if e.step == Step::Starting {
                    // The reply to EnrollStart is still to come; it will see
                    // the outcome and stop.
                    e.cancel = true;
                    return (Activity::Enrolling(e), vec![]);
                }
                stop(e)
            }
            (Step::Claiming | Step::Starting, Input::Cancel) => {
                e.cancel = true;
                if e.outcome.is_none() {
                    e.outcome = Some(Outcome::Cancelled);
                }
                (Activity::Enrolling(e), vec![])
            }
            (Step::Scanning, Input::Cancel) => {
                e.cancel = true;
                e.outcome = Some(Outcome::Cancelled);
                stop(e)
            }
            (Step::Stopping, Input::EnrollStopped(_)) => release(e),
            (Step::Releasing, Input::Released(_)) => self.finish_enroll(e),
            (_, _) => (Activity::Enrolling(e), vec![]),
        }
    }

    /// The enrolment is over and the device let go of: the outcome becomes
    /// the message.
    fn finish_enroll(&mut self, e: Enroll) -> (Activity, Vec<Command>) {
        match e.outcome {
            Some(Outcome::Completed) => {
                self.enrolled = Some(true);
                self.message = Some(msg("Registration complete."));
                (Activity::Idle, vec![Command::List])
            }
            Some(Outcome::Failed(m)) => {
                self.message = Some(m);
                (Activity::Idle, vec![])
            }
            Some(Outcome::Cancelled) | None => {
                self.message = Some(msg("Registration cancelled."));
                (Activity::Idle, vec![])
            }
        }
    }

    fn removing(&mut self, mut r: Remove, input: Input) -> (Activity, Vec<Command>) {
        let delete = |mut r: Remove| {
            r.step = Step::Starting;
            let cmd = if r.all {
                Command::DeleteAll
            } else {
                Command::DeleteFinger
            };
            (Activity::Removing(r), vec![cmd])
        };
        match (r.step, input) {
            (Step::Claiming, Input::Claimed(Ok(_))) => delete(r),
            (Step::Claiming, Input::Claimed(Err(err))) => {
                if may_hold_claim(&err) {
                    r.result = Some(Removed::Failed(err));
                    r.step = Step::Releasing;
                    return (Activity::Removing(r), vec![Command::Release]);
                }
                self.message = Some(err.message());
                (Activity::Idle, vec![])
            }
            (Step::Starting, Input::Deleted(res)) => match res {
                // An older service: only "remove all" exists.
                Err(e) if e.kind == ErrorKind::Unsupported && !r.all => {
                    r.all = true;
                    delete(r)
                }
                res => {
                    r.result = Some(match res {
                        Ok(()) => Removed::Yes,
                        Err(e) => Removed::Failed(e),
                    });
                    r.step = Step::Releasing;
                    (Activity::Removing(r), vec![Command::Release])
                }
            },
            (Step::Releasing, Input::Released(_)) => {
                match r.result {
                    Some(Removed::Yes) => {
                        self.enrolled = Some(false);
                        self.message = Some(msg("Fingerprint removed."));
                    }
                    Some(Removed::Failed(e)) => self.message = Some(e.message()),
                    None => {}
                }
                (Activity::Idle, vec![Command::List])
            }
            (_, _) => (Activity::Removing(r), vec![]),
        }
    }
}

fn start_enroll() -> (Activity, Vec<Command>) {
    (
        Activity::Enrolling(Enroll {
            stages: None,
            passed: 0,
            step: Step::Claiming,
            instruction: msg("Getting the reader ready…"),
            outcome: None,
            cancel: false,
        }),
        vec![Command::Claim],
    )
}

/// Whether a failed Claim may still have claimed the device: when it timed
/// out, the service may have claimed it after we stopped waiting. Releasing
/// then is harmless if it did not, and frees the reader if it did.
fn may_hold_claim(err: &ServiceError) -> bool {
    err.kind == ErrorKind::Timeout
}

#[cfg(test)]
mod tests;
