//! The ten fingers, with fprintd's names for them and their display names.

use crate::i18n::n_;

/// A hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Hand {
    /// The left hand.
    Left,
    /// The right hand.
    Right,
}

impl Hand {
    /// Both hands, left first.
    pub const ALL: [Hand; 2] = [Hand::Left, Hand::Right];

    /// The English display name (a msgid).
    pub fn name(self) -> &'static str {
        match self {
            Hand::Left => n_("Left hand"),
            Hand::Right => n_("Right hand"),
        }
    }

    /// The five fingers of this hand, thumb first.
    pub fn fingers(self) -> [Finger; 5] {
        match self {
            Hand::Left => [
                Finger::LeftThumb,
                Finger::LeftIndex,
                Finger::LeftMiddle,
                Finger::LeftRing,
                Finger::LeftLittle,
            ],
            Hand::Right => [
                Finger::RightThumb,
                Finger::RightIndex,
                Finger::RightMiddle,
                Finger::RightRing,
                Finger::RightLittle,
            ],
        }
    }
}

/// One finger, as fprintd names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum Finger {
    LeftThumb,
    LeftIndex,
    LeftMiddle,
    LeftRing,
    LeftLittle,
    RightThumb,
    RightIndex,
    RightMiddle,
    RightRing,
    RightLittle,
}

impl Finger {
    /// All ten, left thumb to right little finger (fprintd's order).
    pub const ALL: [Finger; 10] = [
        Finger::LeftThumb,
        Finger::LeftIndex,
        Finger::LeftMiddle,
        Finger::LeftRing,
        Finger::LeftLittle,
        Finger::RightThumb,
        Finger::RightIndex,
        Finger::RightMiddle,
        Finger::RightRing,
        Finger::RightLittle,
    ];

    /// fprintd's name (`"right-index-finger"`...).
    pub fn fprintd_name(self) -> &'static str {
        match self {
            Finger::LeftThumb => "left-thumb",
            Finger::LeftIndex => "left-index-finger",
            Finger::LeftMiddle => "left-middle-finger",
            Finger::LeftRing => "left-ring-finger",
            Finger::LeftLittle => "left-little-finger",
            Finger::RightThumb => "right-thumb",
            Finger::RightIndex => "right-index-finger",
            Finger::RightMiddle => "right-middle-finger",
            Finger::RightRing => "right-ring-finger",
            Finger::RightLittle => "right-little-finger",
        }
    }

    /// The finger fprintd calls `name`; `None` for `"any"` and unknown names.
    pub fn from_fprintd(name: &str) -> Option<Finger> {
        Finger::ALL.into_iter().find(|f| f.fprintd_name() == name)
    }

    /// The English display name, capitalised for a list (a msgid).
    pub fn name(self) -> &'static str {
        match self {
            Finger::LeftThumb => n_("Left thumb"),
            Finger::LeftIndex => n_("Left index finger"),
            Finger::LeftMiddle => n_("Left middle finger"),
            Finger::LeftRing => n_("Left ring finger"),
            Finger::LeftLittle => n_("Left little finger"),
            Finger::RightThumb => n_("Right thumb"),
            Finger::RightIndex => n_("Right index finger"),
            Finger::RightMiddle => n_("Right middle finger"),
            Finger::RightRing => n_("Right ring finger"),
            Finger::RightLittle => n_("Right little finger"),
        }
    }

    /// Which hand.
    pub fn hand(self) -> Hand {
        match self {
            Finger::LeftThumb
            | Finger::LeftIndex
            | Finger::LeftMiddle
            | Finger::LeftRing
            | Finger::LeftLittle => Hand::Left,
            _ => Hand::Right,
        }
    }
}

/// Parses fprintd's list of enrolled fingers: known names only, each once,
/// in [`Finger::ALL`] order. Unknown names (a newer service) are skipped.
pub fn parse_enrolled<S: AsRef<str>>(names: &[S]) -> Vec<Finger> {
    let mut out: Vec<Finger> = names
        .iter()
        .filter_map(|n| Finger::from_fprintd(n.as_ref()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A display name in a sentence: the first letter lower-cased
/// ("Right index finger" -> "right index finger", "Index droit" -> "index droit").
pub fn in_sentence(name: &str) -> String {
    let mut c = name.chars();
    match c.next() {
        Some(first) => first.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Catalogue;

    #[test]
    fn fprintd_names_round_trip() {
        for f in Finger::ALL {
            assert_eq!(Finger::from_fprintd(f.fprintd_name()), Some(f));
        }
        assert_eq!(Finger::from_fprintd("any"), None);
        assert_eq!(Finger::from_fprintd(""), None);
        assert_eq!(Finger::from_fprintd("Right-Index-Finger"), None);
        let names: Vec<_> = Finger::ALL.iter().map(|f| f.fprintd_name()).collect();
        assert_eq!(
            names,
            [
                "left-thumb",
                "left-index-finger",
                "left-middle-finger",
                "left-ring-finger",
                "left-little-finger",
                "right-thumb",
                "right-index-finger",
                "right-middle-finger",
                "right-ring-finger",
                "right-little-finger"
            ]
        );
    }

    #[test]
    fn hands_partition_the_fingers() {
        let mut all: Vec<Finger> = Hand::ALL.iter().flat_map(|h| h.fingers()).collect();
        for f in &all {
            assert!(h_of(*f).fingers().contains(f));
        }
        all.sort();
        assert_eq!(all, Finger::ALL);
        fn h_of(f: Finger) -> Hand {
            f.hand()
        }
        assert_eq!(Finger::LeftLittle.hand(), Hand::Left);
        assert_eq!(Finger::RightThumb.hand(), Hand::Right);
    }

    #[test]
    fn enrolled_list_is_cleaned() {
        assert_eq!(
            parse_enrolled(&["right-thumb", "left-thumb", "any", "toe", "right-thumb"]),
            [Finger::LeftThumb, Finger::RightThumb]
        );
        assert!(parse_enrolled::<&str>(&[]).is_empty());
    }

    #[test]
    fn french_names() {
        let fr = Catalogue::builtin("fr").unwrap();
        let names: Vec<_> = Finger::ALL.iter().map(|f| fr.get(f.name())).collect();
        assert_eq!(
            names,
            [
                "Pouce gauche",
                "Index gauche",
                "Majeur gauche",
                "Annulaire gauche",
                "Petit doigt gauche",
                "Pouce droit",
                "Index droit",
                "Majeur droit",
                "Annulaire droit",
                "Petit doigt droit"
            ]
        );
        assert_eq!(fr.get(Hand::Left.name()), "Main gauche");
        assert_eq!(fr.get(Hand::Right.name()), "Main droite");
        // Distinct names: no two fingers read the same.
        let mut d = names.clone();
        d.sort();
        d.dedup();
        assert_eq!(d.len(), 10);
    }

    #[test]
    fn sentence_case() {
        assert_eq!(in_sentence("Right index finger"), "right index finger");
        assert_eq!(in_sentence("Index droit"), "index droit");
        assert_eq!(in_sentence("Évier"), "évier");
        assert_eq!(in_sentence(""), "");
    }
}
