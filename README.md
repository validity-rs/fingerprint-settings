# Fingerprints (`fingerprint-settings`)

A small desktop application to manage **your own fingerprints**: add one,
name it, test it, remove it. It is meant for desktops that have no
fingerprint settings of their own, such as **XFCE, MATE and LXQt**, and for
people who want something plain and readable.

- **Add a fingerprint**: four big buttons for the fingers people use (right
  and left index, right and left thumb), the six others behind "Other
  finger…", or "Just a name, any finger". Then follow large, plain
  instructions ("Lift your finger, then place it on the reader again") with a
  step counter ("Step 3 of 6").
- **Name it** the way you like: "Index droit", or "Thumb, when I'm in bed".
  The list shows the names, each with **Rename** and **Remove**.
- **Test a finger**: touch the reader and see "Recognised: Thumb, when I'm in
  bed" or "Not recognised".
- **Remove a fingerprint**, after a confirmation.
- Errors in plain language: no reader, reader busy, not allowed, service not
  available. The technical detail stays folded under "Technical details".
- The password always keeps working; the application says so.
- English and French.

App id: `io.github.validity_rs.FingerprintSettings`. License: MIT.

## Requirements

- A fingerprint service that speaks fprintd's D-Bus API
  (`net.reactivated.Fprint` on the system bus): **fprintd** itself, or any
  compatible service such as [validity-rs](https://github.com/validity-rs/validity-rs)'s
  daemon. The application uses only the standard API (`Manager.GetDefaultDevice`,
  `Device.ListEnrolledFingers`, `Claim`, `Release`, `EnrollStart/Stop`,
  `VerifyStart/Stop`, `DeleteEnrolledFinger`, falling back to
  `DeleteEnrolledFingers2` on older services, after asking).
- GTK 4.18 or newer and libadwaita 1.7 or newer (Debian 13 "trixie" is the
  oldest supported system).
- To build: Rust 1.99 or newer, and the development packages
  (Debian/Ubuntu: `libgtk-4-dev libadwaita-1-dev`; Fedora: `gtk4-devel
  libadwaita-devel`; Arch: `gtk4 libadwaita`; Alpine: `gtk4.0-dev
  libadwaita-dev`).
- To run the end-to-end test: `dbus-daemon` (Debian: `dbus-daemon`;
  Fedora, Arch, Alpine: `dbus`) and network access to GitHub, where cargo
  fetches validity-rs (see "Tests").

Whether you may change your fingerprints is decided by the service's polkit
policy (`net.reactivated.fprint.device.enroll`). fprintd's default asks for
**your own password** before adding or removing a fingerprint, so a polkit
authentication agent must be running in the session (XFCE: `xfce-polkit` or
`policykit-1-gnome`; MATE: `mate-polkit`; LXQt: `lxqt-policykit`). Without
one, the application says you are not allowed. A call that may show this
prompt is given ten minutes; any other call one minute, after which the
application says the service did not answer.

## Where the names are kept

fprintd knows ten finger slots, not names. The names are kept in your own
configuration, `$XDG_CONFIG_HOME/fingerprint-settings/names.toml` (by default
`~/.config/fingerprint-settings/names.toml`), keyed by fprintd's finger name:

```toml
[names]
left-thumb = "Thumb, when I'm in bed"
right-index-finger = "Work"
```

- A fingerprint without a name shows its finger's standard name, in your
  language. Choosing a finger and keeping its suggested name stores nothing.
- "Just a name, any finger" puts the fingerprint in the first free slot:
  right index, left index, right thumb, left thumb, then the others. The
  instructions then say "your finger" rather than naming one.
- The name follows the slot: removing a fingerprint removes its name, and
  names of slots that are no longer enrolled (removed with another tool) are
  dropped the next time the list is read.
- The file is written atomically (temporary file, then rename). A missing
  file means no names. An unreadable or invalid one means the standard names
  and a warning in the log, never a crash; it is kept as
  `names.toml.corrupt` the next time a name is saved.

## Build, run, install

```bash
cargo build --release
./target/release/fingerprint-settings
```

To install for all users:

```bash
sudo install -Dm755 target/release/fingerprint-settings /usr/local/bin/fingerprint-settings
sudo install -Dm644 packaging/common/io.github.validity_rs.FingerprintSettings.desktop \
    /usr/local/share/applications/io.github.validity_rs.FingerprintSettings.desktop
sudo install -Dm644 packaging/common/io.github.validity_rs.FingerprintSettings.metainfo.xml \
    /usr/local/share/metainfo/io.github.validity_rs.FingerprintSettings.metainfo.xml
sudo install -Dm644 packaging/common/icons/hicolor/scalable/apps/io.github.validity_rs.FingerprintSettings.svg \
    /usr/local/share/icons/hicolor/scalable/apps/io.github.validity_rs.FingerprintSettings.svg
```

The desktop entry puts "Fingerprints" in the Settings menu and, on XFCE, in
the Settings Manager under Hardware.

### Trying it without a reader

`e2e/examples/demo_service.rs` runs validity-rs's daemon with its emulated
sensor on a private bus; the application finds it through
`DBUS_SYSTEM_BUS_ADDRESS`:

```bash
dbus-daemon --session --nofork --print-address=1 > /tmp/demo-bus &
sleep 1; BUS=$(head -1 /tmp/demo-bus)
cargo run --manifest-path e2e/Cargo.toml --example demo_service -- --address "$BUS" &
DBUS_SYSTEM_BUS_ADDRESS="$BUS" cargo run
# While the application waits for a finger, "touch" the emulated reader
# (finger number 1; another number is another finger):
gdbus call --address "$BUS" --dest io.github.validity_rs.FingerprintSettings.Demo \
    --object-path /io/github/validity_rs/FingerprintSettings/Demo \
    --method io.github.validity_rs.FingerprintSettings.Demo.Touch 1
```

## Tests

```bash
cargo test                                   # the application
cargo test --manifest-path e2e/Cargo.toml    # the end-to-end test
```

- Unit tests of the UI-free core (`src/machine.rs` and its tests, `status`,
  `error`, `i18n`, `fingers`, `labels`): every enrolment and verification
  result, every error path, cancelling at each step, the rule that a claimed
  reader is always released, naming, renaming and slot allocation, and the
  names file (round trip, corrupt or unreadable file, clean-up).
- `tests/translations.rs`: every user-visible string has a French
  translation, the catalogue has nothing stale, placeholders and French
  typography are right; the desktop entry and metainfo are translated.
- `e2e/tests/end_to_end.rs`: the same core and D-Bus driver as the window,
  against validity-rs's daemon with its emulated sensor on a private
  `dbus-daemon`: enrol, list, test, remove, duplicates, cancels, a busy
  reader, polkit refusals, no service, no reader, the service going away, an
  older service without `DeleteEnrolledFinger`, a service that never
  answers, and named fingerprints saved to a real names file.

The end-to-end test and the demo service are a package of their own, `e2e/`
(its own workspace and `Cargo.lock`), so that building, testing and
packaging the application never needs validity-rs. `e2e/Cargo.toml` depends
on validity-rs's daemon by git URL, pinned by commit: cargo fetches it from
GitHub. To move to another validity-rs commit, change `rev` there and run
`cargo update -p validity-daemon --manifest-path e2e/Cargo.toml`. To try a
local validity-rs checkout instead, without touching the tracked files, add
`--config 'patch."https://github.com/validity-rs/validity-rs.git".validity-daemon.path="../validity-rs/crates/validity-daemon"'`
to the cargo command (then without `--locked`, and do not commit the
`e2e/Cargo.lock` it rewrites).

### CI

`ci/run.sh` is the whole CI, runnable locally (`ci/run.sh` for the quick
set, `ci/run.sh all` for everything, or named jobs; the script's header lists
them):

- `noai zizmor gitleaks` on the host;
- `fmt clippy doc test packaging e2e` in the pinned Debian 13 image
  (`docker/Dockerfile.debian`) when docker is available, or on the host with
  `CI_HOST=1`;
- `distro:<distro>`: `cargo test` on Debian 13, Fedora, Arch Linux and
  Alpine 3.24, each with its own GTK 4 and libadwaita
  (`docker/Dockerfile.<distro>`).

Every image is pinned by digest, every action by commit, every downloaded
tool by version and SHA-256. GitHub Actions (`.github/workflows/ci.yml`)
calls the same jobs.

## Architecture

- `src/machine.rs`: the state machine. User actions and service replies and
  signals go in as `Input`s; the view state (`Model`) and the service calls
  to make (`Command`s) come out. No GTK, no D-Bus.
- `src/fprint.rs`: runs the commands over D-Bus on a thread of its own and
  turns replies and signals into inputs, in bus order.
- `src/status.rs`, `src/error.rs`: what to tell the user for each fprintd
  result and each error.
- `src/labels.rs`: the names file. The machine owns the names in memory and
  says when they changed (`Model::take_labels_to_save`); the window saves
  them.
- `src/ui.rs`, `src/main.rs`: the GTK 4 / libadwaita window, which only
  renders the model and forwards clicks.

## Translations

Strings are written in English in the source, through `tr("…")`,
`trf("…", …)`, `msg("…")` or `n_("…")`, always with a string literal. The
catalogues are ordinary gettext PO files in `po/`, compiled into the binary
(no `msgfmt` needed, translated even from `cargo run`). The language follows
`LANGUAGE`, `LC_ALL`, `LC_MESSAGES` and `LANG`, like gettext.

To add a language, for example German:

1. Copy `po/fr.po` to `po/de.po`, set `Language: de` in its header, and
   replace every `msgstr` with the German text (Poedit or any PO editor
   works). Keep the `{placeholders}` as they are.
2. Add `("de", include_str!("../po/de.po"))` to `CATALOGUES` in
   `src/i18n.rs`.
3. Add `Name[de]`, `GenericName[de]`, `Comment[de]` and `Keywords[de]` to the
   desktop entry, and `xml:lang="de"` elements to the metainfo.
4. Run `cargo test --test translations`: it lists anything missing.

## Accessibility

- Large text: the window's text is 15% larger than the desktop's, the
  instruction during enrolment larger still, and the buttons are big.
- Keyboard: every action is reachable with Tab and Enter; the main button of
  each page has the focus when the page opens ("Start" on the add page, so
  Enter adds the suggested finger); Escape cancels or goes back; in the
  removal confirmation, the safe choice ("Cancel") is the default; in the
  rename dialog, Enter renames.
- Screen readers: every control has a text label; each "Rename" and "Remove"
  button is named after its fingerprint; the name field is labelled "Name"; instructions and results are announced when they
  change; the progress bar reads "Step 3 of 6".
- Nothing is told by colour alone: results have an icon *and* words; the
  selected finger on the add page has a check mark, and a finger already
  registered says so in words.
- Closing the window during an enrolment or a test stops it and releases the
  reader first.

## License

MIT, see [LICENSE](LICENSE).
