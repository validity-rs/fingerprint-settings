//! fprintd's enrolment and verification results, and what to tell the user
//! about each.

use crate::fingers::Finger;
use crate::message::{Arg, Message, msg};

/// How the reader is used (the device's `scan-type` property).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanType {
    /// The finger is placed on the reader (`"press"`, and anything unknown).
    #[default]
    Press,
    /// The finger is swiped over the reader (`"swipe"`).
    Swipe,
}

impl ScanType {
    /// From the property value.
    pub fn parse(s: &str) -> ScanType {
        if s == "swipe" {
            ScanType::Swipe
        } else {
            ScanType::Press
        }
    }
}

/// An `EnrollStatus` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollResult {
    /// `enroll-completed`: done, the finger is enrolled.
    Completed,
    /// `enroll-failed`.
    Failed,
    /// `enroll-stage-passed`: one more good scan.
    StagePassed,
    /// `enroll-retry-scan`.
    RetryScan,
    /// `enroll-swipe-too-short`.
    SwipeTooShort,
    /// `enroll-too-fast`.
    TooFast,
    /// `enroll-finger-not-centered`.
    FingerNotCentered,
    /// `enroll-remove-and-retry`.
    RemoveAndRetry,
    /// `enroll-data-full`: no room left on the device.
    DataFull,
    /// `enroll-duplicate`: this print is already enrolled (maybe for someone else).
    Duplicate,
    /// `enroll-disconnected`: the device is gone.
    Disconnected,
    /// `enroll-unknown-error`.
    UnknownError,
    /// A result this application does not know (a newer service).
    Other(String),
}

impl EnrollResult {
    /// From the signal's `result` string.
    pub fn parse(s: &str) -> EnrollResult {
        match s {
            "enroll-completed" => EnrollResult::Completed,
            "enroll-failed" => EnrollResult::Failed,
            "enroll-stage-passed" => EnrollResult::StagePassed,
            "enroll-retry-scan" => EnrollResult::RetryScan,
            "enroll-swipe-too-short" => EnrollResult::SwipeTooShort,
            "enroll-too-fast" => EnrollResult::TooFast,
            "enroll-finger-not-centered" => EnrollResult::FingerNotCentered,
            "enroll-remove-and-retry" => EnrollResult::RemoveAndRetry,
            "enroll-data-full" => EnrollResult::DataFull,
            "enroll-duplicate" => EnrollResult::Duplicate,
            "enroll-disconnected" => EnrollResult::Disconnected,
            "enroll-unknown-error" => EnrollResult::UnknownError,
            other => EnrollResult::Other(other.to_string()),
        }
    }

    /// Whether this result ends the enrolment, whatever the signal's `done`
    /// says (fprintd always sets `done` for these).
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            EnrollResult::Completed
                | EnrollResult::Failed
                | EnrollResult::DataFull
                | EnrollResult::Duplicate
                | EnrollResult::Disconnected
                | EnrollResult::UnknownError
        )
    }

    /// What to ask of the user after this result while the enrolment goes on
    /// (`None` for final results).
    pub fn instruction(&self, scan: ScanType) -> Option<Message> {
        Some(match (self, scan) {
            (EnrollResult::StagePassed, ScanType::Press) => {
                msg("Good. Lift your finger, then place it on the reader again.")
            }
            (EnrollResult::StagePassed, ScanType::Swipe) => {
                msg("Good. Slide your finger over the reader again.")
            }
            (EnrollResult::RetryScan, _) => retry_scan(scan),
            (EnrollResult::SwipeTooShort, _) => swipe_too_short(),
            (EnrollResult::TooFast, _) => too_fast(scan),
            (EnrollResult::FingerNotCentered, _) => not_centered(),
            (EnrollResult::RemoveAndRetry, _) => remove_and_retry(),
            (EnrollResult::Other(_), _) => retry_scan(scan),
            _ => return None,
        })
    }

    /// Why the enrolment failed, for a final result other than `Completed`
    /// (`None` otherwise).
    pub fn failure(&self) -> Option<Message> {
        Some(match self {
            EnrollResult::Failed => msg("Adding this finger did not work. Please try again."),
            EnrollResult::DataFull => msg(
                "The reader's memory is full. Remove a fingerprint you no longer use, then try again.",
            ),
            EnrollResult::Duplicate => msg(
                "This fingerprint is already registered on this computer, perhaps as another finger or for another person. Choose a different finger.",
            ),
            EnrollResult::Disconnected => disconnected(),
            EnrollResult::UnknownError => unknown_error(),
            EnrollResult::Other(_) => unknown_error(),
            _ => return None,
        })
    }
}

/// A `VerifyStatus` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyResult {
    /// `verify-match`.
    Match,
    /// `verify-no-match`.
    NoMatch,
    /// `verify-retry-scan`.
    RetryScan,
    /// `verify-swipe-too-short`.
    SwipeTooShort,
    /// `verify-finger-not-centered`.
    FingerNotCentered,
    /// `verify-remove-and-retry`.
    RemoveAndRetry,
    /// `verify-too-fast`.
    TooFast,
    /// `verify-disconnected`.
    Disconnected,
    /// `verify-unknown-error`.
    UnknownError,
    /// A result this application does not know.
    Other(String),
}

impl VerifyResult {
    /// From the signal's `result` string.
    pub fn parse(s: &str) -> VerifyResult {
        match s {
            "verify-match" => VerifyResult::Match,
            "verify-no-match" => VerifyResult::NoMatch,
            "verify-retry-scan" => VerifyResult::RetryScan,
            "verify-swipe-too-short" => VerifyResult::SwipeTooShort,
            "verify-finger-not-centered" => VerifyResult::FingerNotCentered,
            "verify-remove-and-retry" => VerifyResult::RemoveAndRetry,
            "verify-too-fast" => VerifyResult::TooFast,
            "verify-disconnected" => VerifyResult::Disconnected,
            "verify-unknown-error" => VerifyResult::UnknownError,
            other => VerifyResult::Other(other.to_string()),
        }
    }

    /// Whether this result ends the verification.
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            VerifyResult::Match
                | VerifyResult::NoMatch
                | VerifyResult::Disconnected
                | VerifyResult::UnknownError
        )
    }

    /// What to ask of the user while the verification goes on (`None` for
    /// final results).
    pub fn instruction(&self, scan: ScanType) -> Option<Message> {
        Some(match self {
            VerifyResult::RetryScan => retry_scan(scan),
            VerifyResult::SwipeTooShort => swipe_too_short(),
            VerifyResult::FingerNotCentered => not_centered(),
            VerifyResult::RemoveAndRetry => remove_and_retry(),
            VerifyResult::TooFast => too_fast(scan),
            VerifyResult::Other(_) => retry_scan(scan),
            _ => return None,
        })
    }
}

fn retry_scan(scan: ScanType) -> Message {
    match scan {
        ScanType::Press => {
            msg("That did not work. Lift your finger, then place it on the reader again.")
        }
        ScanType::Swipe => msg("That did not work. Slide your finger over the reader again."),
    }
}

fn swipe_too_short() -> Message {
    msg("The swipe was too short. Slide your whole finger slowly over the reader.")
}

fn too_fast(scan: ScanType) -> Message {
    match scan {
        ScanType::Press => msg("Too quick. Leave your finger on the reader a little longer."),
        ScanType::Swipe => msg("Too quick. Slide your finger more slowly."),
    }
}

fn not_centered() -> Message {
    msg("Your finger was not in the middle of the reader. Place it in the middle and try again.")
}

fn remove_and_retry() -> Message {
    msg("Lift your finger completely off the reader, then place it again.")
}

/// The reader was unplugged during an operation.
pub fn disconnected() -> Message {
    msg("The fingerprint reader was disconnected. Check that it is plugged in, then try again.")
}

/// The driver reported an error it could not explain.
pub fn unknown_error() -> Message {
    msg(
        "The fingerprint reader had a problem. Please try again. If it happens again, restart the computer.",
    )
}

/// The first instruction of an enrolment: with the finger when the user
/// chose one, without when they only gave a name.
pub fn enroll_first(finger: Option<Finger>, scan: ScanType) -> Message {
    match (finger, scan) {
        (Some(f), ScanType::Press) => {
            msg("Place your {finger} flat on the reader.").with("finger", Arg::FingerInSentence(f))
        }
        (Some(f), ScanType::Swipe) => {
            msg("Slide your {finger} over the reader.").with("finger", Arg::FingerInSentence(f))
        }
        (None, ScanType::Press) => msg("Place your finger flat on the reader."),
        (None, ScanType::Swipe) => msg("Slide your finger over the reader."),
    }
}

/// The first instruction of a test, once the service said which
/// fingerprint it checks: its name (`None`: any enrolled one).
pub fn verify_first(selected: Option<Arg>, scan: ScanType) -> Message {
    match (selected, scan) {
        (Some(name), ScanType::Press) => {
            msg("Place the finger \u{201c}{name}\u{201d} on the reader.").with("name", name)
        }
        (Some(name), ScanType::Swipe) => {
            msg("Slide the finger \u{201c}{name}\u{201d} over the reader.").with("name", name)
        }
        (None, ScanType::Press) => msg("Place one of your registered fingers on the reader."),
        (None, ScanType::Swipe) => msg("Slide one of your registered fingers over the reader."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Catalogue;

    const ENROLL: [&str; 12] = [
        "enroll-completed",
        "enroll-failed",
        "enroll-stage-passed",
        "enroll-retry-scan",
        "enroll-swipe-too-short",
        "enroll-too-fast",
        "enroll-finger-not-centered",
        "enroll-remove-and-retry",
        "enroll-data-full",
        "enroll-duplicate",
        "enroll-disconnected",
        "enroll-unknown-error",
    ];

    const VERIFY: [&str; 9] = [
        "verify-match",
        "verify-no-match",
        "verify-retry-scan",
        "verify-swipe-too-short",
        "verify-finger-not-centered",
        "verify-remove-and-retry",
        "verify-too-fast",
        "verify-disconnected",
        "verify-unknown-error",
    ];

    #[test]
    fn scan_type() {
        assert_eq!(ScanType::parse("press"), ScanType::Press);
        assert_eq!(ScanType::parse("swipe"), ScanType::Swipe);
        assert_eq!(ScanType::parse(""), ScanType::Press);
        assert_eq!(ScanType::default(), ScanType::Press);
    }

    #[test]
    fn every_enroll_result_is_known_and_explained() {
        let fr = Catalogue::builtin("fr").unwrap();
        for s in ENROLL {
            let r = EnrollResult::parse(s);
            assert!(!matches!(r, EnrollResult::Other(_)), "{s}");
            for scan in [ScanType::Press, ScanType::Swipe] {
                match (r.instruction(scan), r.failure()) {
                    // Ongoing: an instruction, no failure, not final.
                    (Some(i), None) => {
                        assert!(!r.is_final(), "{s}");
                        assert_ne!(i.render(&fr), i.id, "{s}: untranslated");
                    }
                    // Final failure: a reason, no instruction.
                    (None, Some(f)) => {
                        assert!(r.is_final(), "{s}");
                        assert_ne!(f.render(&fr), f.id, "{s}: untranslated");
                    }
                    // Completed: neither.
                    (None, None) => assert_eq!(r, EnrollResult::Completed),
                    (Some(_), Some(_)) => panic!("{s}: both"),
                }
            }
        }
        // The ongoing ones, exactly.
        let ongoing: Vec<_> = ENROLL
            .iter()
            .filter(|s| !EnrollResult::parse(s).is_final())
            .copied()
            .collect();
        assert_eq!(
            ongoing,
            [
                "enroll-stage-passed",
                "enroll-retry-scan",
                "enroll-swipe-too-short",
                "enroll-too-fast",
                "enroll-finger-not-centered",
                "enroll-remove-and-retry"
            ]
        );
    }

    #[test]
    fn enroll_messages_exactly() {
        use EnrollResult as E;
        let en = Catalogue::default();
        let p = |r: E| r.instruction(ScanType::Press).unwrap().render(&en);
        let s = |r: E| r.instruction(ScanType::Swipe).unwrap().render(&en);
        assert_eq!(
            p(E::StagePassed),
            "Good. Lift your finger, then place it on the reader again."
        );
        assert_eq!(
            s(E::StagePassed),
            "Good. Slide your finger over the reader again."
        );
        assert_eq!(
            p(E::RetryScan),
            "That did not work. Lift your finger, then place it on the reader again."
        );
        assert_eq!(
            s(E::RetryScan),
            "That did not work. Slide your finger over the reader again."
        );
        assert_eq!(
            p(E::SwipeTooShort),
            "The swipe was too short. Slide your whole finger slowly over the reader."
        );
        assert_eq!(
            p(E::TooFast),
            "Too quick. Leave your finger on the reader a little longer."
        );
        assert_eq!(s(E::TooFast), "Too quick. Slide your finger more slowly.");
        assert_eq!(
            p(E::FingerNotCentered),
            "Your finger was not in the middle of the reader. Place it in the middle and try again."
        );
        assert_eq!(
            p(E::RemoveAndRetry),
            "Lift your finger completely off the reader, then place it again."
        );
        // Unknown ongoing results ask for another scan.
        assert_eq!(p(E::parse("enroll-something-new")), p(E::RetryScan));
        let f = |r: E| r.failure().unwrap().render(&en);
        assert_eq!(
            f(E::Failed),
            "Adding this finger did not work. Please try again."
        );
        assert!(f(E::DataFull).starts_with("The reader's memory is full."));
        assert!(f(E::Duplicate).starts_with("This fingerprint is already registered"));
        assert!(f(E::Disconnected).starts_with("The fingerprint reader was disconnected."));
        assert!(f(E::UnknownError).starts_with("The fingerprint reader had a problem."));
        assert_eq!(f(E::Other("x".into())), f(E::UnknownError));
        assert!(E::Completed.failure().is_none());
        assert!(E::StagePassed.failure().is_none());
        assert!(!E::Other("x".into()).is_final());
    }

    #[test]
    fn every_verify_result_is_known_and_explained() {
        let fr = Catalogue::builtin("fr").unwrap();
        for s in VERIFY {
            let r = VerifyResult::parse(s);
            assert!(!matches!(r, VerifyResult::Other(_)), "{s}");
            for scan in [ScanType::Press, ScanType::Swipe] {
                match r.instruction(scan) {
                    Some(i) => {
                        assert!(!r.is_final(), "{s}");
                        assert_ne!(i.render(&fr), i.id, "{s}");
                    }
                    None => assert!(r.is_final(), "{s}"),
                }
            }
        }
        let finals: Vec<_> = VERIFY
            .iter()
            .filter(|s| VerifyResult::parse(s).is_final())
            .copied()
            .collect();
        assert_eq!(
            finals,
            [
                "verify-match",
                "verify-no-match",
                "verify-disconnected",
                "verify-unknown-error"
            ]
        );
        let other = VerifyResult::parse("verify-new");
        assert_eq!(other, VerifyResult::Other("verify-new".into()));
        assert!(!other.is_final());
        assert_eq!(
            other.instruction(ScanType::Press),
            VerifyResult::RetryScan.instruction(ScanType::Press)
        );
    }

    #[test]
    fn verify_and_enroll_share_retry_wording() {
        for scan in [ScanType::Press, ScanType::Swipe] {
            for (e, v) in [
                (EnrollResult::RetryScan, VerifyResult::RetryScan),
                (EnrollResult::SwipeTooShort, VerifyResult::SwipeTooShort),
                (EnrollResult::TooFast, VerifyResult::TooFast),
                (
                    EnrollResult::FingerNotCentered,
                    VerifyResult::FingerNotCentered,
                ),
                (EnrollResult::RemoveAndRetry, VerifyResult::RemoveAndRetry),
            ] {
                assert_eq!(e.instruction(scan), v.instruction(scan));
            }
        }
    }

    #[test]
    fn first_instructions() {
        let en = Catalogue::default();
        let fr = Catalogue::builtin("fr").unwrap();
        let m = enroll_first(Some(Finger::RightIndex), ScanType::Press);
        assert_eq!(
            m.render(&en),
            "Place your right index finger flat on the reader."
        );
        assert_eq!(
            m.render(&fr),
            "Posez votre index droit à plat sur le lecteur."
        );
        let m = enroll_first(Some(Finger::LeftLittle), ScanType::Swipe);
        assert_eq!(
            m.render(&en),
            "Slide your left little finger over the reader."
        );
        assert_eq!(
            m.render(&fr),
            "Faites glisser votre petit doigt gauche sur le lecteur."
        );
        // Only a name: no finger to name.
        let m = enroll_first(None, ScanType::Press);
        assert_eq!(m.render(&en), "Place your finger flat on the reader.");
        assert_eq!(m.render(&fr), "Posez votre doigt à plat sur le lecteur.");
        assert_eq!(
            enroll_first(None, ScanType::Swipe).render(&fr),
            "Faites glisser votre doigt sur le lecteur."
        );
        let named = Arg::Text("Pouce, quand je suis au lit".into());
        assert_eq!(
            verify_first(Some(named.clone()), ScanType::Press).render(&fr),
            "Posez le doigt «\u{a0}Pouce, quand je suis au lit\u{a0}» sur le lecteur."
        );
        assert_eq!(
            verify_first(Some(named), ScanType::Swipe).render(&en),
            "Slide the finger \u{201c}Pouce, quand je suis au lit\u{201d} over the reader."
        );
        assert_eq!(
            verify_first(Some(Arg::Finger(Finger::RightThumb)), ScanType::Press).render(&en),
            "Place the finger \u{201c}Right thumb\u{201d} on the reader."
        );
        assert_eq!(
            verify_first(None, ScanType::Press).render(&en),
            "Place one of your registered fingers on the reader."
        );
        assert_eq!(
            verify_first(None, ScanType::Swipe).render(&fr),
            "Faites glisser l'un de vos doigts enregistrés sur le lecteur."
        );
    }
}
