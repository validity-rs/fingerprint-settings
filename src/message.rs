//! A user-visible message: an English msgid with its arguments, translated
//! when it is shown.

use crate::fingers::{Finger, in_sentence};
use crate::i18n::{self, Catalogue};

/// The value of a `{name}` placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    /// A number.
    Number(u32),
    /// A finger, capitalised as in a list ("Right index finger").
    Finger(Finger),
    /// A finger inside a sentence ("your right index finger").
    FingerInSentence(Finger),
}

/// A message to show: the msgid and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The English source string.
    pub id: &'static str,
    /// Its placeholders' values.
    pub args: Vec<(&'static str, Arg)>,
}

/// A message without arguments. Always called with a string literal: the
/// translation test collects them.
pub fn msg(id: &'static str) -> Message {
    Message {
        id,
        args: Vec::new(),
    }
}

impl Message {
    /// Adds the value of placeholder `{name}`.
    pub fn with(mut self, name: &'static str, value: Arg) -> Self {
        self.args.push((name, value));
        self
    }

    /// The text in the language of `cat`.
    pub fn render(&self, cat: &Catalogue) -> String {
        let values: Vec<(&str, String)> = self
            .args
            .iter()
            .map(|(name, arg)| {
                let v = match arg {
                    Arg::Number(n) => n.to_string(),
                    Arg::Finger(f) => cat.get(f.name()).to_string(),
                    Arg::FingerInSentence(f) => in_sentence(cat.get(f.name())),
                };
                (*name, v)
            })
            .collect();
        let refs: Vec<(&str, &str)> = values.iter().map(|(n, v)| (*n, v.as_str())).collect();
        i18n::fill(cat.get(self.id), &refs)
    }

    /// The text in the active language (see [`i18n::init`]).
    pub fn text(&self) -> String {
        let values: Vec<(&str, String)> = self
            .args
            .iter()
            .map(|(name, arg)| {
                let v = match arg {
                    Arg::Number(n) => n.to_string(),
                    Arg::Finger(f) => i18n::tr(f.name()).to_string(),
                    Arg::FingerInSentence(f) => in_sentence(i18n::tr(f.name())),
                };
                (*name, v)
            })
            .collect();
        let refs: Vec<(&str, &str)> = values.iter().map(|(n, v)| (*n, v.as_str())).collect();
        i18n::fill(i18n::tr(self.id), &refs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_in_english_and_french() {
        let m = msg("Place your {finger} flat on the reader.")
            .with("finger", Arg::FingerInSentence(Finger::RightIndex));
        assert_eq!(
            m.render(&Catalogue::default()),
            "Place your right index finger flat on the reader."
        );
        assert_eq!(
            m.render(&Catalogue::builtin("fr").unwrap()),
            "Posez votre index droit à plat sur le lecteur."
        );
        let s = msg("Step {step} of {total}")
            .with("step", Arg::Number(2))
            .with("total", Arg::Number(6));
        assert_eq!(s.render(&Catalogue::default()), "Step 2 of 6");
        assert_eq!(
            s.render(&Catalogue::builtin("fr").unwrap()),
            "Étape 2 sur 6"
        );
        let r = msg("Recognised: {finger}").with("finger", Arg::Finger(Finger::LeftThumb));
        assert_eq!(r.render(&Catalogue::default()), "Recognised: Left thumb");
        assert_eq!(
            r.render(&Catalogue::builtin("fr").unwrap()),
            "Reconnu\u{a0}: Pouce gauche"
        );
        // The active language is English in unit tests (never initialised).
        assert_eq!(r.text(), "Recognised: Left thumb");
    }
}
