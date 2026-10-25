use super::*;
use crate::i18n::Catalogue;

fn err(kind: ErrorKind) -> ServiceError {
    ServiceError::new(kind, "detail")
}

fn fr(m: &Message) -> String {
    m.render(&Catalogue::builtin("fr").unwrap())
}

/// Started and listed: connected, with `enrolled` fingers.
fn ready(enrolled: &[&str]) -> Model {
    let mut m = Model::new();
    assert_eq!(m.update(Input::Start), [Command::Connect]);
    assert_eq!(m.update(Input::Connected(Ok(()))), [Command::List]);
    assert_eq!(
        m.update(Input::Listed(Ok(enrolled
            .iter()
            .map(|s| s.to_string())
            .collect()))),
        []
    );
    assert_eq!(m.activity, Activity::Idle);
    m
}

fn enrolling(m: &Model) -> &Enroll {
    match &m.activity {
        Activity::Enrolling(e) => e,
        a => panic!("not enrolling: {a:?}"),
    }
}

/// Enrolment up to the first scan, with six stages.
fn scanning() -> Model {
    let mut m = ready(&[]);
    assert_eq!(m.update(Input::Enroll), [Command::Claim]);
    assert_eq!(
        fr(&enrolling(&m).instruction),
        "Préparation du lecteur, un instant…"
    );
    assert_eq!(
        m.update(Input::Claimed(Ok(Some(6)))),
        [Command::EnrollStart]
    );
    assert_eq!(m.update(Input::EnrollStarted(Ok(()))), []);
    assert_eq!(enrolling(&m).step, Step::Scanning);
    m
}

fn status(s: &str, done: bool) -> Input {
    Input::EnrollStatus(s.into(), done)
}

#[test]
fn start_reads_the_status() {
    let m = ready(&[]);
    assert_eq!(m.enrolled, Some(false));
    assert_eq!(
        fr(&m.status().unwrap()),
        "Aucune empreinte n'est encore enregistrée."
    );
    assert!(m.can_enroll() && !m.shows_remove() && !m.busy());
    // Another finger, enrolled with another tool, is not the right index.
    let m = ready(&["left-thumb"]);
    assert_eq!(m.enrolled, Some(false));
    let m = ready(&["left-thumb", FINGER]);
    assert_eq!(m.enrolled, Some(true));
    assert_eq!(
        fr(&m.status().unwrap()),
        "Votre index droit est enregistré."
    );
    assert!(!m.can_enroll() && m.shows_remove() && m.can_remove());
    // Not known before the list.
    let mut m = Model::default();
    assert_eq!(m.status(), None);
    assert!(m.busy() && !m.holds_reader());
    m.update(Input::Start);
    assert!(!m.can_enroll());
    // No prints is no prints, not an error.
    m.update(Input::Connected(Ok(())));
    m.update(Input::Listed(Err(err(ErrorKind::NoPrints))));
    assert_eq!((m.enrolled, &m.message), (Some(false), &None));
}

#[test]
fn enroll_with_progress() {
    let mut m = scanning();
    assert_eq!(
        fr(&enrolling(&m).instruction),
        "Posez votre index droit sur le lecteur."
    );
    assert_eq!(enrolling(&m).fraction(), 0.0);
    assert!(m.holds_reader() && !m.can_enroll());
    // Good scans fill the indicator; the instruction never counts them.
    for n in 1..=5 {
        assert_eq!(m.update(status("enroll-stage-passed", false)), []);
        let e = enrolling(&m);
        assert_eq!(fr(&e.instruction), "Relevez le doigt, puis reposez-le.");
        assert_eq!(e.fraction(), f64::from(n) / 6.0);
    }
    // Scans that did not take: the same neutral hint, no blame, and the
    // indicator does not move.
    for s in [
        "enroll-retry-scan",
        "enroll-swipe-too-short",
        "enroll-too-fast",
        "enroll-finger-not-centered",
        "enroll-remove-and-retry",
        "enroll-something-new",
    ] {
        assert_eq!(m.update(status(s, false)), [], "{s}");
        let e = enrolling(&m);
        assert_eq!(
            fr(&e.instruction),
            "Relevez le doigt, puis reposez-le.",
            "{s}"
        );
        assert_eq!((e.fraction(), e.outcome.clone()), (5.0 / 6.0, None), "{s}");
    }
    // A stage beyond the count does not go past the last one.
    m.update(status("enroll-stage-passed", false));
    assert_eq!(enrolling(&m).passed, 5);
    assert_eq!(enrolling(&m).fraction(), 5.0 / 6.0);
    assert_eq!(
        m.update(status("enroll-completed", true)),
        [Command::EnrollStop]
    );
    let e = enrolling(&m);
    assert_eq!((e.outcome.clone(), e.passed), (Some(Outcome::Completed), 6));
    assert_eq!(e.fraction(), 1.0);
    assert!(!e.cancellable());
    assert_eq!(m.update(Input::EnrollStopped(Ok(()))), [Command::Release]);
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(m.activity, Activity::Idle);
    assert_eq!(m.enrolled, Some(true));
    assert_eq!(fr(m.message.as_ref().unwrap()), "Enregistrement terminé.");
    assert_eq!(m.update(Input::Listed(Ok(vec![FINGER.into()]))), []);
    assert!(m.can_remove() && !m.can_enroll());
}

#[test]
fn unknown_stage_count() {
    let mut m = ready(&[]);
    m.update(Input::Enroll);
    m.update(Input::Claimed(Ok(None)));
    m.update(Input::EnrollStarted(Ok(())));
    assert_eq!(enrolling(&m).fraction(), 0.0);
    // Each good scan fills a quarter of what is left: forward, never full.
    let mut last = 0.0;
    for want in [0.25, 0.4375, 0.578125] {
        m.update(status("enroll-stage-passed", false));
        let e = enrolling(&m);
        assert_eq!(fr(&e.instruction), "Relevez le doigt, puis reposez-le.");
        assert_eq!(e.fraction(), want);
        assert!(e.fraction() > last);
        last = e.fraction();
    }
    // A scan that did not take leaves it where it is.
    m.update(status("enroll-retry-scan", false));
    assert_eq!(enrolling(&m).fraction(), last);
    for _ in 0..100 {
        m.update(status("enroll-stage-passed", false));
    }
    assert!(enrolling(&m).fraction() < 1.0);
    m.update(status("enroll-completed", true));
    assert_eq!(enrolling(&m).fraction(), 1.0);
}

#[test]
fn every_final_failure_releases_and_says_why() {
    for (s, want) in [
        (
            "enroll-duplicate",
            "Ce doigt est déjà enregistré (peut-être sur un autre compte).",
        ),
        (
            "enroll-data-full",
            "La mémoire du lecteur est pleine : il ne peut pas enregistrer d'autre empreinte.",
        ),
        ("enroll-failed", "L'enregistrement a échoué. Réessayez."),
        (
            "enroll-unknown-error",
            "Le lecteur d'empreintes a eu un problème. Réessayez.",
        ),
        // A new result, final because done.
        (
            "enroll-new-thing",
            "Le lecteur d'empreintes a eu un problème. Réessayez.",
        ),
    ] {
        let mut m = scanning();
        assert_eq!(m.update(status(s, true)), [Command::EnrollStop], "{s}");
        assert_eq!(m.update(Input::EnrollStopped(Ok(()))), [Command::Release]);
        assert_eq!(m.update(Input::Released(Ok(()))), []);
        assert_eq!(m.activity, Activity::Idle, "{s}");
        assert_eq!(fr(m.message.as_ref().unwrap()), want, "{s}");
        assert_eq!(m.enrolled, Some(false));
        // Still usable: enrol again.
        assert_eq!(m.update(Input::Enroll), [Command::Claim], "{s}");
        assert_eq!(m.message, None);
    }
    // Disconnected: no stop, only the release.
    let mut m = scanning();
    assert_eq!(
        m.update(status("enroll-disconnected", true)),
        [Command::Release]
    );
    m.update(Input::Released(Err(err(ErrorKind::NoReader))));
    assert_eq!(
        fr(m.message.as_ref().unwrap()),
        "Le lecteur d'empreintes ne répond plus. Réessayez."
    );
}

#[test]
fn service_errors_become_sentences() {
    // Claim refused: nothing claimed, nothing to release.
    for (k, want) in [
        (
            ErrorKind::Busy,
            "Le lecteur est utilisé par un autre programme. Réessayez dans un instant.",
        ),
        (
            ErrorKind::PermissionDenied,
            "L'autorisation a été refusée. Réessayez, et tapez votre mot de passe si l'ordinateur le demande.",
        ),
        (
            ErrorKind::NoReader,
            "Aucun lecteur d'empreintes n'a été trouvé sur cet ordinateur.",
        ),
    ] {
        let mut m = ready(&[]);
        m.update(Input::Enroll);
        assert_eq!(m.update(Input::Claimed(Err(err(k)))), [], "{k:?}");
        assert_eq!(m.activity, Activity::Idle);
        assert_eq!(fr(m.message.as_ref().unwrap()), want);
    }
    // A claim that timed out may have happened: released.
    let mut m = ready(&[]);
    m.update(Input::Enroll);
    assert_eq!(
        m.update(Input::Claimed(Err(err(ErrorKind::Timeout)))),
        [Command::Release]
    );
    assert_eq!(m.update(Input::Released(Ok(()))), []);
    assert_eq!(
        fr(m.message.as_ref().unwrap()),
        "Le programme des empreintes ne répond pas. Réessayez."
    );
    // polkit refuses the start: released.
    let mut m = ready(&[]);
    m.update(Input::Enroll);
    m.update(Input::Claimed(Ok(Some(6))));
    assert_eq!(
        m.update(Input::EnrollStarted(Err(err(ErrorKind::PermissionDenied)))),
        [Command::Release]
    );
    m.update(Input::Released(Ok(())));
    assert_eq!(
        fr(m.message.as_ref().unwrap()),
        "L'autorisation a été refusée. Réessayez, et tapez votre mot de passe si l'ordinateur le demande."
    );
    assert!(m.can_enroll());
}

#[test]
fn no_reader_at_start_then_enroll_looks_again() {
    let mut m = Model::new();
    m.update(Input::Start);
    assert_eq!(
        m.update(Input::Connected(Err(err(ErrorKind::NoReader)))),
        []
    );
    assert_eq!(m.activity, Activity::Idle);
    assert_eq!(
        fr(m.message.as_ref().unwrap()),
        "Aucun lecteur d'empreintes n'a été trouvé sur cet ordinateur."
    );
    assert_eq!(m.status(), None);
    // The button stays usable: it looks for the reader again.
    assert!(m.can_enroll());
    assert_eq!(m.update(Input::Enroll), [Command::Connect]);
    assert_eq!(m.activity, Activity::Loading { then_enroll: true });
    assert_eq!(m.message, None);
    // Still nothing.
    m.update(Input::Connected(Err(err(ErrorKind::Unavailable))));
    assert_eq!(
        fr(m.message.as_ref().unwrap()),
        "Le programme des empreintes ne fonctionne pas. Redémarrez l'ordinateur, puis réessayez."
    );
    // Plugged in: found, listed, and the enrolment starts.
    m.update(Input::Enroll);
    assert_eq!(m.update(Input::Connected(Ok(()))), [Command::List]);
    assert_eq!(m.update(Input::Listed(Ok(vec![]))), [Command::Claim]);
    assert_eq!(enrolling(&m).step, Step::Claiming);
    // Unless the finger turns out to be enrolled already.
    let mut m = Model::new();
    m.update(Input::Start);
    m.update(Input::Connected(Err(err(ErrorKind::NoReader))));
    m.update(Input::Enroll);
    m.update(Input::Connected(Ok(())));
    assert_eq!(m.update(Input::Listed(Ok(vec![FINGER.into()]))), []);
    assert_eq!(
        (m.activity.clone(), m.enrolled),
        (Activity::Idle, Some(true))
    );
    // A failed list at start: said, and the app stays usable.
    let mut m = Model::new();
    m.update(Input::Start);
    m.update(Input::Connected(Ok(())));
    m.update(Input::Listed(Err(err(ErrorKind::PermissionDenied))));
    assert_eq!(
        fr(m.message.as_ref().unwrap()),
        "L'autorisation a été refusée. Réessayez, et tapez votre mot de passe si l'ordinateur le demande."
    );
    assert_eq!(m.enrolled, None);
    assert_eq!(m.update(Input::Enroll), [Command::Claim]);
}

#[test]
fn cancel_at_every_step_releases() {
    let said = |m: &Model| fr(m.message.as_ref().unwrap());
    // While claiming: the claim's answer is released.
    let mut m = ready(&[]);
    m.update(Input::Enroll);
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(m.update(Input::Claimed(Ok(Some(6)))), [Command::Release]);
    assert_eq!(m.update(Input::Released(Ok(()))), []);
    assert_eq!(said(&m), "Enregistrement annulé.");
    // Claiming, and the claim fails: nothing to release.
    let mut m = ready(&[]);
    m.update(Input::Enroll);
    m.update(Input::Cancel);
    assert_eq!(m.update(Input::Claimed(Err(err(ErrorKind::Busy)))), []);
    assert_eq!(
        (m.activity.clone(), said(&m)),
        (Activity::Idle, "Enregistrement annulé.".into())
    );
    // While starting: stopped once started.
    let mut m = ready(&[]);
    m.update(Input::Enroll);
    m.update(Input::Claimed(Ok(Some(6))));
    assert!(enrolling(&m).cancellable());
    m.update(Input::Cancel);
    assert!(!enrolling(&m).cancellable());
    // A final status before the reply changes nothing.
    assert_eq!(m.update(status("enroll-completed", true)), []);
    assert_eq!(
        m.update(Input::EnrollStarted(Ok(()))),
        [Command::EnrollStop]
    );
    assert_eq!(m.update(Input::EnrollStopped(Ok(()))), [Command::Release]);
    m.update(Input::Released(Ok(())));
    assert_eq!(said(&m), "Enregistrement annulé.");
    // Starting, and the start fails: released.
    let mut m = ready(&[]);
    m.update(Input::Enroll);
    m.update(Input::Claimed(Ok(Some(6))));
    m.update(Input::Cancel);
    assert_eq!(
        m.update(Input::EnrollStarted(Err(err(ErrorKind::Failed)))),
        [Command::Release]
    );
    // While scanning.
    let mut m = scanning();
    assert_eq!(m.update(Input::Cancel), [Command::EnrollStop]);
    // A second cancel, and late inputs, do nothing.
    assert_eq!(m.update(Input::Cancel), []);
    assert_eq!(m.update(status("enroll-stage-passed", false)), []);
    assert_eq!(m.update(Input::Enroll), []);
    // A failed stop still releases.
    assert_eq!(
        m.update(Input::EnrollStopped(Err(err(ErrorKind::Failed)))),
        [Command::Release]
    );
    assert_eq!(m.update(Input::Cancel), []);
    m.update(Input::Released(Err(err(ErrorKind::Failed))));
    assert_eq!(said(&m), "Enregistrement annulé.");
    assert!(!m.busy());
}

#[test]
fn completed_while_starting_stops_after_the_reply() {
    let mut m = ready(&[]);
    m.update(Input::Enroll);
    m.update(Input::Claimed(Ok(Some(1))));
    assert_eq!(m.update(status("enroll-completed", true)), []);
    assert_eq!(
        m.update(Input::EnrollStarted(Ok(()))),
        [Command::EnrollStop]
    );
    m.update(Input::EnrollStopped(Ok(())));
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(m.enrolled, Some(true));
}

#[test]
fn remove() {
    let mut m = ready(&[FINGER]);
    assert_eq!(m.update(Input::Remove), [Command::Claim]);
    assert!(m.holds_reader() && !m.can_remove());
    assert_eq!(m.update(Input::Remove), []);
    assert_eq!(
        m.update(Input::Claimed(Ok(Some(6)))),
        [Command::DeleteFinger]
    );
    assert_eq!(m.update(Input::Deleted(Ok(()))), [Command::Release]);
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(m.enrolled, Some(false));
    assert_eq!(
        fr(m.message.as_ref().unwrap()),
        "L'empreinte a été supprimée."
    );
    assert!(!m.shows_remove() && m.can_enroll());
    // Not offered when nothing is enrolled.
    assert_eq!(m.update(Input::Remove), []);
}

#[test]
fn remove_falls_back_to_delete_all_on_older_services() {
    let mut m = ready(&[FINGER]);
    m.update(Input::Remove);
    m.update(Input::Claimed(Ok(None)));
    assert_eq!(
        m.update(Input::Deleted(Err(err(ErrorKind::Unsupported)))),
        [Command::DeleteAll]
    );
    // Without DeleteEnrolledFingers2 either: an error, released.
    assert_eq!(
        m.update(Input::Deleted(Err(err(ErrorKind::Unsupported)))),
        [Command::Release]
    );
    m.update(Input::Released(Ok(())));
    assert_eq!(
        fr(m.message.as_ref().unwrap()),
        "Le programme des empreintes a eu un problème. Réessayez."
    );
    assert_eq!(m.enrolled, Some(true));
    let mut m = ready(&[FINGER]);
    m.update(Input::Remove);
    m.update(Input::Claimed(Ok(None)));
    m.update(Input::Deleted(Err(err(ErrorKind::Unsupported))));
    assert_eq!(m.update(Input::Deleted(Ok(()))), [Command::Release]);
    m.update(Input::Released(Ok(())));
    assert_eq!(m.enrolled, Some(false));
}

#[test]
fn remove_errors() {
    for (k, want) in [
        (
            ErrorKind::PermissionDenied,
            "L'autorisation a été refusée. Réessayez, et tapez votre mot de passe si l'ordinateur le demande.",
        ),
        (
            ErrorKind::NotDeleted,
            "L'empreinte n'a pas pu être supprimée. Réessayez.",
        ),
    ] {
        let mut m = ready(&[FINGER]);
        m.update(Input::Remove);
        m.update(Input::Claimed(Ok(None)));
        assert_eq!(m.update(Input::Deleted(Err(err(k)))), [Command::Release]);
        assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
        assert_eq!(fr(m.message.as_ref().unwrap()), want);
        assert!(m.can_remove());
    }
    // The claim refused: nothing to release.
    let mut m = ready(&[FINGER]);
    m.update(Input::Remove);
    assert_eq!(m.update(Input::Claimed(Err(err(ErrorKind::Busy)))), []);
    assert!(m.can_remove());
    // Timed out: released.
    m.update(Input::Remove);
    assert_eq!(
        m.update(Input::Claimed(Err(err(ErrorKind::Timeout)))),
        [Command::Release]
    );
    assert_eq!(m.update(Input::Released(Ok(()))), [Command::List]);
    assert_eq!(
        fr(m.message.as_ref().unwrap()),
        "Le programme des empreintes ne répond pas. Réessayez."
    );
    // A refresh that fails changes nothing.
    assert_eq!(m.update(Input::Listed(Err(err(ErrorKind::Failed)))), []);
    assert_eq!(m.enrolled, Some(true));
}

#[test]
fn service_lost() {
    let lost =
        "Le programme des empreintes ne fonctionne pas. Redémarrez l'ordinateur, puis réessayez.";
    // Mid-enrolment: over, nothing to release.
    let mut m = scanning();
    assert_eq!(m.update(Input::ServiceLost), []);
    assert_eq!(
        (m.activity.clone(), fr(m.message.as_ref().unwrap())),
        (Activity::Idle, lost.into())
    );
    assert_eq!(m.enrolled, Some(false));
    // After completion: the print is stored.
    let mut m = scanning();
    m.update(status("enroll-completed", true));
    m.update(Input::ServiceLost);
    assert_eq!(m.enrolled, Some(true));
    assert_eq!(fr(m.message.as_ref().unwrap()), "Enregistrement terminé.");
    // After a cancel, or a failure: said as such.
    let mut m = scanning();
    m.update(Input::Cancel);
    m.update(Input::ServiceLost);
    assert_eq!(fr(m.message.as_ref().unwrap()), "Enregistrement annulé.");
    let mut m = scanning();
    m.update(status("enroll-duplicate", true));
    m.update(Input::ServiceLost);
    assert!(fr(m.message.as_ref().unwrap()).starts_with("Ce doigt est déjà enregistré"));
    // Removing.
    let mut m = ready(&[FINGER]);
    m.update(Input::Remove);
    m.update(Input::ServiceLost);
    assert_eq!(fr(m.message.as_ref().unwrap()), lost);
    assert!(m.can_remove());
    // Idle: fprintd exits when unused; nothing changes.
    let mut m = ready(&[FINGER]);
    m.update(Input::ServiceLost);
    assert_eq!(m, ready(&[FINGER]));
    // While loading: the connect's answer decides.
    let mut m = Model::new();
    m.update(Input::Start);
    m.update(Input::ServiceLost);
    assert_eq!(m.activity, Activity::Loading { then_enroll: false });
}

#[test]
fn late_and_stray_inputs_are_ignored() {
    let mut m = ready(&[]);
    let before = m.clone();
    for i in [
        Input::Cancel,
        Input::Remove,
        Input::Claimed(Ok(Some(3))),
        Input::Released(Ok(())),
        Input::EnrollStarted(Ok(())),
        status("enroll-completed", true),
        Input::EnrollStopped(Ok(())),
        Input::Deleted(Ok(())),
        Input::Connected(Ok(())),
    ] {
        assert_eq!(m.update(i.clone()), [], "{i:?}");
        assert_eq!(m, before, "{i:?}");
    }
    // While loading, user actions wait.
    let mut m = Model::new();
    m.update(Input::Start);
    assert_eq!(m.update(Input::Enroll), []);
    assert_eq!(m.update(Input::Remove), []);
    // Start during an enrolment does not drop the claim.
    let mut m = scanning();
    assert_eq!(m.update(Input::Start), []);
    assert!(m.holds_reader());
    // Enrol is not offered once enrolled.
    let mut m = ready(&[FINGER]);
    assert_eq!(m.update(Input::Enroll), []);
    // Start again from idle: a fresh look.
    assert_eq!(m.update(Input::Start), [Command::Connect]);
}

#[test]
fn help_for_a_first_time_user() {
    let where_ = "Le lecteur d'empreintes est la petite surface de l'ordinateur où l'on pose le doigt. Il s'allume quand il attend votre doigt.";
    let how = "Posez le doigt à plat sur le lecteur quand il est allumé, puis relevez-le. Recommencez en bougeant un peu le doigt à chaque fois, jusqu'à ce que la barre soit pleine.";
    let help = |m: &Model| m.help().map(|h| fr(&h));
    // Not before the reader is found.
    let mut m = Model::new();
    assert_eq!(help(&m), None);
    m.update(Input::Start);
    assert_eq!(help(&m), None);
    // Before registering: where the reader is.
    let mut m = ready(&[]);
    assert_eq!(help(&m).as_deref(), Some(where_));
    // While registering, from the first moment: what to do.
    m.update(Input::Enroll);
    assert_eq!(help(&m).as_deref(), Some(how));
    m.update(Input::Claimed(Ok(Some(6))));
    m.update(Input::EnrollStarted(Ok(())));
    m.update(status("enroll-retry-scan", false));
    assert_eq!(help(&m).as_deref(), Some(how));
    // Once it is over, while the reader is let go: nothing more to do.
    m.update(status("enroll-completed", true));
    assert_eq!(help(&m), None);
    m.update(Input::EnrollStopped(Ok(())));
    m.update(Input::Released(Ok(())));
    // Registered: no more help, "Remove the fingerprint" is shown.
    m.update(Input::Listed(Ok(vec![FINGER.into()])));
    assert_eq!(help(&m), None);
    assert_eq!(help(&ready(&[FINGER])), None);
    // Cancelled: the screen is back to its start, with where the reader is.
    let mut m = scanning();
    m.update(Input::Cancel);
    assert_eq!(help(&m), None);
    m.update(Input::EnrollStopped(Ok(())));
    m.update(Input::Released(Ok(())));
    assert_eq!(help(&m).as_deref(), Some(where_));
}
