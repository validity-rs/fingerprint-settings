//! A user-visible message: an English msgid with its numbers, translated
//! when it is shown.

use crate::i18n::{self, Catalogue};

/// A message to show: the msgid and the values of its `{name}` placeholders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The English source string.
    pub id: &'static str,
    /// Its placeholders' values.
    pub args: Vec<(&'static str, u32)>,
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
    pub fn with(mut self, name: &'static str, value: u32) -> Self {
        self.args.push((name, value));
        self
    }

    fn fill(&self, template: &str) -> String {
        let values: Vec<(&str, String)> = self
            .args
            .iter()
            .map(|(name, n)| (*name, n.to_string()))
            .collect();
        let refs: Vec<(&str, &str)> = values.iter().map(|(n, v)| (*n, v.as_str())).collect();
        i18n::fill(template, &refs)
    }

    /// The text in the language of `cat`.
    pub fn render(&self, cat: &Catalogue) -> String {
        self.fill(cat.get(self.id))
    }

    /// The text in the active language (see [`i18n::init`]).
    pub fn text(&self) -> String {
        self.fill(i18n::tr(self.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_in_english_and_french() {
        let fr = Catalogue::from_po(
            "msgid \"{n} left\"\nmsgstr \"encore {n}\"\n\nmsgid \"Remove\"\nmsgstr \"Supprimer\"\n",
        )
        .unwrap();
        let m = msg("{n} left").with("n", 2);
        assert_eq!(m.render(&Catalogue::default()), "2 left");
        assert_eq!(m.render(&fr), "encore 2");
        // The active language is English in unit tests (never initialised).
        assert_eq!(m.text(), "2 left");
        assert_eq!(msg("Remove").text(), "Remove");
        assert_eq!(msg("Remove").render(&fr), "Supprimer");
        assert_eq!(
            msg("Remove").render(&Catalogue::builtin("fr").unwrap()),
            "Supprimer"
        );
    }
}
