# Fingerprints (`fingerprint-settings`)

A small desktop application to manage **your own fingerprints**: register a
finger, test it, remove it. It is meant for desktops that have no fingerprint
settings of their own, such as **XFCE, MATE and LXQt**, and for people who
want something plain and readable.

- **Add a finger**: pick a finger, then follow large, plain instructions
  ("Lift your finger, then place it on the reader again") with a step counter
  ("Step 3 of 6").
- **Test a finger**: touch the reader and see "Recognised" or "Not recognised".
- **Remove a finger**, after a confirmation.
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
- To run the tests: `dbus-daemon` (Debian: `dbus-daemon`; Fedora, Arch,
  Alpine: `dbus`), and a checkout of validity-rs next to this one (see
  below).

Whether you may change your fingerprints is decided by the service's polkit
policy (`net.reactivated.fprint.device.enroll`). fprintd's default asks for
**your own password** before adding or removing a fingerprint, so a polkit
authentication agent must be running in the session (XFCE: `xfce-polkit` or
`policykit-1-gnome`; MATE: `mate-polkit`; LXQt: `lxqt-policykit`). Without
one, the application says you are not allowed. A call that may show this
prompt is given ten minutes; any other call one minute, after which the
application says the service did not answer.

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

`examples/demo_service.rs` runs validity-rs's daemon with its emulated sensor
on a private bus; the application finds it through `DBUS_SYSTEM_BUS_ADDRESS`:

```bash
dbus-daemon --session --nofork --print-address=1 > /tmp/demo-bus &
sleep 1; BUS=$(head -1 /tmp/demo-bus)
cargo run --example demo_service -- --address "$BUS" &
DBUS_SYSTEM_BUS_ADDRESS="$BUS" cargo run
# While the application waits for a finger, "touch" the emulated reader
# (finger number 1; another number is another finger):
gdbus call --address "$BUS" --dest io.github.validity_rs.FingerprintSettings.Demo \
    --object-path /io/github/validity_rs/FingerprintSettings/Demo \
    --method io.github.validity_rs.FingerprintSettings.Demo.Touch 1
```

## Tests

```bash
cargo test
```

- Unit tests of the UI-free core (`src/machine.rs` and its tests, `status`,
  `error`, `i18n`, `fingers`): every enrolment and verification result, every
  error path, cancelling at each step, and the rule that a claimed reader is
  always released.
- `tests/end_to_end.rs`: the same core and D-Bus driver as the window, against
  validity-rs's daemon with its emulated sensor on a private `dbus-daemon`:
  enrol, list, test, remove, duplicates, cancels, a busy reader, polkit
  refusals, no service, no reader, the service going away, and an older
  service without `DeleteEnrolledFinger`.
- `tests/translations.rs`: every user-visible string has a French
  translation, the catalogue has nothing stale, placeholders and French
  typography are right; the desktop entry and metainfo are translated.

The end-to-end test uses validity-rs as a path dev-dependency:
**validity-rs must be checked out next to this repository**
(`../validity-rs`). `ci/run.sh` clones it there at the revision this tree was
tested against when it is missing. Once validity-rs is published, the
dependency becomes a git (or crates.io) dependency and the sibling checkout
goes away.

### CI

`ci/run.sh` is the whole CI, runnable locally: `noai zizmor gitleaks fmt
clippy doc test packaging`. The Rust jobs run in the pinned Debian 13 image
(`docker/Dockerfile.debian`) when docker is available, or on the host with
`CI_HOST=1`. GitHub Actions (`.github/workflows/ci.yml`) calls the same jobs.

## Architecture

- `src/machine.rs`: the state machine. User actions and service replies and
  signals go in as `Input`s; the view state (`Model`) and the service calls
  to make (`Command`s) come out. No GTK, no D-Bus.
- `src/fprint.rs`: runs the commands over D-Bus on a thread of its own and
  turns replies and signals into inputs, in bus order.
- `src/status.rs`, `src/error.rs`: what to tell the user for each fprintd
  result and each error.
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
  each page has the focus when the page opens; Escape cancels or goes back;
  in the confirmation, the safe choice ("Cancel") is the default.
- Screen readers: every control has a text label; each "Remove" button is
  named after its finger; instructions and results are announced when they
  change; the progress bar reads "Step 3 of 6".
- Nothing is told by colour alone: results have an icon *and* words.
- Closing the window during an enrolment or a test stops it and releases the
  reader first.

## License

MIT, see [LICENSE](LICENSE).
