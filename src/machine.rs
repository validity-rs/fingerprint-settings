//! The application's state machine, free of any UI and of D-Bus itself.
//!
//! [`Model::update`] takes an [`Input`] (something the user did, or a reply or
//! signal from the fingerprint service) and returns the [`Command`]s to send
//! to the service. The model is the view state: the UI shows [`Model::page`],
//! [`Model::dialog`] and [`Model::fingers`] as they are.
//!
//! The service is used one call at a time: a command is only issued once the
//! input answering the previous one arrived. Inputs that do not fit the
//! current state (a late signal, a double click) are ignored.
//!
//! Every path that claimed the device releases it, whatever happens next
//! (cancel, failure, error), unless the service itself went away. A Claim
//! that timed out is released too: the service may have claimed the device
//! after the application stopped waiting.

use crate::error::{ErrorKind, ServiceError};
use crate::fingers::Finger;
use crate::message::{Arg, Message, msg};
use crate::status::{self, EnrollResult, ScanType, VerifyResult};

/// What the service says about its reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The product name.
    pub name: String,
    /// Press or swipe.
    pub scan: ScanType,
}

/// A call to make on the fingerprint service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Find the default reader (`Manager.GetDefaultDevice`) and read its
    /// properties; answered by [`Input::Connected`].
    Connect,
    /// `ListEnrolledFingers("")`; answered by [`Input::Listed`].
    List,
    /// `Claim("")`, then read `num-enroll-stages`; answered by [`Input::Claimed`].
    Claim,
    /// `Release()`; answered by [`Input::Released`].
    Release,
    /// `EnrollStart(finger)`; answered by [`Input::EnrollStarted`].
    EnrollStart(Finger),
    /// `EnrollStop()`; answered by [`Input::EnrollStopped`].
    EnrollStop,
    /// `VerifyStart("any")`; answered by [`Input::VerifyStarted`].
    VerifyStart,
    /// `VerifyStop()`; answered by [`Input::VerifyStopped`].
    VerifyStop,
    /// `DeleteEnrolledFinger(finger)`; answered by [`Input::Deleted`].
    DeleteFinger(Finger),
    /// `DeleteEnrolledFingers2()`; answered by [`Input::Deleted`].
    DeleteAll,
}

/// Something that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    // ---- the user
    /// Start, or look for the reader again.
    Start,
    /// "Add a finger".
    AddFinger,
    /// A finger picked in the list of fingers to add.
    ChooseFinger(Finger),
    /// "Test a finger".
    TestFinger,
    /// "Remove" on an enrolled finger: asks for confirmation.
    RemoveFinger(Finger),
    /// The dialog's main button (confirm, or "OK" on an error).
    Confirm,
    /// The dialog's cancel button, or the dialog was closed.
    Dismiss,
    /// "Cancel" during an operation, or "back" on a page that has one.
    Cancel,
    /// "Try again" after a failure or a test.
    TryAgain,

    // ---- the service
    /// The answer to [`Command::Connect`].
    Connected(Result<DeviceInfo, ServiceError>),
    /// The answer to [`Command::List`] (no enrolled prints is an empty list).
    Listed(Result<Vec<Finger>, ServiceError>),
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
    /// The answer to [`Command::VerifyStart`].
    VerifyStarted(Result<(), ServiceError>),
    /// The `VerifyFingerSelected` signal.
    VerifyFingerSelected(String),
    /// The `VerifyStatus` signal: result and done.
    VerifyStatus(String, bool),
    /// The answer to [`Command::VerifyStop`].
    VerifyStopped(Result<(), ServiceError>),
    /// The answer to [`Command::DeleteFinger`] or [`Command::DeleteAll`].
    Deleted(Result<(), ServiceError>),
    /// The service left the bus (it crashed, or was stopped).
    ServiceLost,
}

/// Where an operation on a claimed device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `Claim` sent.
    Claiming,
    /// `EnrollStart` / `VerifyStart` sent.
    Starting,
    /// Waiting for the finger.
    Scanning,
    /// `EnrollStop` / `VerifyStop` sent.
    Stopping,
    /// `Release` sent.
    Releasing,
    /// Over: the outcome is shown.
    Done,
}

/// Why something failed, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The short title.
    pub title: Message,
    /// What happened and what to do.
    pub description: Message,
    /// The technical detail, shown only on request.
    pub detail: Option<String>,
}

impl Failure {
    /// From a service error.
    pub fn service(e: &ServiceError) -> Self {
        Failure {
            title: e.title(),
            description: e.description(),
            detail: Some(e.detail.clone()),
        }
    }

    fn enroll(description: Message) -> Self {
        Failure {
            title: msg("The finger was not added"),
            description,
            detail: None,
        }
    }

    fn test(description: Message) -> Self {
        Failure {
            title: msg("The test did not work"),
            description,
            detail: None,
        }
    }
}

/// How an enrolment ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollOutcome {
    /// The finger is enrolled.
    Completed,
    /// The user cancelled.
    Cancelled,
    /// It failed. `duplicate`: the print exists already, so trying the same
    /// finger again is pointless.
    Failed {
        /// What to show.
        failure: Failure,
        /// The print is already enrolled.
        duplicate: bool,
    },
}

/// The enrolment page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enroll {
    /// The finger being added.
    pub finger: Finger,
    /// The number of good scans needed, when the device says.
    pub stages: Option<u32>,
    /// Good scans so far.
    pub passed: u32,
    /// Where it is.
    pub step: Step,
    /// What to tell the user now.
    pub instruction: Message,
    /// How it ended (set from the moment it is decided).
    pub outcome: Option<EnrollOutcome>,
    cancel: bool,
}

impl Enroll {
    /// Progress from 0 to 1 (`None`: the number of stages is unknown).
    pub fn fraction(&self) -> Option<f64> {
        if self.outcome == Some(EnrollOutcome::Completed) {
            return Some(1.0);
        }
        self.stages
            .map(|n| (f64::from(self.passed) / f64::from(n)).clamp(0.0, 1.0))
    }

    /// The progress in words: "Step 2 of 6", or the number of good scans
    /// when the device does not say how many it needs.
    pub fn progress_text(&self) -> Message {
        match self.stages {
            Some(total) => {
                let step = if self.outcome == Some(EnrollOutcome::Completed) {
                    total
                } else {
                    (self.passed + 1).min(total)
                };
                msg("Step {step} of {total}")
                    .with("step", Arg::Number(step))
                    .with("total", Arg::Number(total))
            }
            None => msg("Good scans so far: {count}").with("count", Arg::Number(self.passed)),
        }
    }

    /// The page title.
    pub fn title(&self) -> Message {
        msg("Adding your {finger}").with("finger", Arg::FingerInSentence(self.finger))
    }
}

/// How a test ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestOutcome {
    /// Recognised; the finger when the service said which one it checked.
    Match(Option<Finger>),
    /// Not recognised.
    NoMatch,
    /// The user cancelled.
    Cancelled,
    /// It failed.
    Failed(Failure),
}

impl TestOutcome {
    /// The result in words: title and explanation.
    pub fn texts(&self) -> Option<(Message, Message)> {
        Some(match self {
            TestOutcome::Match(Some(f)) => (
                msg("Recognised: {finger}").with("finger", Arg::Finger(*f)),
                msg("This finger can unlock the computer."),
            ),
            TestOutcome::Match(None) => (
                msg("Recognised"),
                msg("This is one of your registered fingers. It can unlock the computer."),
            ),
            TestOutcome::NoMatch => (
                msg("Not recognised"),
                msg("This finger is not registered, or it was not placed well. You can try again."),
            ),
            TestOutcome::Failed(f) => (f.title.clone(), f.description.clone()),
            TestOutcome::Cancelled => return None,
        })
    }
}

/// The test page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Test {
    /// Where it is.
    pub step: Step,
    /// The finger the service checks (`None`: any enrolled finger).
    pub selected: Option<Finger>,
    /// What to tell the user now.
    pub instruction: Message,
    /// How it ended.
    pub outcome: Option<TestOutcome>,
    cancel: bool,
}

/// What a removal ended with, before the release.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoveResult {
    Removed,
    /// The service lacks `DeleteEnrolledFinger`: only "remove all" is possible.
    NeedsAll,
    Failed(ServiceError),
}

/// The removal in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remove {
    /// The finger the user asked to remove.
    pub finger: Finger,
    /// Every print of the user is being removed (older services).
    pub all: bool,
    /// Where it is (`Claiming`, `Starting` = deleting, `Releasing`).
    pub step: Step,
    result: Option<RemoveResult>,
}

/// The page shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    /// Looking for the reader.
    Loading,
    /// The reader or the service cannot be used.
    Problem(Failure),
    /// The enrolled fingers (or the empty state).
    Fingers,
    /// Picking a finger to add.
    Choose,
    /// Adding a finger.
    Enroll(Enroll),
    /// Testing a finger.
    Test(Test),
    /// Removing a finger (the fingers page, busy).
    Removing(Remove),
}

/// A dialog over the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    /// "Remove this fingerprint?"
    ConfirmRemove(Finger),
    /// The service can only remove every print at once: "Remove all?"
    ConfirmRemoveAll(Finger),
    /// Something failed.
    Error(Failure),
}

impl Dialog {
    /// Title, body, confirm button label; the cancel button is "Cancel"
    /// (`None` for an error, which has only "OK").
    pub fn texts(&self) -> (Message, Message, Message, Option<Message>) {
        match self {
            Dialog::ConfirmRemove(f) => (
                msg("Remove this fingerprint?"),
                msg("Your {finger} will no longer unlock this computer. You can add it again later.")
                    .with("finger", Arg::FingerInSentence(*f)),
                msg("Remove"),
                Some(msg("Cancel")),
            ),
            Dialog::ConfirmRemoveAll(_) => (
                msg("Remove all your fingerprints?"),
                msg("This fingerprint service can only remove all your fingerprints at once, not a single one. You can add the others again afterwards."),
                msg("Remove all"),
                Some(msg("Cancel")),
            ),
            Dialog::Error(f) => (f.title.clone(), f.description.clone(), msg("OK"), None),
        }
    }
}

/// The application state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    /// The page shown.
    pub page: Page,
    /// A dialog over it.
    pub dialog: Option<Dialog>,
    /// The user's enrolled fingers, in [`Finger::ALL`] order.
    pub fingers: Vec<Finger>,
    /// The reader, once found.
    pub device: Option<DeviceInfo>,
    /// A short notice to show once ("Fingerprint removed"); the UI takes it.
    pub notice: Option<Message>,
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
            page: Page::Loading,
            dialog: None,
            fingers: Vec::new(),
            device: None,
            notice: None,
        }
    }

    fn scan(&self) -> ScanType {
        self.device.as_ref().map(|d| d.scan).unwrap_or_default()
    }

    /// Whether a call is in flight or the device is claimed: closing now
    /// should cancel first and wait for the release.
    pub fn busy(&self) -> bool {
        match &self.page {
            Page::Loading => true,
            Page::Enroll(e) => e.step != Step::Done,
            Page::Test(t) => t.step != Step::Done,
            Page::Removing(_) => true,
            Page::Problem(_) | Page::Fingers | Page::Choose => false,
        }
    }

    /// Whether the device is claimed, or may be: an enrolment, a test or a
    /// removal is under way. Closing the window then cancels first and waits
    /// for the release.
    pub fn holds_reader(&self) -> bool {
        match &self.page {
            Page::Enroll(e) => e.step != Step::Done,
            Page::Test(t) => t.step != Step::Done,
            Page::Removing(_) => true,
            Page::Loading | Page::Problem(_) | Page::Fingers | Page::Choose => false,
        }
    }

    /// Whether `finger` is enrolled.
    pub fn is_enrolled(&self, finger: Finger) -> bool {
        self.fingers.contains(&finger)
    }

    /// Takes the pending notice.
    pub fn take_notice(&mut self) -> Option<Message> {
        self.notice.take()
    }

    /// Applies `input`; returns the calls to make, in order.
    pub fn update(&mut self, input: Input) -> Vec<Command> {
        // Inputs that do not depend on the page.
        match input {
            Input::Start => {
                self.page = Page::Loading;
                self.dialog = None;
                return vec![Command::Connect];
            }
            Input::Connected(r) => return self.connected(r),
            Input::Listed(r) => return self.listed(r),
            Input::Dismiss => {
                self.dialog = None;
                return vec![];
            }
            Input::Confirm if self.dialog.is_some() => return self.confirm(),
            Input::ServiceLost => return self.service_lost(),
            _ => {}
        }
        let page = std::mem::replace(&mut self.page, Page::Loading);
        let (page, cmds) = match page {
            Page::Enroll(e) => self.enroll(e, input),
            Page::Test(t) => self.test(t, input),
            Page::Removing(r) => self.removing(r, input),
            Page::Fingers => self.fingers_page(input),
            Page::Choose => match input {
                Input::ChooseFinger(f) if !self.is_enrolled(f) => self.start_enroll(f),
                Input::Cancel => (Page::Fingers, vec![]),
                _ => (Page::Choose, vec![]),
            },
            Page::Problem(p) => match input {
                Input::TryAgain => (Page::Loading, vec![Command::Connect]),
                _ => (Page::Problem(p), vec![]),
            },
            Page::Loading => (Page::Loading, vec![]),
        };
        self.page = page;
        cmds
    }

    fn connected(&mut self, r: Result<DeviceInfo, ServiceError>) -> Vec<Command> {
        if self.page != Page::Loading {
            return vec![];
        }
        match r {
            Ok(info) => {
                self.device = Some(info);
                vec![Command::List]
            }
            Err(e) => {
                self.device = None;
                self.page = Page::Problem(Failure::service(&e));
                vec![]
            }
        }
    }

    fn listed(&mut self, r: Result<Vec<Finger>, ServiceError>) -> Vec<Command> {
        match r {
            Ok(mut v) => {
                v.sort();
                v.dedup();
                self.fingers = v;
            }
            Err(e) if e.kind == ErrorKind::NoPrints => self.fingers.clear(),
            Err(e) => {
                // A failed list during an operation changes nothing there.
                if matches!(self.page, Page::Loading | Page::Fingers | Page::Choose) {
                    self.page = Page::Problem(Failure::service(&e));
                }
                return vec![];
            }
        }
        if self.page == Page::Loading {
            self.page = Page::Fingers;
        }
        vec![]
    }

    fn confirm(&mut self) -> Vec<Command> {
        // Only from the fingers page: nothing else is running.
        let dialog = self.dialog.take();
        if self.page != Page::Fingers {
            return vec![];
        }
        let (finger, all) = match dialog {
            Some(Dialog::ConfirmRemove(f)) => (f, false),
            Some(Dialog::ConfirmRemoveAll(f)) => (f, true),
            _ => return vec![],
        };
        self.page = Page::Removing(Remove {
            finger,
            all,
            step: Step::Claiming,
            result: None,
        });
        vec![Command::Claim]
    }

    fn service_lost(&mut self) -> Vec<Command> {
        let lost = ServiceError::new(ErrorKind::Unavailable, "the service left the bus");
        match &mut self.page {
            // The claim went with the service: nothing to release.
            Page::Enroll(e) if e.step != Step::Done => {
                e.step = Step::Done;
                match e.outcome {
                    Some(EnrollOutcome::Cancelled) => self.page = Page::Fingers,
                    // Completed stays completed: the print is stored.
                    Some(_) => {}
                    None => {
                        e.outcome = Some(EnrollOutcome::Failed {
                            failure: Failure::service(&lost),
                            duplicate: false,
                        });
                    }
                }
            }
            Page::Test(t) if t.step != Step::Done => {
                t.step = Step::Done;
                match t.outcome {
                    Some(TestOutcome::Cancelled) => self.page = Page::Fingers,
                    Some(_) => {}
                    None => t.outcome = Some(TestOutcome::Failed(Failure::service(&lost))),
                }
            }
            Page::Removing(_) => {
                self.page = Page::Fingers;
                self.dialog = Some(Dialog::Error(Failure::service(&lost)));
            }
            // Idle: fprintd exits when unused and is started again on demand.
            _ => {}
        }
        vec![]
    }

    fn fingers_page(&mut self, input: Input) -> (Page, Vec<Command>) {
        if self.dialog.is_some() {
            return (Page::Fingers, vec![]);
        }
        match input {
            Input::AddFinger if self.fingers.len() < Finger::ALL.len() => (Page::Choose, vec![]),
            Input::TestFinger if !self.fingers.is_empty() => self.start_test(),
            Input::RemoveFinger(f) if self.is_enrolled(f) => {
                self.dialog = Some(Dialog::ConfirmRemove(f));
                (Page::Fingers, vec![])
            }
            _ => (Page::Fingers, vec![]),
        }
    }

    fn start_enroll(&self, finger: Finger) -> (Page, Vec<Command>) {
        (
            Page::Enroll(Enroll {
                finger,
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

    fn start_test(&self) -> (Page, Vec<Command>) {
        (
            Page::Test(Test {
                step: Step::Claiming,
                selected: None,
                instruction: msg("Getting the reader ready…"),
                outcome: None,
                cancel: false,
            }),
            vec![Command::Claim],
        )
    }

    fn enroll(&mut self, mut e: Enroll, input: Input) -> (Page, Vec<Command>) {
        let scan = self.scan();
        let release = |mut e: Enroll| {
            e.step = Step::Releasing;
            (Page::Enroll(e), vec![Command::Release])
        };
        let stop = |mut e: Enroll| {
            e.step = Step::Stopping;
            (Page::Enroll(e), vec![Command::EnrollStop])
        };
        match (e.step, input) {
            (Step::Claiming, Input::Claimed(Ok(stages))) => {
                e.stages = stages;
                if e.cancel {
                    return release(e);
                }
                e.step = Step::Starting;
                let finger = e.finger;
                (Page::Enroll(e), vec![Command::EnrollStart(finger)])
            }
            (Step::Claiming, Input::Claimed(Err(err))) => {
                if !e.cancel {
                    e.outcome = Some(enroll_service_failure(&err));
                }
                if may_hold_claim(&err) {
                    return release(e);
                }
                e.step = Step::Done;
                if e.cancel {
                    return (Page::Fingers, vec![]);
                }
                (Page::Enroll(e), vec![])
            }
            (Step::Starting, Input::EnrollStarted(Ok(()))) => {
                if e.cancel {
                    return stop(e);
                }
                e.step = Step::Scanning;
                e.instruction = status::enroll_first(e.finger, scan);
                (Page::Enroll(e), vec![])
            }
            (Step::Starting, Input::EnrollStarted(Err(err))) => {
                if !e.cancel {
                    e.outcome = Some(enroll_service_failure(&err));
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
                    if let Some(i) = r.instruction(scan) {
                        e.instruction = i;
                    }
                    return (Page::Enroll(e), vec![]);
                }
                if e.cancel {
                    // Still starting, and a stop is already decided: it goes
                    // out with the reply to EnrollStart.
                    return (Page::Enroll(e), vec![]);
                }
                e.outcome = Some(match r {
                    EnrollResult::Completed => {
                        if let Some(n) = e.stages {
                            e.passed = n;
                        }
                        EnrollOutcome::Completed
                    }
                    ref other => EnrollOutcome::Failed {
                        failure: Failure::enroll(
                            other.failure().unwrap_or_else(status::unknown_error),
                        ),
                        duplicate: *other == EnrollResult::Duplicate,
                    },
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
                    return (Page::Enroll(e), vec![]);
                }
                stop(e)
            }
            (Step::Claiming | Step::Starting, Input::Cancel) => {
                e.cancel = true;
                if e.outcome.is_none() {
                    e.outcome = Some(EnrollOutcome::Cancelled);
                }
                (Page::Enroll(e), vec![])
            }
            (Step::Scanning, Input::Cancel) => {
                e.cancel = true;
                e.outcome = Some(EnrollOutcome::Cancelled);
                stop(e)
            }
            (Step::Stopping, Input::EnrollStopped(_)) => release(e),
            (Step::Releasing, Input::Released(_)) => {
                e.step = Step::Done;
                match e.outcome {
                    Some(EnrollOutcome::Completed) => {
                        if !self.fingers.contains(&e.finger) {
                            self.fingers.push(e.finger);
                            self.fingers.sort();
                        }
                        (Page::Enroll(e), vec![Command::List])
                    }
                    Some(EnrollOutcome::Cancelled) | None => (Page::Fingers, vec![Command::List]),
                    Some(EnrollOutcome::Failed { .. }) => (Page::Enroll(e), vec![]),
                }
            }
            (Step::Done, Input::Cancel) => (Page::Fingers, vec![]),
            (Step::Done, Input::TryAgain) => match e.outcome {
                Some(EnrollOutcome::Failed {
                    duplicate: false, ..
                }) if !self.is_enrolled(e.finger) => self.start_enroll(e.finger),
                _ => (Page::Enroll(e), vec![]),
            },
            (Step::Done, Input::AddFinger) if self.fingers.len() < Finger::ALL.len() => {
                (Page::Choose, vec![])
            }
            (Step::Done, Input::TestFinger) if !self.fingers.is_empty() => self.start_test(),
            (_, _) => (Page::Enroll(e), vec![]),
        }
    }

    fn test(&mut self, mut t: Test, input: Input) -> (Page, Vec<Command>) {
        let scan = self.scan();
        let release = |mut t: Test| {
            t.step = Step::Releasing;
            (Page::Test(t), vec![Command::Release])
        };
        let stop = |mut t: Test| {
            t.step = Step::Stopping;
            (Page::Test(t), vec![Command::VerifyStop])
        };
        match (t.step, input) {
            (Step::Claiming, Input::Claimed(Ok(_))) => {
                if t.cancel {
                    return release(t);
                }
                t.step = Step::Starting;
                (Page::Test(t), vec![Command::VerifyStart])
            }
            (Step::Claiming, Input::Claimed(Err(err))) => {
                if !t.cancel {
                    t.outcome = Some(TestOutcome::Failed(Failure::service(&err)));
                }
                if may_hold_claim(&err) {
                    return release(t);
                }
                t.step = Step::Done;
                if t.cancel {
                    return (Page::Fingers, vec![]);
                }
                (Page::Test(t), vec![])
            }
            (Step::Starting, Input::VerifyStarted(Ok(()))) => {
                if t.cancel {
                    return stop(t);
                }
                t.step = Step::Scanning;
                t.instruction = status::verify_first(t.selected, scan);
                (Page::Test(t), vec![])
            }
            (Step::Starting, Input::VerifyStarted(Err(err))) => {
                if !t.cancel {
                    t.outcome = Some(TestOutcome::Failed(Failure::service(&err)));
                }
                release(t)
            }
            (Step::Starting | Step::Scanning, Input::VerifyFingerSelected(name)) => {
                t.selected = Finger::from_fprintd(&name);
                if t.step == Step::Scanning {
                    t.instruction = status::verify_first(t.selected, scan);
                }
                (Page::Test(t), vec![])
            }
            (Step::Starting | Step::Scanning, Input::VerifyStatus(result, done)) => {
                let r = VerifyResult::parse(&result);
                if !(done || r.is_final()) {
                    if let Some(i) = r.instruction(scan) {
                        t.instruction = i;
                    }
                    return (Page::Test(t), vec![]);
                }
                if t.cancel {
                    // Still starting, and a stop is already decided: it goes
                    // out with the reply to VerifyStart.
                    return (Page::Test(t), vec![]);
                }
                t.outcome = Some(match r {
                    VerifyResult::Match => TestOutcome::Match(t.selected),
                    VerifyResult::NoMatch => TestOutcome::NoMatch,
                    VerifyResult::Disconnected => {
                        TestOutcome::Failed(Failure::test(status::disconnected()))
                    }
                    _ => TestOutcome::Failed(Failure::test(status::unknown_error())),
                });
                if r == VerifyResult::Disconnected {
                    return release(t);
                }
                if t.step == Step::Starting {
                    t.cancel = true;
                    return (Page::Test(t), vec![]);
                }
                stop(t)
            }
            (Step::Claiming | Step::Starting, Input::Cancel) => {
                t.cancel = true;
                if t.outcome.is_none() {
                    t.outcome = Some(TestOutcome::Cancelled);
                }
                (Page::Test(t), vec![])
            }
            (Step::Scanning, Input::Cancel) => {
                t.cancel = true;
                t.outcome = Some(TestOutcome::Cancelled);
                stop(t)
            }
            (Step::Stopping, Input::VerifyStopped(_)) => release(t),
            (Step::Releasing, Input::Released(_)) => {
                t.step = Step::Done;
                match t.outcome {
                    Some(TestOutcome::Cancelled) | None => (Page::Fingers, vec![]),
                    _ => (Page::Test(t), vec![]),
                }
            }
            (Step::Done, Input::Cancel) => (Page::Fingers, vec![]),
            (Step::Done, Input::TryAgain) => self.start_test(),
            (_, _) => (Page::Test(t), vec![]),
        }
    }

    fn removing(&mut self, mut r: Remove, input: Input) -> (Page, Vec<Command>) {
        match (r.step, input) {
            (Step::Claiming, Input::Claimed(Ok(_))) => {
                r.step = Step::Starting;
                let cmd = if r.all {
                    Command::DeleteAll
                } else {
                    Command::DeleteFinger(r.finger)
                };
                (Page::Removing(r), vec![cmd])
            }
            (Step::Claiming, Input::Claimed(Err(err))) => {
                if may_hold_claim(&err) {
                    r.result = Some(RemoveResult::Failed(err));
                    r.step = Step::Releasing;
                    return (Page::Removing(r), vec![Command::Release]);
                }
                self.dialog = Some(Dialog::Error(Failure::service(&err)));
                (Page::Fingers, vec![])
            }
            (Step::Starting, Input::Deleted(res)) => {
                r.result = Some(match res {
                    Ok(()) => RemoveResult::Removed,
                    Err(e) if e.kind == ErrorKind::Unsupported && !r.all => RemoveResult::NeedsAll,
                    Err(e) => RemoveResult::Failed(e),
                });
                r.step = Step::Releasing;
                (Page::Removing(r), vec![Command::Release])
            }
            (Step::Releasing, Input::Released(_)) => match r.result {
                Some(RemoveResult::Removed) => {
                    if r.all {
                        self.fingers.clear();
                        self.notice = Some(msg("All your fingerprints were removed."));
                    } else {
                        self.fingers.retain(|f| *f != r.finger);
                        self.notice = Some(
                            msg("Fingerprint removed: {finger}")
                                .with("finger", Arg::Finger(r.finger)),
                        );
                    }
                    (Page::Fingers, vec![Command::List])
                }
                Some(RemoveResult::NeedsAll) => {
                    self.dialog = Some(Dialog::ConfirmRemoveAll(r.finger));
                    (Page::Fingers, vec![])
                }
                Some(RemoveResult::Failed(e)) => {
                    self.dialog = Some(Dialog::Error(Failure::service(&e)));
                    (Page::Fingers, vec![Command::List])
                }
                None => (Page::Fingers, vec![Command::List]),
            },
            (_, _) => (Page::Removing(r), vec![]),
        }
    }
}

/// Whether a failed Claim may still have claimed the device: when it timed
/// out, the service may have claimed it after we stopped waiting. Releasing
/// then is harmless if it did not, and frees the reader if it did.
fn may_hold_claim(err: &ServiceError) -> bool {
    err.kind == ErrorKind::Timeout
}

fn enroll_service_failure(err: &ServiceError) -> EnrollOutcome {
    EnrollOutcome::Failed {
        failure: Failure::service(err),
        duplicate: err.kind == ErrorKind::AlreadyEnrolled,
    }
}

#[cfg(test)]
mod tests;
