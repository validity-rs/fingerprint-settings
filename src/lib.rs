//! The UI-free core of Fingerprints (`fingerprint-settings`): register or
//! remove your right index finger with any service that speaks fprintd's
//! D-Bus API (`net.reactivated.Fprint`).
//!
//! - [`machine`]: the state machine (user actions and service events in,
//!   view state and service calls out). Pure; unit-tested exhaustively.
//! - [`fprint`]: runs the service calls over D-Bus and turns replies and
//!   signals into inputs for the machine.
//! - [`status`]: fprintd's enrolment results and what to tell the user
//!   about each.
//! - [`error`]: service errors in plain language.
//! - [`message`], [`i18n`]: user-visible text and its translations.
//!
//! The GTK interface lives in the binary (`src/main.rs`, `src/ui.rs`).

pub mod error;
pub mod fprint;
pub mod i18n;
pub mod machine;
pub mod message;
pub mod status;

/// The application id (D-Bus name, desktop file, icon, AppStream).
pub const APP_ID: &str = "io.github.validity_rs.FingerprintSettings";
