//! Translations: English source strings, looked up in gettext PO catalogues.
//!
//! The catalogues (`po/<lang>.po`) are compiled into the binary and parsed once
//! at start-up, so the application needs neither `msgfmt` at build time nor
//! installed `.mo` files at run time, and runs translated straight from
//! `cargo run`. Translators still work on ordinary PO files (Poedit,
//! Weblate...).
//!
//! The language follows gettext's rules: the first of `LANGUAGE`
//! (a colon-separated list), `LC_ALL`, `LC_MESSAGES` and `LANG` that is set
//! decides; `C` and `POSIX` mean the English source strings.
//!
//! Every user-visible string goes through [`tr`], [`trf`] or, for tables that
//! are translated later, [`n_`], always with a string literal: the
//! translation completeness test (`tests/translations.rs`) finds them in the
//! sources that way.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The languages with a catalogue: (code, PO file).
const CATALOGUES: &[(&str, &str)] = &[("fr", include_str!("../po/fr.po"))];

/// A parsed catalogue: msgid -> msgstr, translated entries only.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Catalogue {
    map: HashMap<String, String>,
}

/// One entry of a PO file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoEntry {
    /// The English source string.
    pub msgid: String,
    /// The translation; empty when untranslated.
    pub msgstr: String,
    /// Marked `#, fuzzy`: needs review, not used.
    pub fuzzy: bool,
}

/// Why a PO file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoError {
    /// 1-based line.
    pub line: usize,
    /// What is wrong.
    pub what: String,
}

impl std::fmt::Display for PoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.what)
    }
}

impl std::error::Error for PoError {}

/// Decodes the body of a C-style quoted PO string (without the quotes).
fn unescape(s: &str, line: usize) -> Result<String, PoError> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            other => {
                return Err(PoError {
                    line,
                    what: format!(
                        "unsupported escape \\{}",
                        other.map(String::from).unwrap_or_default()
                    ),
                });
            }
        }
    }
    Ok(out)
}

/// The string of a `"..."` token.
fn quoted(s: &str, line: usize) -> Result<String, PoError> {
    let s = s.trim();
    match s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        Some(body) if s.len() >= 2 => unescape(body, line),
        _ => Err(PoError {
            line,
            what: format!("expected a quoted string, got {s:?}"),
        }),
    }
}

/// Parses a PO file: singular entries, `#, fuzzy` flags and multi-line
/// strings. The header (empty msgid) is skipped. Plural forms and message
/// contexts are not used by this application and are refused.
pub fn parse_po(text: &str) -> Result<Vec<PoEntry>, PoError> {
    #[derive(PartialEq)]
    enum Field {
        None,
        Id,
        Str,
    }
    let mut entries = Vec::new();
    let mut id: Option<String> = None;
    let mut msgstr: Option<String> = None;
    let mut fuzzy = false;
    let mut next_fuzzy = false;
    let mut field = Field::None;

    let mut flush = |id: &mut Option<String>,
                     msgstr: &mut Option<String>,
                     fuzzy: bool,
                     line: usize|
     -> Result<(), PoError> {
        match (id.take(), msgstr.take()) {
            (None, None) => Ok(()),
            (Some(msgid), Some(msgstr)) => {
                if !msgid.is_empty() {
                    entries.push(PoEntry {
                        msgid,
                        msgstr,
                        fuzzy,
                    });
                }
                Ok(())
            }
            _ => Err(PoError {
                line,
                what: "msgid without msgstr".into(),
            }),
        }
    };

    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let l = raw.trim();
        if l.is_empty() {
            continue;
        }
        if let Some(flags) = l.strip_prefix("#,") {
            if flags.split(',').any(|f| f.trim() == "fuzzy") {
                next_fuzzy = true;
            }
            continue;
        }
        if l.starts_with('#') {
            continue;
        }
        if let Some(rest) = l.strip_prefix("msgid ") {
            flush(&mut id, &mut msgstr, fuzzy, line)?;
            fuzzy = std::mem::take(&mut next_fuzzy);
            id = Some(quoted(rest, line)?);
            field = Field::Id;
        } else if let Some(rest) = l.strip_prefix("msgstr ") {
            if id.is_none() || msgstr.is_some() {
                return Err(PoError {
                    line,
                    what: "msgstr without msgid".into(),
                });
            }
            msgstr = Some(quoted(rest, line)?);
            field = Field::Str;
        } else if l.starts_with('"') {
            let s = quoted(l, line)?;
            match field {
                Field::Id => id.as_mut().map(|v| v.push_str(&s)),
                Field::Str => msgstr.as_mut().map(|v| v.push_str(&s)),
                Field::None => {
                    return Err(PoError {
                        line,
                        what: "string outside an entry".into(),
                    });
                }
            };
        } else {
            return Err(PoError {
                line,
                what: format!("unsupported line {l:?} (plurals and contexts are not used)"),
            });
        }
    }
    flush(&mut id, &mut msgstr, fuzzy, text.lines().count())?;
    Ok(entries)
}

impl Catalogue {
    /// A catalogue from PO text: translated, non-fuzzy entries only.
    pub fn from_po(text: &str) -> Result<Self, PoError> {
        let map = parse_po(text)?
            .into_iter()
            .filter(|e| !e.fuzzy && !e.msgstr.is_empty())
            .map(|e| (e.msgid, e.msgstr))
            .collect();
        Ok(Catalogue { map })
    }

    /// The built-in catalogue for `lang` (`"fr"`, ...), if there is one.
    pub fn builtin(lang: &str) -> Option<Self> {
        CATALOGUES
            .iter()
            .find(|(code, _)| *code == lang)
            .map(|(_, po)| Catalogue::from_po(po).expect("a built-in catalogue is valid"))
    }

    /// The translation of `id`, or `id` itself.
    pub fn get<'a>(&'a self, id: &'a str) -> &'a str {
        self.map.get(id).map(String::as_str).unwrap_or(id)
    }

    /// Number of translated entries.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// No translated entry: English.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The codes of the built-in languages.
pub fn languages() -> impl Iterator<Item = &'static str> {
    CATALOGUES.iter().map(|(code, _)| *code)
}

/// The PO text of a built-in language.
pub fn builtin_po(lang: &str) -> Option<&'static str> {
    CATALOGUES
        .iter()
        .find(|(code, _)| *code == lang)
        .map(|(_, po)| *po)
}

/// The language code of a locale name: `fr_FR.UTF-8@euro` -> `fr`,
/// `pt_BR` -> `pt_BR`... reduced to what is built in: the full name, then
/// the language alone. `None` for English, `C`, `POSIX` and unknown languages.
pub fn match_locale(locale: &str) -> Option<&'static str> {
    let base = locale.split(['.', '@']).next().unwrap_or("");
    if base.is_empty() || base == "C" || base == "POSIX" {
        return None;
    }
    let lang = base.split('_').next().unwrap_or(base);
    languages()
        .find(|c| *c == base)
        .or_else(|| languages().find(|c| *c == lang))
}

/// The language gettext would pick from these variables (values as read from
/// the environment, `None` when unset). English (`None`) when nothing built in
/// matches.
pub fn pick_language(
    language: Option<&str>,
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
) -> Option<&'static str> {
    fn nonempty(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|v| !v.is_empty())
    }
    // The locale proper: the first of LC_ALL, LC_MESSAGES, LANG that is set.
    let locale = nonempty(lc_all)
        .or(nonempty(lc_messages))
        .or(nonempty(lang));
    // gettext ignores LANGUAGE when the locale is "C".
    let c_locale = matches!(
        locale.map(|l| l.split(['.', '@']).next().unwrap_or("")),
        Some("C" | "POSIX")
    );
    if !c_locale && let Some(list) = nonempty(language) {
        for item in list.split(':') {
            if item.starts_with("en") {
                return None;
            }
            if let Some(code) = match_locale(item) {
                return Some(code);
            }
        }
    }
    locale.and_then(match_locale)
}

static ACTIVE: OnceLock<Catalogue> = OnceLock::new();

/// Chooses the language from the environment. Call once at start-up, before
/// any [`tr`]; later calls do nothing. Returns the language code chosen.
pub fn init_from_env() -> Option<&'static str> {
    let var = |n: &str| std::env::var(n).ok();
    let lang = pick_language(
        var("LANGUAGE").as_deref(),
        var("LC_ALL").as_deref(),
        var("LC_MESSAGES").as_deref(),
        var("LANG").as_deref(),
    );
    init(lang);
    lang
}

/// Uses language `lang` (`None`: English) for [`tr`]. The first call wins.
pub fn init(lang: Option<&str>) {
    let _ = ACTIVE.set(lang.and_then(Catalogue::builtin).unwrap_or_default());
}

fn active() -> &'static Catalogue {
    ACTIVE.get_or_init(Catalogue::default)
}

/// The translation of the English string `id` in the active language.
pub fn tr(id: &'static str) -> &'static str {
    active().get(id)
}

/// [`tr`], then every `{name}` replaced by its value.
pub fn trf(id: &'static str, args: &[(&str, &str)]) -> String {
    fill(tr(id), args)
}

/// Replaces every `{name}` of `template` by its value in `args`.
pub fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Marks a string for translation without translating it: for tables whose
/// entries are passed to [`tr`] later. Returns `id` unchanged.
pub const fn n_(id: &'static str) -> &'static str {
    id
}

/// The `{name}` placeholders of a string, sorted, for checking translations.
pub fn placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    out.push(name.to_string());
                }
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_flags_and_multiline_strings() {
        let po = r#"
# A comment
msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"

#: src/a.rs:1
msgid "Hello"
msgstr "Bonjour"

#, fuzzy
msgid "Old"
msgstr "Vieux"

msgid ""
"Two "
"lines"
msgstr ""
"Deux "
"lignes"

msgid "Untranslated"
msgstr ""

msgid "Quote \"x\" and \\ and\nnewline"
msgstr "Guillemet \"x\" et \\ et\nretour"
"#;
        let e = parse_po(po).unwrap();
        assert_eq!(e.len(), 5);
        assert_eq!(
            e[0],
            PoEntry {
                msgid: "Hello".into(),
                msgstr: "Bonjour".into(),
                fuzzy: false
            }
        );
        assert!(e[1].fuzzy);
        assert_eq!(e[2].msgid, "Two lines");
        assert_eq!(e[2].msgstr, "Deux lignes");
        assert_eq!(e[3].msgstr, "");
        assert_eq!(e[4].msgid, "Quote \"x\" and \\ and\nnewline");
        let c = Catalogue::from_po(po).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c.get("Hello"), "Bonjour");
        assert_eq!(c.get("Old"), "Old", "fuzzy entries are not used");
        assert_eq!(c.get("Untranslated"), "Untranslated");
        assert_eq!(c.get("Not there"), "Not there");
    }

    #[test]
    fn refuses_malformed_po() {
        for (po, line) in [
            ("msgid \"a\"\nmsgid \"b\"\nmsgstr \"c\"\n", 2),
            ("msgstr \"x\"\n", 1),
            ("msgid \"a\"\nmsgstr \"b\"\nmsgstr \"c\"\n", 3),
            ("\"orphan\"\n", 1),
            ("msgid \"a\"\nmsgid_plural \"as\"\n", 2),
            ("msgctxt \"x\"\n", 1),
            ("msgid a\n", 1),
            ("msgid \"a\\q\"\nmsgstr \"\"\n", 1),
            ("msgid \"a\"\n", 1),
        ] {
            let err = parse_po(po).expect_err(po);
            assert_eq!(err.line, line, "{po:?}: {err}");
            assert!(!err.to_string().is_empty());
        }
    }

    #[test]
    fn locale_names() {
        assert_eq!(match_locale("fr_FR.UTF-8"), Some("fr"));
        assert_eq!(match_locale("fr_CA"), Some("fr"));
        assert_eq!(match_locale("fr"), Some("fr"));
        assert_eq!(match_locale("fr_BE@euro"), Some("fr"));
        assert_eq!(match_locale("en_GB.UTF-8"), None);
        assert_eq!(match_locale("C.UTF-8"), None);
        assert_eq!(match_locale("POSIX"), None);
        assert_eq!(match_locale(""), None);
        assert_eq!(match_locale("de_DE"), None);
    }

    #[test]
    fn language_choice_follows_gettext() {
        // LANG alone.
        assert_eq!(
            pick_language(None, None, None, Some("fr_FR.UTF-8")),
            Some("fr")
        );
        // LC_ALL wins over LC_MESSAGES, which wins over LANG.
        assert_eq!(
            pick_language(None, Some("en_US.UTF-8"), Some("fr_FR"), Some("fr_FR")),
            None
        );
        assert_eq!(
            pick_language(None, None, Some("fr_FR"), Some("en_US")),
            Some("fr")
        );
        assert_eq!(
            pick_language(None, Some(""), Some(""), Some("fr_FR")),
            Some("fr")
        );
        // LANGUAGE is a priority list over the locale...
        assert_eq!(
            pick_language(Some("de:fr"), None, None, Some("en_US.UTF-8")),
            Some("fr")
        );
        assert_eq!(
            pick_language(Some("en_GB:fr"), None, None, Some("fr_FR.UTF-8")),
            None
        );
        // ...unknown entries fall through to the locale...
        assert_eq!(
            pick_language(Some("de"), None, None, Some("fr_FR.UTF-8")),
            Some("fr")
        );
        // ...and is ignored under the C locale.
        assert_eq!(pick_language(Some("fr"), Some("C"), None, None), None);
        assert_eq!(pick_language(Some("fr"), None, None, Some("POSIX")), None);
        // Nothing set: English.
        assert_eq!(pick_language(None, None, None, None), None);
        assert_eq!(pick_language(Some("fr"), None, None, None), Some("fr"));
    }

    #[test]
    fn placeholders_and_fill() {
        assert_eq!(placeholders("Step {step} of {total}"), ["step", "total"]);
        assert_eq!(placeholders("{finger}: {x y} {}"), ["finger"]);
        assert!(placeholders("none { here").is_empty());
        assert_eq!(
            fill("Step {step} of {total}", &[("step", "2"), ("total", "6")]),
            "Step 2 of 6"
        );
        assert_eq!(fill("{a}{a}", &[("a", "x")]), "xx");
        assert_eq!(n_("Same"), "Same");
    }

    #[test]
    fn builtin_french_is_valid() {
        let fr = Catalogue::builtin("fr").unwrap();
        assert!(!fr.is_empty());
        assert_eq!(fr.get("Fingerprints"), "Empreintes digitales");
        assert!(Catalogue::builtin("xx").is_none());
        assert!(builtin_po("fr").is_some());
        assert_eq!(languages().collect::<Vec<_>>(), ["fr"]);
    }
}
