//! The window: one screen that shows the [`Model`] and turns clicks into
//! [`Input`]s.
//!
//! All the logic is in the library's state machine; this module only builds
//! widgets, renders the model into them, and wires the D-Bus driver's inputs
//! into the GLib main loop.
//!
//! Accessibility: every control has a visible text label, instructions and
//! messages are announced when they change, every action is reachable with
//! the keyboard (Tab, Enter, Escape to cancel), and nothing is told by colour
//! alone.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use fingerprint_settings::APP_ID;
use fingerprint_settings::error::{ErrorKind, ServiceError};
use fingerprint_settings::fprint::{Bus, Service};
use fingerprint_settings::i18n::tr;
use fingerprint_settings::machine::{Activity, Input, Model, Step};
use gtk::{gdk, glib};

/// Larger text than the desktop default, for readability; the instruction
/// larger still.
const CSS: &str = "
window.fingerprint-settings { font-size: 1.15em; }
.instruction { font-size: 1.6em; font-weight: bold; }
.big-button { font-size: 1.1em; min-height: 44px; padding-left: 28px; padding-right: 28px; }
progressbar.big trough, progressbar.big progress { min-height: 14px; }
";

/// Opens the window (or raises it).
pub fn activate(app: &adw::Application) {
    if let Some(w) = app.active_window() {
        w.present();
        return;
    }
    if let Some(display) = gdk::Display::default() {
        let css = gtk::CssProvider::new();
        css.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    gtk::Window::set_default_icon_name(APP_ID);
    let ui = Ui::new(app);
    ui.w.window.present();
    ui.dispatch(Input::Start);
}

fn label(classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::builder()
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .build();
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn button(s: &str, class: &str) -> gtk::Button {
    let b = gtk::Button::builder()
        .label(s)
        .halign(gtk::Align::Center)
        .build();
    b.add_css_class("pill");
    b.add_css_class("big-button");
    if !class.is_empty() {
        b.add_css_class(class);
    }
    b
}

/// The widgets.
struct Widgets {
    window: adw::ApplicationWindow,
    status: gtk::Label,
    spinner: adw::Spinner,
    instruction: gtk::Label,
    progress: gtk::ProgressBar,
    help: gtk::Label,
    message: gtk::Label,
    enroll: gtk::Button,
    cancel: gtk::Button,
    remove: gtk::Button,
}

/// The window and its state.
struct Ui {
    model: RefCell<Model>,
    service: Option<Service>,
    w: Widgets,
    /// The last instruction or message announced.
    announced: RefCell<String>,
    /// The window was asked to close while busy: close once idle.
    quitting: Cell<bool>,
}

impl Ui {
    fn new(app: &adw::Application) -> Rc<Ui> {
        let service = Service::start(Bus::System).ok();
        let ui = Rc::new(Ui {
            model: RefCell::new(Model::new()),
            service,
            w: Self::build(app),
            announced: RefCell::new(String::new()),
            quitting: Cell::new(false),
        });
        ui.connect();
        if let Some(s) = &ui.service {
            let inputs = s.inputs().clone();
            let weak = Rc::downgrade(&ui);
            glib::spawn_future_local(async move {
                while let Ok(i) = inputs.recv().await {
                    match weak.upgrade() {
                        Some(ui) => ui.dispatch(i),
                        None => break,
                    }
                }
            });
        }
        ui
    }

    fn build(app: &adw::Application) -> Widgets {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 24);
        b.set_margin_top(24);
        b.set_margin_bottom(24);
        b.set_margin_start(18);
        b.set_margin_end(18);
        b.set_valign(gtk::Align::Center);
        let icon = gtk::Image::from_icon_name("auth-fingerprint-symbolic");
        icon.set_pixel_size(96);
        icon.set_accessible_role(gtk::AccessibleRole::Presentation);
        b.append(&icon);
        let status = label(&["title-2"]);
        b.append(&status);
        let spinner = adw::Spinner::new();
        spinner.set_size_request(48, 48);
        b.append(&spinner);
        let instruction = label(&["instruction"]);
        b.append(&instruction);
        let progress = gtk::ProgressBar::new();
        progress.add_css_class("big");
        b.append(&progress);
        // One or two plain sentences for a first-time user.
        let help = label(&["body"]);
        b.append(&help);
        let message = label(&["heading"]);
        b.append(&message);
        let enroll = button(tr("Register my right index finger"), "suggested-action");
        b.append(&enroll);
        let cancel = button(tr("Cancel"), "");
        b.append(&cancel);
        let remove = button(tr("Remove the fingerprint"), "destructive-action");
        b.append(&remove);

        let clamp = adw::Clamp::builder().maximum_size(560).child(&b).build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&scrolled));
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(tr("Fingerprints"))
            .icon_name(APP_ID)
            .default_width(560)
            .default_height(560)
            .content(&view)
            .build();
        window.add_css_class("fingerprint-settings");

        Widgets {
            window,
            status,
            spinner,
            instruction,
            progress,
            help,
            message,
            enroll,
            cancel,
            remove,
        }
    }

    /// A handler that sends `input`.
    fn on(self: &Rc<Self>, input: Input) -> impl Fn() + 'static {
        let weak = Rc::downgrade(self);
        move || {
            if let Some(ui) = weak.upgrade() {
                ui.dispatch(input.clone());
            }
        }
    }

    fn connect(self: &Rc<Self>) {
        let w = &self.w;
        let f = self.on(Input::Enroll);
        w.enroll.connect_clicked(move |_| f());
        let f = self.on(Input::Cancel);
        w.cancel.connect_clicked(move |_| f());
        let f = self.on(Input::Remove);
        w.remove.connect_clicked(move |_| f());

        // "Cancel", also on Escape.
        let cancel = gtk::gio::SimpleAction::new("cancel", None);
        let f = self.on(Input::Cancel);
        cancel.connect_activate(move |_, _| f());
        w.window.add_action(&cancel);
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(gtk::Shortcut::new(
            Some(gtk::KeyvalTrigger::new(
                gdk::Key::Escape,
                gdk::ModifierType::empty(),
            )),
            Some(gtk::NamedAction::new("win.cancel")),
        ));
        w.window.add_controller(shortcuts);

        // The window owns this state: the handler keeps it alive until the
        // window is destroyed, then ends the D-Bus thread.
        let ui = self.clone();
        w.window.connect_destroy(move |_| {
            if let Some(s) = &ui.service {
                s.close();
            }
        });

        // Closing while the reader is in use: cancel, release, then close.
        let weak = Rc::downgrade(self);
        w.window.connect_close_request(move |window| {
            let Some(ui) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !ui.model.borrow().holds_reader() || ui.quitting.get() {
                return glib::Propagation::Proceed;
            }
            ui.quitting.set(true);
            ui.dispatch(Input::Cancel);
            // Should the service not answer, closing the connection releases
            // the reader anyway.
            let window = window.clone();
            glib::timeout_add_seconds_local_once(5, move || window.destroy());
            glib::Propagation::Stop
        });
    }

    /// Applies `input`, sends the resulting calls, shows the result.
    fn dispatch(self: &Rc<Self>, input: Input) {
        let cmds = self.model.borrow_mut().update(input);
        match &self.service {
            Some(s) => cmds.into_iter().for_each(|c| s.send(c)),
            None if !cmds.is_empty() => {
                // No driver thread: the same as no service at all.
                let e =
                    ServiceError::new(ErrorKind::Unavailable, "could not start the D-Bus thread");
                let mut m = self.model.borrow_mut();
                m.activity = Activity::Idle;
                m.message = Some(e.message());
            }
            None => {}
        }
        self.render();
        if self.quitting.get() && !self.model.borrow().holds_reader() {
            self.w.window.close();
        }
    }

    fn render(&self) {
        // A copy: GTK calls below may run handlers that dispatch again.
        let m = self.model.borrow().clone();
        let w = &self.w;
        let status = match (m.status(), &m.activity) {
            (Some(s), _) => s.text(),
            (None, Activity::Loading { .. }) => tr("Looking for the fingerprint reader…").into(),
            (None, _) => String::new(),
        };
        w.status.set_label(&status);
        w.status.set_visible(!status.is_empty());

        let instruction = match &m.activity {
            Activity::Enrolling(e) => {
                w.progress.set_fraction(e.fraction());
                e.instruction.text()
            }
            Activity::Removing(_) => tr("Removing the fingerprint…").into(),
            _ => String::new(),
        };
        w.instruction.set_label(&instruction);
        w.instruction.set_visible(!instruction.is_empty());
        w.progress
            .set_visible(matches!(m.activity, Activity::Enrolling(_)));
        let help = m.help().map(|h| h.text()).unwrap_or_default();
        w.help.set_label(&help);
        w.help.set_visible(!help.is_empty());
        let waiting = match &m.activity {
            Activity::Loading { .. } | Activity::Removing(_) => true,
            Activity::Enrolling(e) => e.step != Step::Scanning,
            Activity::Idle => false,
        };
        w.spinner.set_visible(waiting);

        let message = m.message.as_ref().map(|t| t.text()).unwrap_or_default();
        w.message.set_label(&message);
        w.message.set_visible(!message.is_empty());

        let enrolling = match &m.activity {
            Activity::Enrolling(e) => Some(e.cancellable()),
            _ => None,
        };
        w.enroll.set_visible(enrolling.is_none());
        w.enroll.set_sensitive(m.can_enroll());
        w.cancel.set_visible(enrolling.is_some());
        w.cancel.set_sensitive(enrolling == Some(true));
        w.remove
            .set_visible(m.shows_remove() && enrolling.is_none());
        w.remove.set_sensitive(m.can_remove());

        // The keyboard focus follows the one control that can be used.
        if enrolling == Some(true) {
            w.cancel.grab_focus();
        } else if m.can_enroll() {
            w.enroll.grab_focus();
        } else if m.can_remove() {
            w.remove.grab_focus();
        }

        // Screen readers hear what changed: the instruction, else the message.
        let said = if instruction.is_empty() {
            &message
        } else {
            &instruction
        };
        if !said.is_empty() && *self.announced.borrow() != *said {
            *self.announced.borrow_mut() = said.clone();
            w.window
                .upcast_ref::<gtk::Widget>()
                .announce(said, gtk::AccessibleAnnouncementPriority::Medium);
        }
    }
}
