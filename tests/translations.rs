//! Every user-visible string has a French translation, and nothing else is in
//! the catalogue.
//!
//! The user-visible strings are the string literals passed to `tr`, `trf`,
//! `n_` and `msg` in `src/` (test code excluded: files named `tests.rs` and
//! everything after a file's `#[cfg(test)]`). A literal anywhere else in
//! `src/` that reads like English prose fails the test too: it would reach
//! the screen untranslated.
//!
//! The desktop entry and the AppStream metainfo are checked for their French
//! too.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use fingerprint_settings::i18n::{self, PoEntry, parse_po, placeholders};

/// The functions whose string literal argument is a msgid.
const MARKERS: [&str; 4] = ["tr", "trf", "n_", "msg"];

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str { value: String, line: usize },
    Punct(char),
}

/// Just enough of a Rust lexer: identifiers, string literals (decoded),
/// punctuation; comments, char literals and lifetimes skipped.
fn lex(src: &str) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
        } else if c == 'r' && (chars.get(i + 1) == Some(&'"') || chars.get(i + 1) == Some(&'#')) {
            // Raw string: r"..." or r#"..."#.
            let start_line = line;
            let mut j = i + 1;
            let mut hashes = 0;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) != Some(&'"') {
                // An identifier starting with r.
                let s = i;
                i += 1;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Token::Ident(chars[s..i].iter().collect()));
                continue;
            }
            j += 1;
            let mut value = String::new();
            loop {
                if chars[j] == '"' && (0..hashes).all(|k| chars.get(j + 1 + k) == Some(&'#')) {
                    j += 1 + hashes;
                    break;
                }
                if chars[j] == '\n' {
                    line += 1;
                }
                value.push(chars[j]);
                j += 1;
            }
            out.push(Token::Str {
                value,
                line: start_line,
            });
            i = j;
        } else if c == '"' {
            let start_line = line;
            let mut value = String::new();
            i += 1;
            while chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1;
                    match chars[i] {
                        'n' => value.push('\n'),
                        't' => value.push('\t'),
                        'r' => value.push('\r'),
                        '0' => value.push('\0'),
                        '\\' => value.push('\\'),
                        '"' => value.push('"'),
                        '\'' => value.push('\''),
                        'u' => {
                            // \u{XXXX}
                            let close = (i..chars.len()).find(|k| chars[*k] == '}').unwrap();
                            let hex: String = chars[i + 2..close].iter().collect();
                            value.push(
                                char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap(),
                            );
                            i = close;
                        }
                        '\n' => {
                            // Line continuation: the newline and the next
                            // line's leading whitespace are dropped.
                            line += 1;
                            while chars[i + 1].is_whitespace() {
                                if chars[i + 1] == '\n' {
                                    line += 1;
                                }
                                i += 1;
                            }
                        }
                        other => panic!("line {line}: unknown escape \\{other}"),
                    }
                } else {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    value.push(chars[i]);
                }
                i += 1;
            }
            i += 1;
            out.push(Token::Str {
                value,
                line: start_line,
            });
        } else if c == '\'' {
            // A char literal ('x', '\n', '\u{a0}') or a lifetime ('a).
            if chars.get(i + 1) == Some(&'\\') {
                i += 2;
                while chars[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if chars.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
            }
        } else if c.is_alphabetic() || c == '_' {
            let s = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Token::Ident(chars[s..i].iter().collect()));
        } else if c.is_ascii_digit() {
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.')
            {
                i += 1;
            }
        } else {
            out.push(Token::Punct(c));
            i += 1;
        }
    }
    out
}

/// The part of a source file that is not test code.
fn product_code(path: &Path, src: &str) -> Option<String> {
    if path.file_name().is_some_and(|n| n == "tests.rs") {
        return None;
    }
    Some(match src.find("#[cfg(test)]") {
        Some(i) => src[..i].to_string(),
        None => src.to_string(),
    })
}

fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.sort();
    assert!(files.len() >= 8, "found {files:?}");
    files
        .into_iter()
        .filter_map(|p| {
            let src = std::fs::read_to_string(&p).unwrap();
            product_code(&p, &src).map(|s| (p, s))
        })
        .collect()
}

/// msgid -> where it is used.
type Used = BTreeMap<String, Vec<String>>;

/// The msgids, and the other literals that read like prose.
fn extract() -> (Used, Vec<String>) {
    let mut used = Used::new();
    let mut prose = Vec::new();
    for (path, src) in sources() {
        let name = path
            .strip_prefix(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .display()
            .to_string();
        let toks = lex(&src);
        for (k, t) in toks.iter().enumerate() {
            let Token::Str { value, line } = t else {
                continue;
            };
            let marked = k >= 2
                && toks[k - 1] == Token::Punct('(')
                && matches!(&toks[k - 2], Token::Ident(f) if MARKERS.contains(&f.as_str()))
                // Not a method of the same name (`x.tr(...)`) nor a path
                // segment other than the i18n/message ones.
                && (k < 3 || toks[k - 3] != Token::Punct('.'));
            if marked {
                used.entry(value.clone())
                    .or_default()
                    .push(format!("{name}:{line}"));
            } else if looks_like_prose(value) {
                prose.push(format!("{name}:{line}: {value:?}"));
            }
        }
    }
    (used, prose)
}

/// English words for a person: a capital, then a lower-case letter ("Remove",
/// "Try again"). A single CamelCase word is an identifier (a D-Bus member);
/// text starting in lower case is technical detail, never shown as such.
fn looks_like_prose(s: &str) -> bool {
    let mut c = s.chars();
    let capitalised = matches!(
        (c.next(), c.next()),
        (Some(a), Some(b)) if a.is_uppercase() && b.is_lowercase()
    );
    let camel_case = !s.contains(char::is_whitespace)
        && s.chars().all(char::is_alphanumeric)
        && s.chars().skip(1).any(char::is_uppercase);
    capitalised && !camel_case
}

fn catalogue(lang: &str) -> Vec<PoEntry> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("po/{lang}.po"));
    let text = std::fs::read_to_string(&path).unwrap();
    // The built-in catalogue is this file.
    assert_eq!(Some(text.as_str()), i18n::builtin_po(lang));
    parse_po(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn the_lexer_finds_marked_strings() {
    let src = r##"
        // tr("in a comment")
        /* msg("in a block comment") */
        fn f<'a>(x: &'a str) -> char { let c = '"'; let d = '\n'; tr("One") ; c }
        let m = msg(
            "Two \
             lines",
        );
        let n = n_("Three {x}");
        let r = r#"Raw "quoted" Prose"#;
        let u = trf("Caf\u{e9}", &[]);
        x.tr("Not a marker");
        let id = "lowercase.id";
    "##;
    let toks = lex(src);
    let strs: Vec<_> = toks
        .iter()
        .filter_map(|t| match t {
            Token::Str { value, .. } => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        strs,
        [
            "One",
            "Two lines",
            "Three {x}",
            "Raw \"quoted\" Prose",
            "Café",
            "Not a marker",
            "lowercase.id"
        ]
    );
    assert!(looks_like_prose("Remove"));
    assert!(looks_like_prose("Try again"));
    assert!(looks_like_prose("Fingerprints"));
    assert!(!looks_like_prose("EnrollStatus"));
    assert!(!looks_like_prose("could not start the D-Bus thread"));
    assert!(!looks_like_prose("auth-fingerprint-symbolic"));
    assert!(!looks_like_prose("win.cancel"));
    assert!(!looks_like_prose("not connected to the service"));
    assert!(!looks_like_prose("OK"));
    assert!(!looks_like_prose("{name}: {m}"));
    assert!(product_code(Path::new("x/tests.rs"), "x").is_none());
    assert_eq!(
        product_code(Path::new("x/a.rs"), "code\n#[cfg(test)]\nmod tests;").unwrap(),
        "code\n"
    );
}

#[test]
fn every_user_visible_string_is_translated_into_french() {
    let (used, prose) = extract();
    assert!(
        used.len() >= 30,
        "only {} strings found: {used:?}",
        used.len()
    );
    assert!(
        prose.is_empty(),
        "user-visible text outside tr/trf/n_/msg (untranslatable):\n{}",
        prose.join("\n")
    );

    let entries = catalogue("fr");
    let mut seen = BTreeSet::new();
    let mut problems = Vec::new();
    for e in &entries {
        if !seen.insert(e.msgid.clone()) {
            problems.push(format!("duplicate msgid {:?}", e.msgid));
        }
    }
    let po: BTreeMap<&str, &PoEntry> = entries.iter().map(|e| (e.msgid.as_str(), e)).collect();

    for (id, places) in &used {
        match po.get(id.as_str()) {
            None => problems.push(format!("missing: {id:?} ({})", places.join(", "))),
            Some(e) if e.fuzzy => problems.push(format!("fuzzy: {id:?}")),
            Some(e) if e.msgstr.is_empty() => problems.push(format!("untranslated: {id:?}")),
            Some(e) => {
                if placeholders(&e.msgid) != placeholders(&e.msgstr) {
                    problems.push(format!("placeholders differ: {id:?} -> {:?}", e.msgstr));
                }
                // The same final punctuation.
                let end = |s: &str| s.chars().last().filter(|c| ".?!:…".contains(*c));
                if end(&e.msgid) != end(&e.msgstr) {
                    problems.push(format!(
                        "final punctuation differs: {id:?} -> {:?}",
                        e.msgstr
                    ));
                }
                // French typography: a no-break space before : ; ? !
                for (i, c) in e.msgstr.char_indices() {
                    let before = e.msgstr[..i].chars().last();
                    if ":;?!".contains(c) && !matches!(before, Some('\u{a0}' | '\u{202f}')) {
                        problems.push(format!("no no-break space before {c:?}: {:?}", e.msgstr));
                    }
                }
            }
        }
    }
    for e in &entries {
        if !used.contains_key(&e.msgid) {
            problems.push(format!("not used any more: {:?}", e.msgid));
        }
    }
    assert!(problems.is_empty(), "po/fr.po:\n{}", problems.join("\n"));
}

#[test]
fn every_language_has_its_catalogue() {
    let mut files: Vec<String> =
        std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("po"))
            .unwrap()
            .filter_map(|e| {
                let p = e.unwrap().path();
                (p.extension().is_some_and(|x| x == "po"))
                    .then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
            })
            .collect();
    files.sort();
    let built_in: Vec<&str> = i18n::languages().collect();
    assert_eq!(files, built_in);
}

fn packaging(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("packaging/common")
            .join(name),
    )
    .unwrap()
}

#[test]
fn desktop_entry_is_translated() {
    let d = packaging(&format!("{}.desktop", fingerprint_settings::APP_ID));
    let fr = i18n::Catalogue::builtin("fr").unwrap();
    let keys: BTreeMap<&str, &str> = d.lines().filter_map(|l| l.split_once('=')).collect();
    assert_eq!(keys["Name"], "Fingerprints");
    // The same name as the window title, in both languages.
    assert_eq!(keys["Name[fr]"], fr.get("Fingerprints"));
    for k in ["Name", "GenericName", "Comment", "Keywords"] {
        assert!(keys.contains_key(k), "{k}");
        assert!(keys.contains_key(format!("{k}[fr]").as_str()), "{k}[fr]");
    }
    assert_eq!(keys["Icon"], fingerprint_settings::APP_ID);
    assert_eq!(keys["Exec"], "fingerprint-settings");
    assert!(keys["Categories"].split(';').any(|c| c == "Settings"));
}

#[test]
fn metainfo_is_translated() {
    let m = packaging(&format!("{}.metainfo.xml", fingerprint_settings::APP_ID));
    // The developer's name is a name, not text to translate.
    let dev = m.find("<developer").unwrap();
    let dev_end = m.find("</developer>").unwrap();
    let m = format!("{}{}", &m[..dev], &m[dev_end..]);
    assert!(m.contains(&format!("<id>{}</id>", fingerprint_settings::APP_ID)));
    assert!(m.contains("<name>Fingerprints</name>"));
    assert!(m.contains("<name xml:lang=\"fr\">Empreintes digitales</name>"));
    // Each translatable element has its French twin.
    for tag in ["name", "summary", "p", "li"] {
        let en = m.matches(&format!("<{tag}>")).count();
        let fr = m.matches(&format!("<{tag} xml:lang=\"fr\">")).count();
        assert!(en > 0, "{tag}");
        assert_eq!(en, fr, "<{tag}>: {en} in English, {fr} in French");
    }
}
