//! The names the user gives their fingerprints.
//!
//! fprintd only knows ten finger slots (`"right-index-finger"`...), so the
//! names live here, in the user's own configuration:
//! `$XDG_CONFIG_HOME/fingerprint-settings/names.toml` (by default
//! `~/.config/fingerprint-settings/names.toml`), keyed by fprintd's finger
//! name:
//!
//! ```toml
//! [names]
//! left-thumb = "Pouce, quand je suis au lit"
//! right-index-finger = "Index droit"
//! ```
//!
//! A slot without a name shows the finger's standard name. The file is
//! written atomically (a temporary file, then a rename). A missing file means
//! no names; an unreadable or corrupt one means no names and a warning, never
//! a failure: it is kept as `names.toml.corrupt` the next time names are
//! saved.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::fingers::Finger;

/// The name of each named slot.
pub type Labels = BTreeMap<Finger, String>;

/// The longest name kept, in characters.
pub const MAX_LEN: usize = 100;

/// A name as stored: trimmed, at most [`MAX_LEN`] characters; `None` when
/// nothing is left.
pub fn clean(label: &str) -> Option<String> {
    let t = label.trim();
    if t.is_empty() {
        return None;
    }
    Some(t.chars().take(MAX_LEN).collect())
}

/// The file's path from `XDG_CONFIG_HOME` and `HOME` (their values, `None`
/// when unset). A relative `XDG_CONFIG_HOME` is invalid and ignored, as the
/// XDG base directory specification says.
pub fn path_from(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let base = xdg_config_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|h| h.join(".config"))
        })?;
    Some(base.join("fingerprint-settings").join("names.toml"))
}

/// Reads the `[names]` table. Entries that are not a known finger with a
/// non-empty text are skipped, each with a warning. Fails when the text is
/// not TOML.
pub fn parse(text: &str) -> Result<(Labels, Vec<String>), String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut labels = Labels::new();
    let mut warnings = Vec::new();
    let Some(names) = table.get("names") else {
        return Ok((labels, warnings));
    };
    let Some(names) = names.as_table() else {
        return Err("\"names\" is not a table".into());
    };
    for (key, value) in names {
        let Some(finger) = Finger::from_fprintd(key) else {
            warnings.push(format!("unknown finger {key:?} ignored"));
            continue;
        };
        match value.as_str().and_then(clean) {
            Some(label) => {
                labels.insert(finger, label);
            }
            None => warnings.push(format!("name of {key:?} is not a non-empty text, ignored")),
        }
    }
    Ok((labels, warnings))
}

/// The file's text: a short explanation, then the `[names]` table.
pub fn render(labels: &Labels) -> String {
    let mut out = String::from(
        "# The names of your fingerprints, shown by Fingerprints (fingerprint-settings).\n\
         # One line per fingerprint: fprintd's finger name = \"the name you gave it\".\n\
         # The application rewrites this file when you add, rename or remove one.\n\
         \n\
         [names]\n",
    );
    for (finger, label) in labels {
        let value = toml::Value::String(label.clone());
        out.push_str(&format!("{} = {value}\n", finger.fprintd_name()));
    }
    out
}

/// What [`Store::load`] found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Loaded {
    /// The names (empty when there is no file or it is unusable).
    pub labels: Labels,
    /// What was wrong, for the log.
    pub warnings: Vec<String>,
}

/// The names file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    path: PathBuf,
    /// The file was unusable when loaded: keep it aside before overwriting.
    corrupt: bool,
}

impl Store {
    /// The store at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Store {
            path: path.into(),
            corrupt: false,
        }
    }

    /// The user's store, from the environment (`None` when neither
    /// `XDG_CONFIG_HOME` nor `HOME` gives a place).
    pub fn from_env() -> Option<Self> {
        path_from(
            std::env::var_os("XDG_CONFIG_HOME").as_deref(),
            std::env::var_os("HOME").as_deref(),
        )
        .map(Store::new)
    }

    /// Where the names are kept.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where an unusable file is kept aside.
    pub fn corrupt_path(&self) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".corrupt");
        self.path.with_file_name(name)
    }

    /// Reads the names. Never fails: a missing file is no names; an
    /// unreadable or corrupt one is no names and a warning.
    pub fn load(&mut self) -> Loaded {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::default(),
            Err(e) => {
                self.corrupt = true;
                return Loaded {
                    labels: Labels::new(),
                    warnings: vec![format!("cannot read {}: {e}", self.path.display())],
                };
            }
        };
        match parse(&text) {
            Ok((labels, warnings)) => Loaded {
                labels,
                warnings: warnings
                    .into_iter()
                    .map(|w| format!("{}: {w}", self.path.display()))
                    .collect(),
            },
            Err(e) => {
                self.corrupt = true;
                Loaded {
                    labels: Labels::new(),
                    warnings: vec![format!(
                        "{} is not valid, the standard names are used: {e}",
                        self.path.display()
                    )],
                }
            }
        }
    }

    /// Writes the names atomically: a temporary file next to the real one,
    /// flushed to disk, then renamed over it. An unusable file found by
    /// [`Store::load`] is first kept as [`Store::corrupt_path`].
    pub fn save(&mut self, labels: &Labels) -> std::io::Result<()> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("the names file has no directory"))?;
        std::fs::create_dir_all(dir)?;
        if self.corrupt {
            match std::fs::rename(&self.path, self.corrupt_path()) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            self.corrupt = false;
        }
        let mut tmp_name = self.path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(format!(".tmp-{}", std::process::id()));
        let tmp = self.path.with_file_name(tmp_name);
        let written = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(render(labels).as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &self.path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
            return written;
        }
        // The rename itself reaches the disk with the directory.
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!(
                "fingerprint-settings-labels-{}-{tag}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&p);
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample() -> Labels {
        Labels::from([
            (Finger::LeftThumb, "Pouce, quand je suis au lit".to_string()),
            (Finger::RightIndex, "Index droit".to_string()),
            (
                Finger::RightLittle,
                "Quotes \" and \\ backslash, tab\t, newline\n, accents éàü, 指".to_string(),
            ),
        ])
    }

    #[test]
    fn round_trip_through_the_file() {
        let dir = TempDir::new("round");
        let path = dir.0.join("fingerprint-settings").join("names.toml");
        let mut store = Store::new(&path);
        assert_eq!(store.path(), path);
        // No file yet: no names, no warning.
        assert_eq!(store.load(), Loaded::default());
        store.save(&sample()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // Human-readable: a comment, the table, one line per name.
        assert!(text.starts_with("# The names of your fingerprints"));
        assert!(text.contains("[names]\n"));
        assert!(text.contains("left-thumb = \"Pouce, quand je suis au lit\"\n"));
        assert!(text.contains("right-index-finger = \"Index droit\"\n"));
        let mut again = Store::new(&path);
        assert_eq!(
            again.load(),
            Loaded {
                labels: sample(),
                warnings: vec![]
            }
        );
        // Saving fewer names removes the others; no temporary file is left.
        let mut fewer = sample();
        fewer.remove(&Finger::RightLittle);
        again.save(&fewer).unwrap();
        assert_eq!(Store::new(&path).load().labels, fewer);
        again.save(&Labels::new()).unwrap();
        assert!(Store::new(&path).load().labels.is_empty());
        let entries: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(entries, ["names.toml"]);
    }

    #[test]
    fn a_corrupt_file_is_tolerated_then_kept_aside() {
        let dir = TempDir::new("corrupt");
        let path = dir.0.join("names.toml");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(&path, "[names\nleft-thumb = ").unwrap();
        let mut store = Store::new(&path);
        let loaded = store.load();
        assert!(loaded.labels.is_empty());
        assert_eq!(loaded.warnings.len(), 1);
        assert!(
            loaded.warnings[0].contains("is not valid"),
            "{:?}",
            loaded.warnings
        );
        // The next save keeps the broken file for the user.
        store.save(&sample()).unwrap();
        assert_eq!(
            std::fs::read_to_string(store.corrupt_path()).unwrap(),
            "[names\nleft-thumb = "
        );
        assert_eq!(store.corrupt_path(), dir.0.join("names.toml.corrupt"));
        assert_eq!(Store::new(&path).load().labels, sample());
        // Only once.
        store.save(&Labels::new()).unwrap();
        assert_eq!(
            std::fs::read_to_string(store.corrupt_path()).unwrap(),
            "[names\nleft-thumb = "
        );

        // Not UTF-8: unreadable, the same.
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let mut store = Store::new(&path);
        let loaded = store.load();
        assert!(loaded.labels.is_empty());
        assert_eq!(loaded.warnings.len(), 1);

        // A directory where the file should be: unreadable, no crash.
        let dir2 = TempDir::new("isdir");
        std::fs::create_dir_all(dir2.0.join("names.toml")).unwrap();
        let mut store = Store::new(dir2.0.join("names.toml"));
        let loaded = store.load();
        assert!(loaded.labels.is_empty());
        assert_eq!(loaded.warnings.len(), 1);
    }

    #[test]
    fn bad_entries_are_skipped_with_a_warning() {
        let (labels, warnings) = parse(
            "title = \"other keys are ignored\"\n\
             [names]\n\
             left-thumb = \"  Pouce  \"\n\
             right-toe = \"no such finger\"\n\
             any = \"not a slot\"\n\
             right-thumb = 42\n\
             left-index-finger = \"   \"\n\
             \"right-index-finger\" = \"quoted key\"\n",
        )
        .unwrap();
        assert_eq!(
            labels,
            Labels::from([
                (Finger::LeftThumb, "Pouce".to_string()),
                (Finger::RightIndex, "quoted key".to_string()),
            ])
        );
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        // No [names] table: nothing, no warning.
        assert_eq!(parse("").unwrap(), (Labels::new(), vec![]));
        assert_eq!(
            parse("# only a comment\n").unwrap(),
            (Labels::new(), vec![])
        );
        // [names] of the wrong type, or not TOML at all: an error.
        assert!(parse("names = 3\n").is_err());
        assert!(parse("[names]\nleft-thumb = \"unterminated\n").is_err());
        assert!(parse("[names]\nleft-thumb = \"a\"\nleft-thumb = \"b\"\n").is_err());
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean("  Pouce  "), Some("Pouce".into()));
        assert_eq!(clean(" \t\n"), None);
        assert_eq!(clean(""), None);
        let long = "é".repeat(MAX_LEN + 20);
        assert_eq!(clean(&long).unwrap().chars().count(), MAX_LEN);
    }

    #[test]
    fn the_path_follows_xdg() {
        let p = |x: Option<&str>, h: Option<&str>| {
            path_from(x.map(OsStr::new), h.map(OsStr::new)).map(|p| p.display().to_string())
        };
        assert_eq!(
            p(Some("/cfg"), Some("/home/u")).as_deref(),
            Some("/cfg/fingerprint-settings/names.toml")
        );
        assert_eq!(
            p(None, Some("/home/u")).as_deref(),
            Some("/home/u/.config/fingerprint-settings/names.toml")
        );
        // A relative XDG_CONFIG_HOME is ignored.
        assert_eq!(
            p(Some("cfg"), Some("/home/u")).as_deref(),
            Some("/home/u/.config/fingerprint-settings/names.toml")
        );
        assert_eq!(p(Some(""), None), None);
        assert_eq!(p(None, Some("relative")), None);
        assert_eq!(p(None, None), None);
    }

    #[test]
    fn a_save_that_cannot_happen_is_an_error_not_a_panic() {
        let dir = TempDir::new("blocked");
        std::fs::create_dir_all(&dir.0).unwrap();
        // The directory is a file.
        std::fs::write(dir.0.join("fingerprint-settings"), "x").unwrap();
        let mut store = Store::new(dir.0.join("fingerprint-settings").join("names.toml"));
        assert!(store.save(&sample()).is_err());
        assert!(Store::new("/").save(&sample()).is_err());
    }
}
