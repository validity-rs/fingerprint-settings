use super::*;
use crate::i18n::Catalogue;

/// "Start" on the add page with this finger and its standard name.
fn pick(f: Finger) -> Input {
    Input::StartEnroll {
        finger: Some(f),
        label: None,
    }
}

fn err(kind: ErrorKind) -> ServiceError {
    ServiceError::new(kind, format!("{kind:?} detail"))
}

fn press() -> DeviceInfo {
    DeviceInfo {
        name: "Reader".into(),
        scan: ScanType::Press,
    }
}

fn en(m: &Message) -> String {
    m.render(&Catalogue::default())
}

/// A model on the fingers page with these fingers enrolled.
fn ready(fingers: &[Finger]) -> Model {
    ready_with(fingers, press())
}

fn ready_with(fingers: &[Finger], dev: DeviceInfo) -> Model {
    let mut m = Model::new();
    assert_eq!(m.update(Input::Start), [Command::Connect]);
    assert_eq!(m.update(Input::Connected(Ok(dev))), [Command::List]);
    assert_eq!(m.update(Input::Listed(Ok(fingers.to_vec()))), []);
    assert_eq!(m.page, Page::Fingers);
    m
}

fn enroll(m: &Model) -> &Enroll {
    match &m.page {
        Page::Enroll(e) => e,
        p => panic!("not enrolling: {p:?}"),
    }
}

fn test(m: &Model) -> &Test {
    match &m.page {
        Page::Test(t) => t,
        p => panic!("not testing: {p:?}"),
    }
}

/// Model enrolling `f`, scanning, with `stages`.
fn scanning(f: Finger, stages: Option<u32>) -> Model {
    let mut m = ready(&[]);
    assert_eq!(m.update(Input::AddFinger), []);
    assert_eq!(m.page, Page::Add);
    assert_eq!(m.update(pick(f)), [Command::Claim]);
    assert_eq!(enroll(&m).step, Step::Claiming);
    assert_eq!(
        m.update(Input::Claimed(Ok(stages))),
        [Command::EnrollStart(f)]
    );
    assert_eq!(enroll(&m).step, Step::Starting);
    assert_eq!(m.update(Input::EnrollStarted(Ok(()))), []);
    assert_eq!(enroll(&m).step, Step::Scanning);
    m
}

/// Model testing, scanning.
fn verifying(fingers: &[Finger]) -> Model {
    let mut m = ready(fingers);
    assert_eq!(m.update(Input::TestFinger), [Command::Claim]);
    assert_eq!(test(&m).step, Step::Claiming);
    assert_eq!(
        m.update(Input::Claimed(Ok(Some(6)))),
        [Command::VerifyStart]
    );
    assert_eq!(test(&m).step, Step::Starting);
    assert_eq!(m.update(Input::VerifyStarted(Ok(()))), []);
    assert_eq!(test(&m).step, Step::Scanning);
    m
}

fn status(s: &str, done: bool) -> Input {
    Input::EnrollStatus(s.into(), done)
}

fn vstatus(s: &str, done: bool) -> Input {
    Input::VerifyStatus(s.into(), done)
}

// ------------------------------------------------------------ start

#[test]
fn start_connects_then_lists() {
    let mut m = Model::new();
    assert_eq!(m, Model::default());
    assert_eq!(m.page, Page::Loading);
    assert!(m.busy());
    assert_eq!(m.update(Input::Start), [Command::Connect]);
    assert_eq!(m.update(Input::Connected(Ok(press()))), [Command::List]);
    assert_eq!(m.device, Some(press()));
    assert_eq!(m.page, Page::Loading);
    assert_eq!(
        m.update(Input::Listed(Ok(vec![
            Finger::RightThumb,
            Finger::LeftThumb,
            Finger::LeftThumb
        ]))),
        []
    );
    assert_eq!(m.page, Page::Fingers);
    assert_eq!(m.fingers, [Finger::LeftThumb, Finger::RightThumb]);
    assert!(!m.busy());
    // Start again from anywhere: back to loading.
    assert_eq!(m.update(Input::Start), [Command::Connect]);
    assert_eq!(m.page, Page::Loading);
}

#[test]
fn every_connect_error_is_a_problem_page_with_retry() {
    for kind in ErrorKind::ALL {
        let mut m = Model::new();
        m.update(Input::Start);
        assert_eq!(m.update(Input::Connected(Err(err(kind)))), []);
        match &m.page {
            Page::Problem(f) => {
                assert_eq!(f.title, kind.title());
                assert_eq!(f.description, kind.description());
                assert_eq!(
                    f.detail.as_deref(),
                    Some(format!("{kind:?} detail").as_str())
                );
            }
            p => panic!("{kind:?}: {p:?}"),
        }
        assert!(m.device.is_none());
        assert!(!m.busy());
        // Nothing but "try again" does anything.
        for i in [
            Input::AddFinger,
            Input::TestFinger,
            Input::Cancel,
            Input::RemoveFinger(Finger::LeftThumb),
            pick(Finger::LeftThumb),
        ] {
            assert_eq!(m.update(i), []);
            assert!(matches!(m.page, Page::Problem(_)));
        }
        assert_eq!(m.update(Input::TryAgain), [Command::Connect]);
        assert_eq!(m.page, Page::Loading);
        assert_eq!(m.update(Input::Connected(Ok(press()))), [Command::List]);
    }
}

#[test]
fn list_results() {
    // No enrolled prints is an empty list, not a problem.
    let mut m = Model::new();
    m.update(Input::Start);
    m.update(Input::Connected(Ok(press())));
    assert_eq!(m.update(Input::Listed(Err(err(ErrorKind::NoPrints)))), []);
    assert_eq!(m.page, Page::Fingers);
    assert!(m.fingers.is_empty());

    // Any other error on loading is a problem page.
    for kind in ErrorKind::ALL
        .into_iter()
        .filter(|k| *k != ErrorKind::NoPrints)
    {
        let mut m = Model::new();
        m.update(Input::Start);
        m.update(Input::Connected(Ok(press())));
        m.update(Input::Listed(Err(err(kind))));
        assert!(matches!(m.page, Page::Problem(_)), "{kind:?}");
    }

    // A refresh on the fingers page replaces the list; an error there too is
    // a problem page.
    let mut m = ready(&[Finger::LeftThumb]);
    m.update(Input::Listed(Ok(vec![Finger::RightIndex])));
    assert_eq!(m.fingers, [Finger::RightIndex]);
    m.update(Input::Listed(Err(err(ErrorKind::PermissionDenied))));
    assert!(matches!(m.page, Page::Problem(_)));

    // During an operation a failed list changes nothing.
    let mut m = scanning(Finger::LeftThumb, Some(6));
    let before = m.page.clone();
    m.update(Input::Listed(Err(err(ErrorKind::Failed))));
    assert_eq!(m.page, before);
}

#[test]
fn a_late_connect_answer_is_ignored() {
    let mut m = ready(&[]);
    assert_eq!(m.update(Input::Connected(Ok(press()))), []);
    assert_eq!(m.page, Page::Fingers);
    assert_eq!(
        m.update(Input::Connected(Err(err(ErrorKind::NoReader)))),
        []
    );
    assert_eq!(m.page, Page::Fingers);
    assert_eq!(m.device, Some(press()));
}

// ------------------------------------------------------------ the fingers page

#[test]
fn fingers_page_inputs() {
    let mut m = ready(&[]);
    // Nothing to test or remove without fingers.
    assert_eq!(m.update(Input::TestFinger), []);
    assert_eq!(m.update(Input::RemoveFinger(Finger::LeftThumb)), []);
    assert!(m.dialog.is_none());
    // Service inputs that do not belong here.
    for i in [
        Input::Claimed(Ok(None)),
        Input::Released(Ok(())),
        status("enroll-completed", true),
        vstatus("verify-match", true),
        Input::VerifyFingerSelected("any".into()),
        Input::Deleted(Ok(())),
        Input::EnrollStarted(Ok(())),
        Input::VerifyStopped(Ok(())),
        Input::EnrollStopped(Ok(())),
        Input::VerifyStarted(Ok(())),
        Input::TryAgain,
        Input::Cancel,
        Input::Confirm,
        pick(Finger::LeftThumb),
    ] {
        assert_eq!(m.update(i.clone()), [], "{i:?}");
        assert_eq!(m.page, Page::Fingers, "{i:?}");
    }
    // All ten enrolled: nothing left to add.
    let mut m = ready(&Finger::ALL);
    assert_eq!(m.update(Input::AddFinger), []);
    assert_eq!(m.page, Page::Fingers);
}

#[test]
fn chooser_offers_only_new_fingers() {
    let mut m = ready(&[Finger::RightIndex]);
    m.update(Input::AddFinger);
    assert_eq!(m.page, Page::Add);
    assert!(!m.busy());
    assert_eq!(m.update(pick(Finger::RightIndex)), []);
    assert_eq!(m.page, Page::Add);
    for i in [Input::TestFinger, Input::Claimed(Ok(None)), Input::TryAgain] {
        assert_eq!(m.update(i), []);
        assert_eq!(m.page, Page::Add);
    }
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(m.page, Page::Fingers);
    m.update(Input::AddFinger);
    assert_eq!(m.update(pick(Finger::LeftIndex)), [Command::Claim]);
    let e = enroll(&m);
    assert_eq!(
        (e.slot, e.finger),
        (Finger::LeftIndex, Some(Finger::LeftIndex))
    );
    assert_eq!(en(&e.instruction), "Getting the reader ready…");
    assert_eq!(en(&e.title()), "Adding \u{201c}Left index finger\u{201d}");
    assert!(m.busy());
}

// ------------------------------------------------------------ enrolment

#[test]
fn enrol_happy_path() {
    let mut m = scanning(Finger::RightIndex, Some(6));
    let e = enroll(&m);
    assert_eq!(
        en(&e.instruction),
        "Place your right index finger flat on the reader."
    );
    assert_eq!(en(&e.progress_text()), "Step 1 of 6");
    assert_eq!(e.fraction(), Some(0.0));
    for n in 1..=5u32 {
        assert_eq!(m.update(status("enroll-stage-passed", false)), []);
        let e = enroll(&m);
        assert_eq!(e.passed, n);
        assert_eq!(en(&e.progress_text()), format!("Step {} of 6", n + 1));
        assert_eq!(e.fraction(), Some(f64::from(n) / 6.0));
        assert_eq!(
            en(&e.instruction),
            "Good. Lift your finger, then place it on the reader again."
        );
    }
    // Extra stage reports do not overflow the count.
    m.update(status("enroll-stage-passed", false));
    assert_eq!(enroll(&m).passed, 5);
    assert_eq!(
        m.update(status("enroll-completed", true)),
        [Command::EnrollStop]
    );
    let e = enroll(&m);
    assert_eq!(e.step, Step::Stopping);
    assert_eq!(e.outcome, Some(EnrollOutcome::Completed));
    assert_eq!(e.fraction(), Some(1.0));
    assert_eq!(en(&e.progress_text()), "Step 6 of 6");
    // Cancel now does nothing: it is over.
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(m.update(Input::EnrollStopped(Ok(()))), [Command::Release]);
    assert_eq!(enroll(&m).step, Step::Releasing);
    assert!(m.busy());
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(enroll(&m).step, Step::Done);
    assert!(!m.busy());
    assert_eq!(m.fingers, [Finger::RightIndex]);
    // The list refresh does not leave the done page.
    m.update(Input::Listed(Ok(vec![Finger::RightIndex])));
    assert_eq!(enroll(&m).outcome, Some(EnrollOutcome::Completed));
    // Done: back, test, add another.
    let mut back = m.clone();
    assert_eq!(back.update(Input::Cancel), []);
    assert_eq!(back.page, Page::Fingers);
    let mut t = m.clone();
    assert_eq!(t.update(Input::TestFinger), [Command::Claim]);
    assert!(matches!(t.page, Page::Test(_)));
    let mut a = m.clone();
    assert_eq!(a.update(Input::AddFinger), []);
    assert_eq!(a.page, Page::Add);
    // "Try again" after a success does nothing.
    assert_eq!(m.update(Input::TryAgain), []);
    assert_eq!(enroll(&m).step, Step::Done);
}

#[test]
fn enrol_swipe_wording() {
    let mut m = ready_with(
        &[],
        DeviceInfo {
            name: "S".into(),
            scan: ScanType::Swipe,
        },
    );
    m.update(Input::AddFinger);
    m.update(pick(Finger::LeftThumb));
    m.update(Input::Claimed(Ok(Some(5))));
    m.update(Input::EnrollStarted(Ok(())));
    assert_eq!(
        en(&enroll(&m).instruction),
        "Slide your left thumb over the reader."
    );
    m.update(status("enroll-stage-passed", false));
    assert_eq!(
        en(&enroll(&m).instruction),
        "Good. Slide your finger over the reader again."
    );
    m.update(status("enroll-too-fast", false));
    assert_eq!(
        en(&enroll(&m).instruction),
        "Too quick. Slide your finger more slowly."
    );
}

#[test]
fn every_ongoing_enroll_status_keeps_scanning() {
    for (s, expected) in [
        (
            "enroll-retry-scan",
            "That did not work. Lift your finger, then place it on the reader again.",
        ),
        (
            "enroll-swipe-too-short",
            "The swipe was too short. Slide your whole finger slowly over the reader.",
        ),
        (
            "enroll-too-fast",
            "Too quick. Leave your finger on the reader a little longer.",
        ),
        (
            "enroll-finger-not-centered",
            "Your finger was not in the middle of the reader. Place it in the middle and try again.",
        ),
        (
            "enroll-remove-and-retry",
            "Lift your finger completely off the reader, then place it again.",
        ),
        (
            "enroll-from-the-future",
            "That did not work. Lift your finger, then place it on the reader again.",
        ),
    ] {
        let mut m = scanning(Finger::LeftThumb, Some(6));
        m.update(status("enroll-stage-passed", false));
        assert_eq!(m.update(status(s, false)), [], "{s}");
        let e = enroll(&m);
        assert_eq!(e.step, Step::Scanning, "{s}");
        assert_eq!(e.passed, 1, "{s}: progress is kept");
        assert_eq!(en(&e.instruction), expected, "{s}");
        assert!(e.outcome.is_none());
    }
}

/// Runs an enrolment to its end with final status `s`; returns the model on
/// the done page and the commands issued after the status.
fn enrol_ending(s: &str, done: bool) -> (Model, Vec<Command>) {
    let mut m = scanning(Finger::LeftThumb, Some(6));
    let mut cmds = m.update(status(s, done));
    if cmds == [Command::EnrollStop] {
        let c = m.update(Input::EnrollStopped(Ok(())));
        assert_eq!(c, [Command::Release]);
        cmds.extend(c);
    }
    assert_eq!(*cmds.last().unwrap(), Command::Release, "{s}");
    cmds.extend(m.update(Input::Released(Ok(()))));
    (m, cmds)
}

#[test]
fn every_final_enroll_status() {
    let cases: [(&str, bool, &str, bool); 7] = [
        (
            "enroll-failed",
            true,
            "Adding this finger did not work. Please try again.",
            false,
        ),
        (
            "enroll-data-full",
            true,
            "The reader's memory is full. Remove a fingerprint you no longer use, then try again.",
            false,
        ),
        (
            "enroll-duplicate",
            true,
            "This fingerprint is already registered on this computer, perhaps as another finger or for another person. Choose a different finger.",
            true,
        ),
        (
            "enroll-unknown-error",
            true,
            "The fingerprint reader had a problem. Please try again. If it happens again, restart the computer.",
            false,
        ),
        (
            "enroll-disconnected",
            true,
            "The fingerprint reader was disconnected. Check that it is plugged in, then try again.",
            false,
        ),
        // A final result without the done flag still ends it.
        (
            "enroll-failed",
            false,
            "Adding this finger did not work. Please try again.",
            false,
        ),
        // done with a result that is not final, or unknown: a failure.
        (
            "enroll-retry-scan",
            true,
            "The fingerprint reader had a problem. Please try again. If it happens again, restart the computer.",
            false,
        ),
    ];
    for (s, done, why, dup) in cases {
        let (m, cmds) = enrol_ending(s, done);
        // Disconnected: no stop, straight to release.
        if s == "enroll-disconnected" {
            assert_eq!(cmds, [Command::Release], "{s}");
        } else {
            assert_eq!(cmds, [Command::EnrollStop, Command::Release], "{s}");
        }
        let e = enroll(&m);
        assert_eq!(e.step, Step::Done, "{s}");
        match &e.outcome {
            Some(EnrollOutcome::Failed { failure, duplicate }) => {
                assert_eq!(en(&failure.title), "The finger was not added");
                assert_eq!(en(&failure.description), why, "{s}");
                assert_eq!(*duplicate, dup, "{s}");
                assert!(failure.detail.is_none());
            }
            o => panic!("{s}: {o:?}"),
        }
        assert!(m.fingers.is_empty());
        // Try again: the same finger, unless it is a duplicate.
        let mut again = m.clone();
        let c = again.update(Input::TryAgain);
        if dup {
            assert_eq!(c, [], "{s}");
            assert_eq!(again.page, m.page);
        } else {
            assert_eq!(c, [Command::Claim], "{s}");
            assert_eq!(enroll(&again).step, Step::Claiming);
            assert_eq!(enroll(&again).slot, Finger::LeftThumb);
            assert_eq!(enroll(&again).finger, Some(Finger::LeftThumb));
            assert!(enroll(&again).outcome.is_none());
        }
        let mut other = m.clone();
        assert_eq!(other.update(Input::AddFinger), []);
        assert_eq!(other.page, Page::Add);
        // No fingers: nothing to test from here.
        let mut t = m.clone();
        assert_eq!(t.update(Input::TestFinger), []);
    }
}

#[test]
fn completed_without_done_flag_and_unknown_stage_count() {
    let mut m = scanning(Finger::LeftRing, None);
    let e = enroll(&m);
    assert_eq!(e.fraction(), None);
    assert_eq!(en(&e.progress_text()), "Good scans so far: 0");
    m.update(status("enroll-stage-passed", false));
    m.update(status("enroll-stage-passed", false));
    m.update(status("enroll-stage-passed", false));
    assert_eq!(enroll(&m).passed, 3);
    assert_eq!(en(&enroll(&m).progress_text()), "Good scans so far: 3");
    assert_eq!(
        m.update(status("enroll-completed", false)),
        [Command::EnrollStop]
    );
    assert_eq!(enroll(&m).fraction(), Some(1.0));
    m.update(Input::EnrollStopped(Err(err(ErrorKind::Failed))));
    assert_eq!(
        m.update(Input::Released(Err(err(ErrorKind::Failed)))),
        [Command::List]
    );
    assert_eq!(enroll(&m).outcome, Some(EnrollOutcome::Completed));
    assert_eq!(m.fingers, [Finger::LeftRing]);
}

#[test]
fn enrol_errors_at_each_call() {
    for kind in ErrorKind::ALL {
        // Claim fails: nothing to release, unless it timed out (the service
        // may have claimed the device after all).
        let mut m = ready(&[]);
        m.update(Input::AddFinger);
        m.update(pick(Finger::RightThumb));
        if kind == ErrorKind::Timeout {
            assert_eq!(m.update(Input::Claimed(Err(err(kind)))), [Command::Release]);
            assert_eq!(enroll(&m).step, Step::Releasing);
            assert_eq!(m.update(Input::Released(Err(err(ErrorKind::Failed)))), []);
        } else {
            assert_eq!(m.update(Input::Claimed(Err(err(kind)))), []);
        }
        let e = enroll(&m);
        assert_eq!(e.step, Step::Done);
        match &e.outcome {
            Some(EnrollOutcome::Failed { failure, duplicate }) => {
                assert_eq!(failure.title, kind.title());
                assert_eq!(failure.description, kind.description());
                assert_eq!(*duplicate, kind == ErrorKind::AlreadyEnrolled);
            }
            o => panic!("{kind:?}: {o:?}"),
        }
        assert!(!m.busy());

        // EnrollStart fails: release.
        let mut m = ready(&[]);
        m.update(Input::AddFinger);
        m.update(pick(Finger::RightThumb));
        m.update(Input::Claimed(Ok(Some(8))));
        assert_eq!(
            m.update(Input::EnrollStarted(Err(err(kind)))),
            [Command::Release]
        );
        assert_eq!(m.update(Input::Released(Ok(()))), []);
        let e = enroll(&m);
        assert_eq!(e.step, Step::Done);
        assert!(
            matches!(e.outcome, Some(EnrollOutcome::Failed { .. })),
            "{kind:?}"
        );

        // EnrollStop fails: release anyway.
        let mut m = scanning(Finger::RightThumb, Some(6));
        m.update(status("enroll-failed", true));
        assert_eq!(
            m.update(Input::EnrollStopped(Err(err(kind)))),
            [Command::Release]
        );
    }
}

#[test]
fn enrol_cancel_at_each_step() {
    // While claiming: the claim answer releases.
    let mut m = ready(&[]);
    m.update(Input::AddFinger);
    m.update(pick(Finger::LeftThumb));
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(enroll(&m).outcome, Some(EnrollOutcome::Cancelled));
    assert_eq!(m.update(Input::Cancel), [], "twice is once");
    assert_eq!(m.update(Input::Claimed(Ok(Some(6)))), [Command::Release]);
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(m.page, Page::Fingers);

    // While claiming, and the claim fails: back, nothing to release.
    let mut m = ready(&[]);
    m.update(Input::AddFinger);
    m.update(pick(Finger::LeftThumb));
    m.update(Input::Cancel);
    assert_eq!(m.update(Input::Claimed(Err(err(ErrorKind::Busy)))), []);
    assert_eq!(m.page, Page::Fingers);
    assert!(m.dialog.is_none());

    // While starting: stop once started, then release.
    let mut m = ready(&[]);
    m.update(Input::AddFinger);
    m.update(pick(Finger::LeftThumb));
    m.update(Input::Claimed(Ok(Some(6))));
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(
        m.update(Input::EnrollStarted(Ok(()))),
        [Command::EnrollStop]
    );
    assert_eq!(m.update(Input::EnrollStopped(Ok(()))), [Command::Release]);
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(m.page, Page::Fingers);

    // While starting, and the start fails: release.
    let mut m = ready(&[]);
    m.update(Input::AddFinger);
    m.update(pick(Finger::LeftThumb));
    m.update(Input::Claimed(Ok(Some(6))));
    m.update(Input::Cancel);
    assert_eq!(
        m.update(Input::EnrollStarted(Err(err(ErrorKind::Failed)))),
        [Command::Release]
    );
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(m.page, Page::Fingers);

    // While scanning.
    let mut m = scanning(Finger::LeftThumb, Some(6));
    m.update(status("enroll-stage-passed", false));
    assert_eq!(m.update(Input::Cancel), [Command::EnrollStop]);
    assert_eq!(enroll(&m).step, Step::Stopping);
    // Late statuses and repeated cancels change nothing.
    assert_eq!(m.update(status("enroll-stage-passed", false)), []);
    assert_eq!(m.update(status("enroll-completed", true)), []);
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(enroll(&m).outcome, Some(EnrollOutcome::Cancelled));
    assert_eq!(m.update(Input::EnrollStopped(Ok(()))), [Command::Release]);
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(m.update(status("enroll-stage-passed", false)), []);
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(m.page, Page::Fingers);
    assert!(m.fingers.is_empty());
}

#[test]
fn enroll_statuses_before_the_start_reply() {
    // An ongoing status before the reply is applied.
    let mut m = ready(&[]);
    m.update(Input::AddFinger);
    m.update(pick(Finger::LeftThumb));
    m.update(Input::Claimed(Ok(Some(3))));
    assert_eq!(m.update(status("enroll-stage-passed", false)), []);
    assert_eq!(enroll(&m).passed, 1);
    // A final one waits for the reply, then stops.
    assert_eq!(m.update(status("enroll-completed", true)), []);
    assert_eq!(enroll(&m).step, Step::Starting);
    assert_eq!(
        m.update(Input::EnrollStarted(Ok(()))),
        [Command::EnrollStop]
    );
    m.update(Input::EnrollStopped(Ok(())));
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(enroll(&m).outcome, Some(EnrollOutcome::Completed));

    // Cancelled while starting, then a final status: still a cancel.
    let mut m = ready(&[]);
    m.update(Input::AddFinger);
    m.update(pick(Finger::LeftThumb));
    m.update(Input::Claimed(Ok(Some(3))));
    m.update(Input::Cancel);
    assert_eq!(m.update(status("enroll-failed", true)), []);
    assert_eq!(
        m.update(Input::EnrollStarted(Ok(()))),
        [Command::EnrollStop]
    );
    m.update(Input::EnrollStopped(Ok(())));
    m.update(Input::Released(Ok(())));
    assert_eq!(m.page, Page::Fingers);
}

#[test]
fn enrol_ignores_stray_inputs() {
    let mut m = scanning(Finger::LeftThumb, Some(6));
    let before = m.clone();
    for i in [
        Input::Claimed(Ok(None)),
        Input::Released(Ok(())),
        Input::EnrollStarted(Ok(())),
        Input::EnrollStopped(Ok(())),
        Input::VerifyStarted(Ok(())),
        vstatus("verify-match", true),
        Input::VerifyFingerSelected("left-thumb".into()),
        Input::VerifyStopped(Ok(())),
        Input::Deleted(Ok(())),
        Input::AddFinger,
        Input::TestFinger,
        Input::TryAgain,
        Input::RemoveFinger(Finger::LeftThumb),
        pick(Finger::RightThumb),
        Input::Confirm,
    ] {
        assert_eq!(m.update(i.clone()), [], "{i:?}");
        assert_eq!(m, before, "{i:?}");
    }
}

// ------------------------------------------------------------ test

#[test]
fn test_match_with_a_selected_finger() {
    let mut m = ready(&[Finger::RightIndex]);
    m.update(Input::TestFinger);
    m.update(Input::Claimed(Ok(Some(6))));
    // The selection usually comes after the reply, sometimes before.
    assert_eq!(
        m.update(Input::VerifyFingerSelected("right-index-finger".into())),
        []
    );
    assert_eq!(m.update(Input::VerifyStarted(Ok(()))), []);
    assert_eq!(
        en(&test(&m).instruction),
        "Place the finger \u{201c}Right index finger\u{201d} on the reader."
    );
    assert_eq!(
        m.update(vstatus("verify-match", true)),
        [Command::VerifyStop]
    );
    assert_eq!(m.update(Input::VerifyStopped(Ok(()))), [Command::Release]);
    assert_eq!(m.update(Input::Released(Ok(()))), []);
    let t = test(&m);
    assert_eq!(t.step, Step::Done);
    assert_eq!(
        t.outcome,
        Some(TestOutcome::Match(Some(Finger::RightIndex)))
    );
    let (title, body) = t.outcome.as_ref().unwrap().texts(&Labels::new()).unwrap();
    assert_eq!(en(&title), "Recognised: Right index finger");
    assert_eq!(en(&body), "This finger can unlock the computer.");
    let fr = Catalogue::builtin("fr").unwrap();
    assert_eq!(title.render(&fr), "Reconnu\u{a0}: Index droit");
    // Again, or back.
    let mut again = m.clone();
    assert_eq!(again.update(Input::TryAgain), [Command::Claim]);
    assert_eq!(test(&again).step, Step::Claiming);
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(m.page, Page::Fingers);
}

#[test]
fn test_match_with_any_finger_and_no_match() {
    let mut m = verifying(&[Finger::LeftThumb, Finger::RightThumb]);
    assert_eq!(
        en(&test(&m).instruction),
        "Place one of your registered fingers on the reader."
    );
    m.update(Input::VerifyFingerSelected("any".into()));
    assert_eq!(test(&m).selected, None);
    m.update(vstatus("verify-match", true));
    m.update(Input::VerifyStopped(Ok(())));
    m.update(Input::Released(Ok(())));
    let o = test(&m).outcome.clone().unwrap();
    assert_eq!(o, TestOutcome::Match(None));
    let (title, body) = o.texts(&Labels::new()).unwrap();
    assert_eq!(en(&title), "Recognised");
    assert_eq!(
        en(&body),
        "This is one of your registered fingers. It can unlock the computer."
    );

    let mut m = verifying(&[Finger::LeftThumb]);
    m.update(Input::VerifyFingerSelected("left-thumb".into()));
    assert_eq!(
        en(&test(&m).instruction),
        "Place the finger \u{201c}Left thumb\u{201d} on the reader."
    );
    assert_eq!(
        m.update(vstatus("verify-no-match", true)),
        [Command::VerifyStop]
    );
    m.update(Input::VerifyStopped(Ok(())));
    m.update(Input::Released(Ok(())));
    let o = test(&m).outcome.clone().unwrap();
    assert_eq!(o, TestOutcome::NoMatch);
    let (title, body) = o.texts(&Labels::new()).unwrap();
    assert_eq!(en(&title), "Not recognised");
    assert_eq!(
        en(&body),
        "This finger is not registered, or it was not placed well. You can try again."
    );
    assert_eq!(TestOutcome::Cancelled.texts(&Labels::new()), None);
}

#[test]
fn every_ongoing_verify_status_keeps_scanning() {
    for (s, expected) in [
        (
            "verify-retry-scan",
            "That did not work. Lift your finger, then place it on the reader again.",
        ),
        (
            "verify-swipe-too-short",
            "The swipe was too short. Slide your whole finger slowly over the reader.",
        ),
        (
            "verify-finger-not-centered",
            "Your finger was not in the middle of the reader. Place it in the middle and try again.",
        ),
        (
            "verify-remove-and-retry",
            "Lift your finger completely off the reader, then place it again.",
        ),
        (
            "verify-too-fast",
            "Too quick. Leave your finger on the reader a little longer.",
        ),
        (
            "verify-unheard-of",
            "That did not work. Lift your finger, then place it on the reader again.",
        ),
    ] {
        let mut m = verifying(&[Finger::LeftThumb]);
        assert_eq!(m.update(vstatus(s, false)), [], "{s}");
        let t = test(&m);
        assert_eq!(t.step, Step::Scanning);
        assert_eq!(en(&t.instruction), expected, "{s}");
        assert!(t.outcome.is_none());
    }
}

#[test]
fn every_final_verify_status() {
    for (s, done, outcome) in [
        ("verify-match", true, TestOutcome::Match(None)),
        ("verify-no-match", true, TestOutcome::NoMatch),
        ("verify-no-match", false, TestOutcome::NoMatch),
        (
            "verify-unknown-error",
            true,
            TestOutcome::Failed(Failure::test(status::unknown_error())),
        ),
        (
            "verify-retry-scan",
            true,
            TestOutcome::Failed(Failure::test(status::unknown_error())),
        ),
        (
            "verify-disconnected",
            true,
            TestOutcome::Failed(Failure::test(status::disconnected())),
        ),
    ] {
        let mut m = verifying(&[Finger::LeftThumb, Finger::LeftIndex]);
        let c = m.update(vstatus(s, done));
        if s == "verify-disconnected" {
            assert_eq!(c, [Command::Release], "{s}");
        } else {
            assert_eq!(c, [Command::VerifyStop], "{s}");
            assert_eq!(m.update(Input::VerifyStopped(Ok(()))), [Command::Release]);
        }
        assert_eq!(m.update(Input::Released(Ok(()))), []);
        let t = test(&m);
        assert_eq!(t.step, Step::Done, "{s}");
        assert_eq!(t.outcome.as_ref(), Some(&outcome), "{s}");
        if let TestOutcome::Failed(f) = &outcome {
            assert_eq!(en(&f.title), "The test did not work");
            let (title, _) = outcome.texts(&Labels::new()).unwrap();
            assert_eq!(title, f.title);
        }
    }
}

#[test]
fn test_errors_and_cancels() {
    // Nothing enrolled for this user (the list was stale).
    let mut m = ready(&[Finger::LeftThumb]);
    m.update(Input::TestFinger);
    m.update(Input::Claimed(Ok(None)));
    assert_eq!(
        m.update(Input::VerifyStarted(Err(err(ErrorKind::NoPrints)))),
        [Command::Release]
    );
    m.update(Input::Released(Ok(())));
    match &test(&m).outcome {
        Some(TestOutcome::Failed(f)) => {
            assert_eq!(en(&f.title), "No registered fingers");
            assert_eq!(
                en(&f.description),
                "Add a finger first, then you can test it."
            );
        }
        o => panic!("{o:?}"),
    }

    for kind in ErrorKind::ALL {
        let mut m = ready(&[Finger::LeftThumb]);
        m.update(Input::TestFinger);
        if kind == ErrorKind::Timeout {
            assert_eq!(m.update(Input::Claimed(Err(err(kind)))), [Command::Release]);
            assert_eq!(m.update(Input::Released(Ok(()))), []);
        } else {
            assert_eq!(m.update(Input::Claimed(Err(err(kind)))), []);
        }
        assert_eq!(test(&m).step, Step::Done);
        assert_eq!(
            test(&m).outcome,
            Some(TestOutcome::Failed(Failure::service(&err(kind))))
        );
        assert!(!m.busy());
        // VerifyStop fails: release anyway.
        let mut m = verifying(&[Finger::LeftThumb]);
        m.update(vstatus("verify-match", true));
        assert_eq!(
            m.update(Input::VerifyStopped(Err(err(kind)))),
            [Command::Release]
        );
    }

    // Cancel while claiming.
    let mut m = ready(&[Finger::LeftThumb]);
    m.update(Input::TestFinger);
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(m.update(Input::Claimed(Ok(None))), [Command::Release]);
    assert_eq!(m.update(Input::Released(Ok(()))), []);
    assert_eq!(m.page, Page::Fingers);
    // ...and the claim fails.
    let mut m = ready(&[Finger::LeftThumb]);
    m.update(Input::TestFinger);
    m.update(Input::Cancel);
    assert_eq!(m.update(Input::Claimed(Err(err(ErrorKind::Busy)))), []);
    assert_eq!(m.page, Page::Fingers);
    // Cancel while starting.
    let mut m = ready(&[Finger::LeftThumb]);
    m.update(Input::TestFinger);
    m.update(Input::Claimed(Ok(None)));
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(m.update(vstatus("verify-match", true)), []);
    assert_eq!(
        m.update(Input::VerifyStarted(Ok(()))),
        [Command::VerifyStop]
    );
    assert_eq!(m.update(Input::VerifyStopped(Ok(()))), [Command::Release]);
    m.update(Input::Released(Ok(())));
    assert_eq!(m.page, Page::Fingers);
    // ...and the start fails.
    let mut m = ready(&[Finger::LeftThumb]);
    m.update(Input::TestFinger);
    m.update(Input::Claimed(Ok(None)));
    m.update(Input::Cancel);
    assert_eq!(
        m.update(Input::VerifyStarted(Err(err(ErrorKind::Failed)))),
        [Command::Release]
    );
    m.update(Input::Released(Ok(())));
    assert_eq!(m.page, Page::Fingers);
    // Cancel while scanning.
    let mut m = verifying(&[Finger::LeftThumb]);
    assert_eq!(m.update(Input::Cancel), [Command::VerifyStop]);
    assert_eq!(m.update(vstatus("verify-match", true)), []);
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(m.update(Input::VerifyStopped(Ok(()))), [Command::Release]);
    m.update(Input::Released(Ok(())));
    assert_eq!(m.page, Page::Fingers);

    // A final status before the start reply waits for it.
    let mut m = ready(&[Finger::LeftThumb]);
    m.update(Input::TestFinger);
    m.update(Input::Claimed(Ok(None)));
    assert_eq!(m.update(vstatus("verify-no-match", true)), []);
    assert_eq!(
        m.update(Input::VerifyStarted(Ok(()))),
        [Command::VerifyStop]
    );
    m.update(Input::VerifyStopped(Ok(())));
    m.update(Input::Released(Ok(())));
    assert_eq!(test(&m).outcome, Some(TestOutcome::NoMatch));
}

#[test]
fn test_ignores_stray_inputs() {
    let mut m = verifying(&[Finger::LeftThumb]);
    let before = m.clone();
    for i in [
        Input::Claimed(Ok(None)),
        Input::Released(Ok(())),
        Input::EnrollStarted(Ok(())),
        status("enroll-completed", true),
        Input::EnrollStopped(Ok(())),
        Input::VerifyStarted(Ok(())),
        Input::VerifyStopped(Ok(())),
        Input::Deleted(Ok(())),
        Input::AddFinger,
        Input::TestFinger,
        Input::TryAgain,
        Input::Confirm,
    ] {
        assert_eq!(m.update(i.clone()), [], "{i:?}");
        assert_eq!(m, before, "{i:?}");
    }
    // Statuses after the end are ignored too.
    m.update(vstatus("verify-match", true));
    m.update(Input::VerifyStopped(Ok(())));
    m.update(Input::Released(Ok(())));
    let done = m.clone();
    for i in [
        vstatus("verify-no-match", true),
        Input::VerifyFingerSelected("left-thumb".into()),
        Input::Released(Ok(())),
    ] {
        assert_eq!(m.update(i), []);
        assert_eq!(m, done);
    }
}

// ------------------------------------------------------------ removal

#[test]
fn remove_one_finger() {
    let mut m = ready(&[Finger::LeftThumb, Finger::RightIndex]);
    assert_eq!(
        m.update(Input::RemoveFinger(Finger::LeftIndex)),
        [],
        "not enrolled"
    );
    assert!(m.dialog.is_none());
    assert_eq!(m.update(Input::RemoveFinger(Finger::RightIndex)), []);
    assert_eq!(m.dialog, Some(Dialog::ConfirmRemove(Finger::RightIndex)));
    // While the dialog is up, the page does not react.
    assert_eq!(m.update(Input::AddFinger), []);
    assert_eq!(m.update(Input::TestFinger), []);
    assert_eq!(m.page, Page::Fingers);
    // Dismiss, then again and confirm.
    assert_eq!(m.update(Input::Dismiss), []);
    assert!(m.dialog.is_none());
    m.update(Input::RemoveFinger(Finger::RightIndex));
    assert_eq!(m.update(Input::Confirm), [Command::Claim]);
    assert!(m.dialog.is_none());
    assert!(m.busy());
    assert_eq!(
        m.update(Input::Claimed(Ok(Some(6)))),
        [Command::DeleteFinger(Finger::RightIndex)]
    );
    // Stray inputs while deleting.
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(m.update(Input::Released(Ok(()))), []);
    assert_eq!(m.update(Input::Deleted(Ok(()))), [Command::Release]);
    assert_eq!(m.update(Input::Deleted(Ok(()))), []);
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(m.page, Page::Fingers);
    assert_eq!(m.fingers, [Finger::LeftThumb]);
    let notice = m.take_notice().unwrap();
    assert_eq!(en(&notice), "Fingerprint removed: Right index finger");
    assert!(m.take_notice().is_none());
    assert!(!m.busy());
}

#[test]
fn remove_falls_back_to_remove_all() {
    let mut m = ready(&[Finger::LeftThumb, Finger::RightIndex]);
    m.update(Input::RemoveFinger(Finger::LeftThumb));
    m.update(Input::Confirm);
    m.update(Input::Claimed(Ok(None)));
    assert_eq!(
        m.update(Input::Deleted(Err(err(ErrorKind::Unsupported)))),
        [Command::Release]
    );
    // Released before asking: the device is not held while the user reads.
    assert_eq!(m.update(Input::Released(Ok(()))), []);
    assert_eq!(m.page, Page::Fingers);
    assert_eq!(m.dialog, Some(Dialog::ConfirmRemoveAll(Finger::LeftThumb)));
    let (title, body, ok, cancel) = m.dialog.as_ref().unwrap().texts(&m.labels);
    assert_eq!(en(&title), "Remove all your fingerprints?");
    assert!(en(&body).starts_with("This fingerprint service can only remove all"));
    assert_eq!(en(&ok), "Remove all");
    assert_eq!(cancel.map(|c| en(&c)).as_deref(), Some("Cancel"));
    assert_eq!(m.fingers.len(), 2, "nothing removed yet");
    // Declining keeps everything.
    let mut no = m.clone();
    no.update(Input::Dismiss);
    assert!(no.dialog.is_none());
    assert_eq!(no.fingers.len(), 2);
    // Accepting removes all.
    assert_eq!(m.update(Input::Confirm), [Command::Claim]);
    assert_eq!(m.update(Input::Claimed(Ok(None))), [Command::DeleteAll]);
    assert_eq!(m.update(Input::Deleted(Ok(()))), [Command::Release]);
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert!(m.fingers.is_empty());
    assert_eq!(
        en(&m.take_notice().unwrap()),
        "All your fingerprints were removed."
    );

    // If even "remove all" is missing, it is an error, not a loop.
    let mut m = ready(&[Finger::LeftThumb]);
    m.update(Input::RemoveFinger(Finger::LeftThumb));
    m.update(Input::Confirm);
    m.update(Input::Claimed(Ok(None)));
    m.update(Input::Deleted(Err(err(ErrorKind::Unsupported))));
    m.update(Input::Released(Ok(())));
    m.update(Input::Confirm);
    m.update(Input::Claimed(Ok(None)));
    assert_eq!(
        m.update(Input::Deleted(Err(err(ErrorKind::Unsupported)))),
        [Command::Release]
    );
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert!(matches!(m.dialog, Some(Dialog::Error(_))));
}

#[test]
fn remove_errors() {
    for kind in ErrorKind::ALL {
        // The claim fails: an error dialog, nothing to release (but after a
        // timeout).
        let mut m = ready(&[Finger::LeftThumb]);
        m.update(Input::RemoveFinger(Finger::LeftThumb));
        m.update(Input::Confirm);
        if kind == ErrorKind::Timeout {
            assert_eq!(m.update(Input::Claimed(Err(err(kind)))), [Command::Release]);
            assert!(m.dialog.is_none(), "not before the release");
            assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
        } else {
            assert_eq!(m.update(Input::Claimed(Err(err(kind)))), []);
        }
        assert_eq!(m.page, Page::Fingers);
        assert_eq!(m.dialog, Some(Dialog::Error(Failure::service(&err(kind)))));
        let (title, body, ok, cancel) = m.dialog.as_ref().unwrap().texts(&m.labels);
        assert_eq!(title, kind.title());
        assert_eq!(body, kind.description());
        assert_eq!(en(&ok), "OK");
        assert!(cancel.is_none());
        // OK closes it.
        assert_eq!(m.update(Input::Confirm), []);
        assert!(m.dialog.is_none());
        assert_eq!(m.fingers, [Finger::LeftThumb]);

        if kind == ErrorKind::Unsupported {
            continue;
        }
        // The delete fails: release, then the error.
        let mut m = ready(&[Finger::LeftThumb]);
        m.update(Input::RemoveFinger(Finger::LeftThumb));
        m.update(Input::Confirm);
        m.update(Input::Claimed(Ok(None)));
        assert_eq!(m.update(Input::Deleted(Err(err(kind)))), [Command::Release]);
        assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
        assert_eq!(m.dialog, Some(Dialog::Error(Failure::service(&err(kind)))));
        assert_eq!(m.fingers, [Finger::LeftThumb]);
        assert!(m.notice.is_none());
    }
}

#[test]
fn confirm_dialog_texts() {
    let d = Dialog::ConfirmRemove(Finger::LeftLittle);
    let (title, body, ok, cancel) = d.texts(&Labels::new());
    assert_eq!(en(&title), "Remove this fingerprint?");
    assert_eq!(
        en(&body),
        "\u{201c}Left little finger\u{201d} will no longer unlock this computer. You can add it again later."
    );
    // With a name, the name.
    let named = Labels::from([(Finger::LeftLittle, "Pinky".to_string())]);
    assert!(en(&d.texts(&named).1).starts_with("\u{201c}Pinky\u{201d} will no longer"));
    assert_eq!(en(&ok), "Remove");
    assert_eq!(en(&cancel.unwrap()), "Cancel");
    let fr = Catalogue::builtin("fr").unwrap();
    assert_eq!(
        body.render(&fr),
        "«\u{a0}Petit doigt gauche\u{a0}» ne pourra plus déverrouiller cet ordinateur. Vous pourrez l'ajouter de nouveau plus tard."
    );
    // Confirm with no dialog, or an error dialog, starts nothing.
    let mut m = ready(&[Finger::LeftThumb]);
    assert_eq!(m.update(Input::Confirm), []);
    m.dialog = Some(Dialog::Error(Failure::service(&err(ErrorKind::Failed))));
    assert_eq!(m.update(Input::Confirm), []);
    assert_eq!(m.page, Page::Fingers);
    // A confirm dialog outside the fingers page starts nothing either.
    let mut m = scanning(Finger::LeftThumb, None);
    m.dialog = Some(Dialog::ConfirmRemove(Finger::LeftThumb));
    assert_eq!(m.update(Input::Confirm), []);
    assert!(m.dialog.is_none());
    assert!(matches!(m.page, Page::Enroll(_)));
}

// ------------------------------------------------------------ the service goes away

#[test]
fn service_lost() {
    // Idle: fprintd exits when unused; nothing to say.
    for mut m in [ready(&[]), {
        let mut m = ready(&[]);
        m.update(Input::AddFinger);
        m
    }] {
        let before = m.clone();
        assert_eq!(m.update(Input::ServiceLost), []);
        assert_eq!(m, before);
    }
    // Enrolling: failed, with nothing left to release.
    let mut m = scanning(Finger::LeftThumb, Some(6));
    assert_eq!(m.update(Input::ServiceLost), []);
    let e = enroll(&m);
    assert_eq!(e.step, Step::Done);
    match &e.outcome {
        Some(EnrollOutcome::Failed { failure, .. }) => {
            assert_eq!(failure.title, ErrorKind::Unavailable.title());
        }
        o => panic!("{o:?}"),
    }
    assert!(!m.busy());
    // Enrolment cancelled, then lost: back to the list.
    let mut m = scanning(Finger::LeftThumb, Some(6));
    m.update(Input::Cancel);
    m.update(Input::ServiceLost);
    assert_eq!(m.page, Page::Fingers);
    // Completed, then lost before the release: still completed.
    let mut m = scanning(Finger::LeftThumb, Some(6));
    m.update(status("enroll-completed", true));
    m.update(Input::ServiceLost);
    assert_eq!(enroll(&m).outcome, Some(EnrollOutcome::Completed));
    assert_eq!(enroll(&m).step, Step::Done);
    // Done: nothing changes.
    let (mut m, _) = enrol_ending("enroll-failed", true);
    let before = m.clone();
    m.update(Input::ServiceLost);
    assert_eq!(m, before);

    // Testing.
    let mut m = verifying(&[Finger::LeftThumb]);
    m.update(Input::ServiceLost);
    assert_eq!(test(&m).step, Step::Done);
    assert!(matches!(test(&m).outcome, Some(TestOutcome::Failed(_))));
    let mut m = verifying(&[Finger::LeftThumb]);
    m.update(Input::Cancel);
    m.update(Input::ServiceLost);
    assert_eq!(m.page, Page::Fingers);
    let mut m = verifying(&[Finger::LeftThumb]);
    m.update(vstatus("verify-match", true));
    m.update(Input::ServiceLost);
    assert_eq!(test(&m).outcome, Some(TestOutcome::Match(None)));

    // Removing.
    let mut m = ready(&[Finger::LeftThumb]);
    m.update(Input::RemoveFinger(Finger::LeftThumb));
    m.update(Input::Confirm);
    m.update(Input::Claimed(Ok(None)));
    assert_eq!(m.update(Input::ServiceLost), []);
    assert_eq!(m.page, Page::Fingers);
    assert!(matches!(m.dialog, Some(Dialog::Error(_))));
}

// ------------------------------------------------------------ the release rule

/// A fake service: answers each command, holds the claim, and checks the
/// calls are made one at a time on a claimed device.
struct Sim {
    m: Model,
    claimed: bool,
    calls: Vec<Command>,
}

/// What the fake service does in one scenario.
#[derive(Clone, Copy, Debug)]
struct Plan {
    /// Fail the n-th call (0-based) with this kind.
    fail_at: Option<(usize, ErrorKind)>,
    /// The user presses Cancel before the n-th call is answered.
    cancel_at: Option<usize>,
    /// The final status of the scan.
    last: &'static str,
}

impl Sim {
    fn run(mut m: Model, start: Input, plan: Plan) -> Sim {
        let mut sim = Sim {
            claimed: false,
            calls: Vec::new(),
            m: Model::new(),
        };
        let mut queue: std::collections::VecDeque<Command> = m.update(start).into();
        let mut n = 0;
        while let Some(cmd) = queue.pop_front() {
            assert!(
                queue.is_empty(),
                "one call at a time: {cmd:?} then {queue:?}"
            );
            sim.calls.push(cmd.clone());
            if plan.cancel_at == Some(n) {
                assert!(m.update(Input::Cancel).is_empty());
            }
            let fail = match plan.fail_at {
                Some((at, kind)) if at == n => Some(err(kind)),
                _ => None,
            };
            n += 1;
            let r = |ok: ()| match &fail {
                Some(e) => Err(e.clone()),
                None => Ok(ok),
            };
            let mut inputs = Vec::new();
            match &cmd {
                Command::Claim => {
                    assert!(!sim.claimed, "claimed twice");
                    // A timed-out claim may have happened anyway: the worst
                    // case is that it did.
                    if fail.as_ref().is_none_or(|e| e.kind == ErrorKind::Timeout) {
                        sim.claimed = true;
                    }
                    inputs.push(Input::Claimed(r(()).map(|()| Some(3))));
                }
                Command::Release => {
                    assert!(sim.claimed, "release without claim");
                    sim.claimed = false;
                    inputs.push(Input::Released(r(())));
                }
                Command::EnrollStart(_) => {
                    assert!(sim.claimed);
                    inputs.push(Input::EnrollStarted(r(())));
                    if fail.is_none() {
                        inputs.push(status("enroll-stage-passed", false));
                        inputs.push(status("enroll-retry-scan", false));
                        inputs.push(status(plan.last, true));
                    }
                }
                Command::VerifyStart => {
                    assert!(sim.claimed);
                    inputs.push(Input::VerifyStarted(r(())));
                    if fail.is_none() {
                        inputs.push(Input::VerifyFingerSelected("any".into()));
                        inputs.push(vstatus("verify-retry-scan", false));
                        inputs.push(vstatus(plan.last, true));
                    }
                }
                Command::EnrollStop => {
                    assert!(sim.claimed);
                    inputs.push(Input::EnrollStopped(r(())));
                }
                Command::VerifyStop => {
                    assert!(sim.claimed);
                    inputs.push(Input::VerifyStopped(r(())));
                }
                Command::DeleteFinger(_) | Command::DeleteAll => {
                    assert!(sim.claimed);
                    inputs.push(Input::Deleted(r(())));
                }
                Command::List => inputs.push(Input::Listed(Ok(vec![]))),
                Command::Connect => inputs.push(Input::Connected(Ok(press()))),
            }
            for i in inputs {
                queue.extend(m.update(i));
            }
        }
        sim.m = m;
        sim
    }
}

#[test]
fn every_claim_is_released_whatever_happens() {
    let mut scenarios = 0;
    let kinds = ErrorKind::ALL;
    let enroll_lasts = [
        "enroll-completed",
        "enroll-failed",
        "enroll-data-full",
        "enroll-duplicate",
        "enroll-disconnected",
        "enroll-unknown-error",
    ];
    let verify_lasts = [
        "verify-match",
        "verify-no-match",
        "verify-disconnected",
        "verify-unknown-error",
    ];
    for cancel_at in [None, Some(0), Some(1), Some(2), Some(3), Some(4)] {
        for fail_at in std::iter::once(None)
            .chain((0..5).flat_map(|n| kinds.iter().map(move |k| Some((n, *k)))))
        {
            for last in enroll_lasts {
                let mut m = ready(&[]);
                m.update(Input::AddFinger);
                let plan = Plan {
                    fail_at,
                    cancel_at,
                    last,
                };
                let sim = Sim::run(m, pick(Finger::LeftThumb), plan);
                assert!(!sim.claimed, "{plan:?}: {:?}", sim.calls);
                assert!(!sim.m.busy(), "{plan:?}: {:?}", sim.m.page);
                scenarios += 1;
            }
            for last in verify_lasts {
                let plan = Plan {
                    fail_at,
                    cancel_at,
                    last,
                };
                let sim = Sim::run(ready(&[Finger::LeftThumb]), Input::TestFinger, plan);
                assert!(!sim.claimed, "{plan:?}: {:?}", sim.calls);
                assert!(!sim.m.busy(), "{plan:?}: {:?}", sim.m.page);
                scenarios += 1;
            }
            let mut m = ready(&[Finger::LeftThumb]);
            m.update(Input::RemoveFinger(Finger::LeftThumb));
            let plan = Plan {
                fail_at,
                cancel_at,
                last: "",
            };
            let sim = Sim::run(m, Input::Confirm, plan);
            assert!(!sim.claimed, "{plan:?}: {:?}", sim.calls);
            assert!(!sim.m.busy(), "{plan:?}: {:?}", sim.m.page);
            scenarios += 1;
        }
    }
    assert_eq!(scenarios, 6 * (1 + 5 * kinds.len()) * (6 + 4 + 1));
}

#[test]
fn holding_the_reader() {
    // Loading is busy but holds nothing: closing then need not wait.
    let mut m = Model::new();
    assert!(m.busy() && !m.holds_reader());
    m.update(Input::Start);
    assert!(!m.holds_reader());
    let mut m = scanning(Finger::LeftThumb, None);
    assert!(m.holds_reader());
    m.update(Input::Cancel);
    assert!(m.holds_reader(), "until released");
    m.update(Input::EnrollStopped(Ok(())));
    assert!(m.holds_reader());
    m.update(Input::Released(Ok(())));
    assert!(!m.holds_reader());
    let mut m = verifying(&[Finger::LeftThumb]);
    assert!(m.holds_reader());
    m.update(vstatus("verify-match", true));
    m.update(Input::VerifyStopped(Ok(())));
    m.update(Input::Released(Ok(())));
    assert!(!m.holds_reader());
    let mut m = ready(&[Finger::LeftThumb]);
    m.update(Input::RemoveFinger(Finger::LeftThumb));
    assert!(!m.holds_reader(), "asking is not holding");
    m.update(Input::Confirm);
    assert!(m.holds_reader());
    let mut c = ready(&[]);
    c.update(Input::AddFinger);
    assert!(!c.holds_reader());
}

#[test]
fn busy_by_page() {
    let mut m = Model::new();
    assert!(m.busy());
    m.page = Page::Fingers;
    assert!(!m.busy());
    m.page = Page::Add;
    assert!(!m.busy());
    m.page = Page::Problem(Failure::service(&err(ErrorKind::Failed)));
    assert!(!m.busy());
    assert!(m.is_enrolled(Finger::LeftThumb) == m.fingers.contains(&Finger::LeftThumb));
}

// ------------------------------------------------------------ names

fn labels(pairs: &[(Finger, &str)]) -> Labels {
    pairs.iter().map(|(f, l)| (*f, l.to_string())).collect()
}

fn ready_named(fingers: &[Finger], names: &[(Finger, &str)]) -> Model {
    let mut m = Model::with_labels(labels(names));
    m.update(Input::Start);
    m.update(Input::Connected(Ok(press())));
    m.update(Input::Listed(Ok(fingers.to_vec())));
    assert_eq!(m.page, Page::Fingers);
    m
}

fn named(label: &str) -> Input {
    Input::StartEnroll {
        finger: None,
        label: Some(label.into()),
    }
}

/// Runs an enrolment that starts on the add page to completion.
fn complete(m: &mut Model) {
    let slot = enroll(m).slot;
    assert_eq!(
        m.update(Input::Claimed(Ok(Some(2)))),
        [Command::EnrollStart(slot)]
    );
    m.update(Input::EnrollStarted(Ok(())));
    m.update(status("enroll-stage-passed", false));
    m.update(status("enroll-completed", true));
    m.update(Input::EnrollStopped(Ok(())));
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(enroll(m).outcome, Some(EnrollOutcome::Completed));
}

#[test]
fn display_names_and_the_default_choice() {
    let m = ready_named(
        &[Finger::RightIndex, Finger::LeftThumb],
        &[(Finger::LeftThumb, "Au lit")],
    );
    assert_eq!(
        m.display_name(Finger::LeftThumb),
        Arg::Text("Au lit".into())
    );
    assert_eq!(
        m.display_name(Finger::RightIndex),
        Arg::Finger(Finger::RightIndex)
    );
    assert_eq!(
        name_of(&m.labels, Finger::LeftThumb),
        Arg::Text("Au lit".into())
    );
    // Loading names is not a change to save.
    let mut m2 = m.clone();
    assert_eq!(m2.take_labels_to_save(), None);
    // The add page starts on the first free finger, indexes then thumbs.
    assert_eq!(m.default_choice(), Some(Finger::LeftIndex));
    assert_eq!(ready(&[]).default_choice(), Some(Finger::RightIndex));
    assert_eq!(
        ready(&crate::fingers::COMMON).default_choice(),
        Some(Finger::RightMiddle)
    );
    assert_eq!(ready(&Finger::ALL).default_choice(), None);
}

#[test]
fn start_enroll_validates_the_choice() {
    let mut m = ready(&[Finger::RightIndex]);
    m.update(Input::AddFinger);
    assert_eq!(m.page, Page::Add);
    // An enrolled finger, or no finger and no name: nothing happens.
    assert_eq!(m.update(pick(Finger::RightIndex)), []);
    for label in [None, Some(String::new()), Some("   ".to_string())] {
        assert_eq!(
            m.update(Input::StartEnroll {
                finger: None,
                label
            }),
            []
        );
        assert_eq!(m.page, Page::Add);
    }
    // Stray inputs on the add page.
    for i in [
        Input::TestFinger,
        Input::Claimed(Ok(None)),
        Input::TryAgain,
        Input::RenameFinger(Finger::RightIndex),
        Input::SetLabel(Finger::RightIndex, Some("x".into())),
    ] {
        assert_eq!(m.update(i), []);
        assert_eq!(m.page, Page::Add);
    }
    // A name alone: the first free slot (the left index), no finger to name.
    assert_eq!(m.update(named("  Au lit  ")), [Command::Claim]);
    let e = enroll(&m);
    assert_eq!((e.slot, e.finger), (Finger::LeftIndex, None));
    assert_eq!(e.label.as_deref(), Some("Au lit"));
    assert_eq!(en(&e.title()), "Adding \u{201c}Au lit\u{201d}");
    m.update(Input::Claimed(Ok(Some(3))));
    m.update(Input::EnrollStarted(Ok(())));
    assert_eq!(
        en(&enroll(&m).instruction),
        "Place your finger flat on the reader."
    );

    // A finger and a name of its own.
    let mut m = ready(&[]);
    m.update(Input::AddFinger);
    m.update(Input::StartEnroll {
        finger: Some(Finger::LeftThumb),
        label: Some("Pouce, quand je suis au lit".into()),
    });
    let e = enroll(&m);
    assert_eq!(
        (e.slot, e.finger),
        (Finger::LeftThumb, Some(Finger::LeftThumb))
    );
    m.update(Input::Claimed(Ok(Some(3))));
    m.update(Input::EnrollStarted(Ok(())));
    // The finger is named in the instruction; the title uses the name.
    assert_eq!(
        en(&enroll(&m).instruction),
        "Place your left thumb flat on the reader."
    );
    assert_eq!(
        en(&enroll(&m).title()),
        "Adding \u{201c}Pouce, quand je suis au lit\u{201d}"
    );
    // A finger with a blank name: the standard name.
    let mut m = ready(&[]);
    m.update(Input::AddFinger);
    m.update(Input::StartEnroll {
        finger: Some(Finger::LeftThumb),
        label: Some(" ".into()),
    });
    assert_eq!(enroll(&m).label, None);
    assert_eq!(en(&enroll(&m).title()), "Adding \u{201c}Left thumb\u{201d}");
}

#[test]
fn nameless_slots_follow_the_preferred_order() {
    let mut m = ready(&[]);
    let mut slots = Vec::new();
    for i in 0..10 {
        m.update(Input::AddFinger);
        assert_eq!(m.update(named(&format!("n{i}"))), [Command::Claim]);
        slots.push(enroll(&m).slot);
        complete(&mut m);
        m.update(Input::Cancel);
    }
    assert_eq!(slots, crate::fingers::PREFERRED);
    assert_eq!(m.fingers.len(), 10);
    assert_eq!(m.labels.len(), 10);
    // Full: nothing more to add.
    assert_eq!(m.update(Input::AddFinger), []);
    assert_eq!(m.page, Page::Fingers);
}

#[test]
fn the_name_is_kept_only_for_a_completed_enrolment() {
    let mut m = ready(&[]);
    m.update(Input::AddFinger);
    m.update(named("Au lit"));
    complete(&mut m);
    assert_eq!(m.labels, labels(&[(Finger::RightIndex, "Au lit")]));
    assert_eq!(
        m.take_labels_to_save(),
        Some(labels(&[(Finger::RightIndex, "Au lit")]))
    );
    assert_eq!(m.take_labels_to_save(), None, "once");
    // The list refresh confirms it: nothing to save.
    m.update(Input::Listed(Ok(vec![Finger::RightIndex])));
    assert_eq!(m.take_labels_to_save(), None);

    // Failed or cancelled: no name, nothing to save.
    for end in ["fail", "cancel"] {
        let mut m = ready(&[]);
        m.update(Input::AddFinger);
        m.update(named("Perdu"));
        m.update(Input::Claimed(Ok(Some(3))));
        m.update(Input::EnrollStarted(Ok(())));
        if end == "fail" {
            m.update(status("enroll-failed", true));
        } else {
            m.update(Input::Cancel);
        }
        m.update(Input::EnrollStopped(Ok(())));
        m.update(Input::Released(Ok(())));
        assert!(m.labels.is_empty(), "{end}");
        assert_eq!(m.take_labels_to_save(), None, "{end}");
        if end == "fail" {
            // "Try again" keeps the slot, the finger choice and the name.
            assert_eq!(m.update(Input::TryAgain), [Command::Claim]);
            let e = enroll(&m);
            assert_eq!(
                (e.slot, e.finger, e.label.as_deref()),
                (Finger::RightIndex, None, Some("Perdu"))
            );
        }
    }
}

#[test]
fn rename() {
    let mut m = ready_named(&[Finger::LeftThumb], &[(Finger::LeftThumb, "Au lit")]);
    // Not enrolled: nothing.
    assert_eq!(m.update(Input::RenameFinger(Finger::RightIndex)), []);
    assert!(m.dialog.is_none());
    // Without the dialog, a name is not taken.
    m.update(Input::SetLabel(Finger::LeftThumb, Some("x".into())));
    assert_eq!(m.labels[&Finger::LeftThumb], "Au lit");
    assert_eq!(m.update(Input::RenameFinger(Finger::LeftThumb)), []);
    assert_eq!(m.dialog, Some(Dialog::Rename(Finger::LeftThumb)));
    let (title, body, ok, cancel) = m.dialog.as_ref().unwrap().texts(&m.labels);
    assert_eq!(en(&title), "Rename this fingerprint");
    assert!(en(&body).starts_with("Choose a name you will recognise."));
    assert_eq!(en(&ok), "Rename");
    assert_eq!(en(&cancel.unwrap()), "Cancel");
    // "OK" without the text, another finger's name, other inputs: nothing.
    assert_eq!(m.update(Input::Confirm), []);
    assert_eq!(m.dialog, Some(Dialog::Rename(Finger::LeftThumb)));
    m.update(Input::SetLabel(Finger::RightIndex, Some("x".into())));
    m.update(Input::TestFinger);
    m.update(Input::AddFinger);
    assert_eq!(m.dialog, Some(Dialog::Rename(Finger::LeftThumb)));
    assert_eq!(m.page, Page::Fingers);
    // The new name, cleaned.
    assert_eq!(
        m.update(Input::SetLabel(Finger::LeftThumb, Some(" Le soir ".into()))),
        []
    );
    assert!(m.dialog.is_none());
    assert_eq!(m.labels[&Finger::LeftThumb], "Le soir");
    assert_eq!(
        en(&m.take_notice().unwrap()),
        "Fingerprint renamed: Le soir"
    );
    assert_eq!(
        m.take_labels_to_save(),
        Some(labels(&[(Finger::LeftThumb, "Le soir")]))
    );
    // The same name again: nothing to save.
    m.update(Input::RenameFinger(Finger::LeftThumb));
    m.update(Input::SetLabel(Finger::LeftThumb, Some("Le soir".into())));
    assert_eq!(m.take_labels_to_save(), None);
    // No name: back to the standard name.
    m.update(Input::RenameFinger(Finger::LeftThumb));
    m.update(Input::SetLabel(Finger::LeftThumb, None));
    assert!(m.labels.is_empty());
    assert_eq!(
        en(&m.take_notice().unwrap()),
        "Fingerprint renamed: Left thumb"
    );
    assert_eq!(m.take_labels_to_save(), Some(Labels::new()));
    // Cancel: unchanged.
    m.update(Input::RenameFinger(Finger::LeftThumb));
    m.update(Input::Dismiss);
    assert!(m.dialog.is_none());
    assert_eq!(m.take_labels_to_save(), None);
    assert!(m.notice.is_none());
}

#[test]
fn names_of_fingerprints_gone_are_cleaned_up() {
    let mut m = Model::with_labels(labels(&[
        (Finger::LeftThumb, "a"),
        (Finger::RightIndex, "b"),
        (Finger::LeftRing, "c"),
    ]));
    m.update(Input::Start);
    m.update(Input::Connected(Ok(press())));
    // A failed list cleans nothing.
    let mut failed = m.clone();
    failed.update(Input::Listed(Err(err(ErrorKind::Busy))));
    assert_eq!(failed.labels.len(), 3);
    assert_eq!(failed.take_labels_to_save(), None);
    m.update(Input::Listed(Ok(vec![
        Finger::LeftThumb,
        Finger::RightThumb,
    ])));
    assert_eq!(m.labels, labels(&[(Finger::LeftThumb, "a")]));
    assert_eq!(
        m.take_labels_to_save(),
        Some(labels(&[(Finger::LeftThumb, "a")]))
    );
    // No enrolled prints at all: every name goes.
    m.update(Input::Listed(Err(err(ErrorKind::NoPrints))));
    assert!(m.labels.is_empty());
    assert_eq!(m.take_labels_to_save(), Some(Labels::new()));
}

#[test]
fn removing_a_fingerprint_removes_its_name() {
    let mut m = ready_named(
        &[Finger::LeftThumb, Finger::RightIndex],
        &[
            (Finger::LeftThumb, "Au lit"),
            (Finger::RightIndex, "Bureau"),
        ],
    );
    m.update(Input::RemoveFinger(Finger::LeftThumb));
    let (_, body, _, _) = m.dialog.as_ref().unwrap().texts(&m.labels);
    assert_eq!(
        en(&body),
        "\u{201c}Au lit\u{201d} will no longer unlock this computer. You can add it again later."
    );
    m.update(Input::Confirm);
    m.update(Input::Claimed(Ok(None)));
    m.update(Input::Deleted(Ok(())));
    m.update(Input::Released(Ok(())));
    assert_eq!(en(&m.take_notice().unwrap()), "Fingerprint removed: Au lit");
    assert_eq!(m.labels, labels(&[(Finger::RightIndex, "Bureau")]));
    assert_eq!(
        m.take_labels_to_save(),
        Some(labels(&[(Finger::RightIndex, "Bureau")]))
    );
    // A failed removal keeps the name.
    m.update(Input::RemoveFinger(Finger::RightIndex));
    m.update(Input::Confirm);
    m.update(Input::Claimed(Ok(None)));
    m.update(Input::Deleted(Err(err(ErrorKind::NotDeleted))));
    m.update(Input::Released(Ok(())));
    assert_eq!(m.labels.len(), 1);
    assert_eq!(m.take_labels_to_save(), None);
    assert!(matches!(m.dialog, Some(Dialog::Error(_))));
    m.update(Input::Confirm);
    // "Remove all" removes every name.
    m.update(Input::RemoveFinger(Finger::RightIndex));
    m.update(Input::Confirm);
    m.update(Input::Claimed(Ok(None)));
    m.update(Input::Deleted(Err(err(ErrorKind::Unsupported))));
    m.update(Input::Released(Ok(())));
    m.update(Input::Confirm);
    m.update(Input::Claimed(Ok(None)));
    m.update(Input::Deleted(Ok(())));
    m.update(Input::Released(Ok(())));
    assert!(m.labels.is_empty());
    assert_eq!(m.take_labels_to_save(), Some(Labels::new()));
}

#[test]
fn the_test_uses_the_name() {
    let mut m = ready_named(&[Finger::LeftThumb], &[(Finger::LeftThumb, "Au lit")]);
    m.update(Input::TestFinger);
    m.update(Input::Claimed(Ok(None)));
    m.update(Input::VerifyStarted(Ok(())));
    m.update(Input::VerifyFingerSelected("left-thumb".into()));
    assert_eq!(
        en(&test(&m).instruction),
        "Place the finger \u{201c}Au lit\u{201d} on the reader."
    );
    m.update(vstatus("verify-match", true));
    m.update(Input::VerifyStopped(Ok(())));
    m.update(Input::Released(Ok(())));
    let (title, _) = test(&m).outcome.as_ref().unwrap().texts(&m.labels).unwrap();
    assert_eq!(en(&title), "Recognised: Au lit");
    let fr = Catalogue::builtin("fr").unwrap();
    assert_eq!(title.render(&fr), "Reconnu\u{a0}: Au lit");
}
