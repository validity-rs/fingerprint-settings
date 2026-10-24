//! fprintd's enrolment results, and what to tell the user about each.

use crate::message::{Message, msg};

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

    /// What to ask of the user after this result while the enrolment goes
    /// on, with the good scans so far and the number needed when the device
    /// says (`None` for final results).
    pub fn instruction(&self, passed: u32, stages: Option<u32>) -> Option<Message> {
        Some(match self {
            EnrollResult::StagePassed => match stages {
                Some(total) => msg("Again… {passed}/{total}")
                    .with("passed", passed)
                    .with("total", total),
                None => msg("Again…"),
            },
            EnrollResult::RetryScan
            | EnrollResult::SwipeTooShort
            | EnrollResult::TooFast
            | EnrollResult::FingerNotCentered
            | EnrollResult::RemoveAndRetry
            | EnrollResult::Other(_) => misread(),
            _ => return None,
        })
    }

    /// Why the enrolment failed, for a final result other than `Completed`
    /// (`None` otherwise).
    pub fn failure(&self) -> Option<Message> {
        Some(match self {
            EnrollResult::Failed => msg("Registration failed. Try again."),
            EnrollResult::DataFull => msg("The reader's memory is full."),
            EnrollResult::Duplicate => {
                msg("This finger is already registered (perhaps on another account).")
            }
            EnrollResult::Disconnected => msg("The fingerprint reader was disconnected."),
            EnrollResult::UnknownError | EnrollResult::Other(_) => unknown_error(),
            _ => return None,
        })
    }
}

/// A scan that did not take: the user places the finger again.
pub fn misread() -> Message {
    msg("Poorly read, place your finger again.")
}

/// The driver reported an error it could not explain.
pub fn unknown_error() -> Message {
    msg("The fingerprint reader had a problem. Try again.")
}

/// The first instruction of an enrolment.
pub fn enroll_first() -> Message {
    msg("Place your finger…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Catalogue;

    /// Every result fprintd sends, and exactly what the user reads in
    /// French: an instruction while it goes on, a reason when it fails.
    #[test]
    fn every_status_has_its_french_sentence() {
        let fr = Catalogue::builtin("fr").unwrap();
        let said = |s: &str| {
            let r = EnrollResult::parse(s);
            assert!(!matches!(r, EnrollResult::Other(_)), "{s}");
            match (r.instruction(2, Some(6)), r.failure()) {
                (Some(i), None) => {
                    assert!(!r.is_final(), "{s}");
                    Some(i.render(&fr))
                }
                (None, Some(f)) => {
                    assert!(r.is_final(), "{s}");
                    Some(f.render(&fr))
                }
                (None, None) => {
                    assert!(r.is_final(), "{s}");
                    None
                }
                (Some(_), Some(_)) => panic!("{s}: both"),
            }
        };
        let misread = Some("Mal lu, reposez le doigt.".to_string());
        let cases = [
            ("enroll-completed", None),
            (
                "enroll-failed",
                Some("L'enregistrement a échoué. Réessayez.".to_string()),
            ),
            ("enroll-stage-passed", Some("Encore… 2/6".to_string())),
            ("enroll-retry-scan", misread.clone()),
            ("enroll-swipe-too-short", misread.clone()),
            ("enroll-too-fast", misread.clone()),
            ("enroll-finger-not-centered", misread.clone()),
            ("enroll-remove-and-retry", misread.clone()),
            (
                "enroll-data-full",
                Some("La mémoire du lecteur est pleine.".to_string()),
            ),
            (
                "enroll-duplicate",
                Some("Ce doigt est déjà enregistré (peut-être sur un autre compte).".to_string()),
            ),
            (
                "enroll-disconnected",
                Some("Le lecteur d'empreintes a été débranché.".to_string()),
            ),
            (
                "enroll-unknown-error",
                Some("Le lecteur d'empreintes a eu un problème. Réessayez.".to_string()),
            ),
        ];
        for (s, want) in cases {
            assert_eq!(said(s), want, "{s}");
        }
        assert_eq!(enroll_first().render(&fr), "Posez le doigt…");
        // The device does not say how many scans it needs.
        assert_eq!(
            EnrollResult::StagePassed
                .instruction(1, None)
                .unwrap()
                .render(&fr),
            "Encore…"
        );
        // A result from a newer service: ongoing, the finger is placed again;
        // final (done), an error the user can read.
        let other = EnrollResult::parse("enroll-something-new");
        assert_eq!(other, EnrollResult::Other("enroll-something-new".into()));
        assert!(!other.is_final());
        assert_eq!(other.instruction(0, None), Some(super::misread()));
        assert_eq!(other.failure(), Some(unknown_error()));
        assert!(EnrollResult::Completed.failure().is_none());
        assert!(EnrollResult::StagePassed.failure().is_none());
    }
}
