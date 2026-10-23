//! Fingerprints (`fingerprint-settings`): manage your own fingerprints with
//! any fprintd-compatible service. The GTK 4 / libadwaita front end of the
//! `fingerprint_settings` library.

use adw::prelude::*;
use fingerprint_settings::{APP_ID, i18n};

mod ui;

fn main() -> gtk::glib::ExitCode {
    i18n::init_from_env();
    gtk::glib::set_application_name(i18n::tr("Fingerprints"));
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(ui::activate);
    app.run()
}
