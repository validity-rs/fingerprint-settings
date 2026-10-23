//! The window: shows the [`Model`] and turns clicks into [`Input`]s.
//!
//! All the logic is in the library's state machine; this module only builds
//! widgets, renders the model into them, and wires the D-Bus driver's inputs
//! into the GLib main loop.
//!
//! Accessibility: every control has a visible text label (no icon-only
//! button), screen readers get the finger name with each "Remove" button,
//! instructions and results are announced when they change, every action is
//! reachable with the keyboard (Tab, Enter, Escape to cancel or go back),
//! and nothing is told by colour alone: results carry an icon *and* words.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use fingerprint_settings::APP_ID;
use fingerprint_settings::error::{ErrorKind, ServiceError};
use fingerprint_settings::fingers::{Finger, Hand};
use fingerprint_settings::fprint::{Bus, Service};
use fingerprint_settings::i18n::{tr, trf};
use fingerprint_settings::machine::{
    Dialog, EnrollOutcome, Failure, Input, Model, Page, TestOutcome,
};
use fingerprint_settings::message::{Arg, Message, msg};
use gtk::{gdk, glib};

/// Larger text than the desktop default, for readability; the instruction
/// larger still.
const CSS: &str = "
window.fingerprint-settings { font-size: 1.15em; }
.instruction { font-size: 1.6em; font-weight: bold; }
.big-button { font-size: 1.1em; min-height: 44px; padding-left: 28px; padding-right: 28px; }
progressbar.big trough, progressbar.big progress { min-height: 14px; }
";

const FINGER_ICON: &str = "auth-fingerprint-symbolic";
const OK_ICON: &str = "object-select-symbolic";
const WARNING_ICON: &str = "dialog-warning-symbolic";
const ERROR_ICON: &str = "dialog-error-symbolic";

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

fn text(m: &Message) -> String {
    m.text()
}

fn label(s: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(s)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .build();
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn button(s: &str, suggested: bool) -> gtk::Button {
    let b = gtk::Button::builder()
        .label(s)
        .halign(gtk::Align::Center)
        .build();
    b.add_css_class("pill");
    b.add_css_class("big-button");
    if suggested {
        b.add_css_class("suggested-action");
    }
    b
}

fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}

fn page_box() -> gtk::Box {
    let b = vbox(24);
    b.set_margin_top(24);
    b.set_margin_bottom(24);
    b.set_margin_start(18);
    b.set_margin_end(18);
    b.set_valign(gtk::Align::Center);
    b
}

fn clamped(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder().maximum_size(560).child(child).build();
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build()
}

fn big_icon(name: &str) -> gtk::Image {
    let i = gtk::Image::from_icon_name(name);
    i.set_pixel_size(96);
    i.set_accessible_role(gtk::AccessibleRole::Presentation);
    i
}

fn set_accessible_label(w: &impl IsA<gtk::Accessible>, s: &str) {
    w.update_property(&[gtk::accessible::Property::Label(s)]);
}

/// The technical detail, folded away.
struct Details {
    expander: gtk::Expander,
    label: gtk::Label,
}

impl Details {
    fn new() -> Self {
        let label = gtk::Label::builder()
            .selectable(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .build();
        label.add_css_class("dim-label");
        label.add_css_class("monospace");
        let expander = gtk::Expander::builder()
            .label(tr("Technical details"))
            .child(&label)
            .halign(gtk::Align::Center)
            .build();
        Details { expander, label }
    }

    fn show(&self, detail: Option<&str>) {
        self.expander.set_visible(detail.is_some());
        self.expander.set_expanded(false);
        self.label.set_label(detail.unwrap_or_default());
    }
}

struct Widgets {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,

    problem: adw::StatusPage,
    problem_retry: gtk::Button,
    problem_details: Details,

    empty_add: gtk::Button,

    fingers_group: adw::PreferencesGroup,
    fingers_rows: RefCell<Vec<adw::ActionRow>>,
    fingers_add: gtk::Button,
    fingers_test: gtk::Button,
    reader: gtk::Label,

    choose_rows: Vec<(Finger, adw::ActionRow, gtk::Image)>,

    enroll_title: gtk::Label,
    enroll_instruction: gtk::Label,
    enroll_progress: gtk::ProgressBar,
    enroll_step: gtk::Label,
    enroll_cancel: gtk::Button,

    test_instruction: gtk::Label,
    test_cancel: gtk::Button,

    result: adw::StatusPage,
    result_buttons: gtk::Box,
    result_details: Details,
}

/// The window and its state.
struct Ui {
    model: RefCell<Model>,
    service: Option<Service>,
    w: Widgets,
    /// The page shown, by name.
    shown: Cell<&'static str>,
    /// The fingers the list shows, to rebuild it only when they change.
    listed: RefCell<Option<Vec<Finger>>>,
    /// The last instruction or result announced.
    announced: RefCell<String>,
    /// What the result page shows, to rebuild its buttons only on change.
    result_key: RefCell<String>,
    /// The dialog presented, if any.
    dialog: RefCell<Option<(Dialog, adw::AlertDialog)>>,
    /// The window was asked to close while busy: close once idle.
    quitting: Cell<bool>,
}

impl Ui {
    fn new(app: &adw::Application) -> Rc<Ui> {
        let service = Service::start(Bus::System).ok();
        let w = Self::build(app);
        let ui = Rc::new(Ui {
            model: RefCell::new(Model::new()),
            service,
            w,
            shown: Cell::new(""),
            listed: RefCell::new(None),
            announced: RefCell::new(String::new()),
            result_key: RefCell::new(String::new()),
            dialog: RefCell::new(None),
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
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vhomogeneous(false)
            .build();

        // Loading.
        let b = page_box();
        let spinner = adw::Spinner::new();
        spinner.set_size_request(48, 48);
        b.append(&spinner);
        b.append(&label(
            tr("Looking for the fingerprint reader…"),
            &["title-3"],
        ));
        stack.add_named(&b, Some("loading"));

        // A problem.
        let problem = adw::StatusPage::builder().icon_name(WARNING_ICON).build();
        let pb = vbox(18);
        let problem_retry = button(tr("Try again"), true);
        pb.append(&problem_retry);
        let problem_details = Details::new();
        pb.append(&problem_details.expander);
        problem.set_child(Some(&pb));
        stack.add_named(&problem, Some("problem"));

        // No fingers yet.
        let empty = adw::StatusPage::builder()
            .icon_name(FINGER_ICON)
            .title(tr("No fingerprints yet"))
            .description(tr(
                "A registered finger lets you unlock this computer by simply touching the fingerprint reader, instead of typing your password.",
            ))
            .build();
        let eb = vbox(24);
        let empty_add = button(tr("Add a finger"), true);
        eb.append(&empty_add);
        eb.append(&label(
            tr("Your password always keeps working: you can still use it at any time."),
            &["heading"],
        ));
        empty.set_child(Some(&eb));
        stack.add_named(&empty, Some("empty"));

        // The fingers.
        let fb = vbox(24);
        fb.set_margin_top(24);
        fb.set_margin_bottom(24);
        fb.set_margin_start(18);
        fb.set_margin_end(18);
        let fingers_group = adw::PreferencesGroup::builder()
            .title(tr("Your registered fingers"))
            .description(tr(
                "Touch the reader with one of these fingers to unlock the computer.",
            ))
            .build();
        fb.append(&fingers_group);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        actions.set_halign(gtk::Align::Center);
        let fingers_add = button(tr("Add a finger"), true);
        let fingers_test = button(tr("Test a finger"), false);
        actions.append(&fingers_add);
        actions.append(&fingers_test);
        fb.append(&actions);
        fb.append(&label(
            tr("Your password always keeps working: you can still use it at any time."),
            &["heading"],
        ));
        let reader = label("", &["dim-label", "caption"]);
        fb.append(&reader);
        stack.add_named(&clamped(&fb), Some("fingers"));

        // Choosing a finger.
        let cb = vbox(18);
        cb.set_margin_top(24);
        cb.set_margin_bottom(24);
        cb.set_margin_start(18);
        cb.set_margin_end(18);
        cb.append(&label(tr("Which finger do you want to add?"), &["title-2"]));
        cb.append(&label(
            tr("Choose a finger that is easy to place on the reader, such as an index finger."),
            &[],
        ));
        let mut choose_rows = Vec::new();
        for hand in Hand::ALL {
            let g = adw::PreferencesGroup::builder()
                .title(tr(hand.name()))
                .build();
            for f in hand.fingers() {
                let row = adw::ActionRow::builder()
                    .title(tr(f.name()))
                    .activatable(true)
                    .build();
                let mark = gtk::Image::from_icon_name("go-next-symbolic");
                mark.set_accessible_role(gtk::AccessibleRole::Presentation);
                row.add_suffix(&mark);
                g.add(&row);
                choose_rows.push((f, row, mark));
            }
            cb.append(&g);
        }
        let choose_back = button(tr("Back"), false);
        choose_back.set_action_name(Some("win.cancel"));
        cb.append(&choose_back);
        stack.add_named(&clamped(&cb), Some("choose"));

        // Enrolling.
        let nb = page_box();
        let enroll_title = label("", &["title-2"]);
        nb.append(&enroll_title);
        nb.append(&big_icon(FINGER_ICON));
        let enroll_instruction = label("", &["instruction"]);
        nb.append(&enroll_instruction);
        let enroll_step = label("", &["title-3"]);
        nb.append(&enroll_step);
        let enroll_progress = gtk::ProgressBar::new();
        enroll_progress.add_css_class("big");
        set_accessible_label(&enroll_progress, tr("Progress"));
        nb.append(&enroll_progress);
        nb.append(&label(
            tr("Lift and place your finger several times, moving it slightly each time."),
            &[],
        ));
        let enroll_cancel = button(tr("Cancel"), false);
        enroll_cancel.set_action_name(Some("win.cancel"));
        nb.append(&enroll_cancel);
        stack.add_named(&clamped(&nb), Some("enroll"));

        // Testing.
        let tb = page_box();
        tb.append(&label(tr("Test a finger"), &["title-2"]));
        tb.append(&big_icon(FINGER_ICON));
        let test_instruction = label("", &["instruction"]);
        tb.append(&test_instruction);
        let test_cancel = button(tr("Cancel"), false);
        test_cancel.set_action_name(Some("win.cancel"));
        tb.append(&test_cancel);
        stack.add_named(&clamped(&tb), Some("test"));

        // A result.
        let result = adw::StatusPage::new();
        let rb = vbox(18);
        let result_buttons = vbox(12);
        result_buttons.set_halign(gtk::Align::Center);
        rb.append(&result_buttons);
        let result_details = Details::new();
        rb.append(&result_details.expander);
        result.set_child(Some(&rb));
        stack.add_named(&result, Some("result"));

        // Removing.
        let rm = page_box();
        let spinner = adw::Spinner::new();
        spinner.set_size_request(48, 48);
        rm.append(&spinner);
        rm.append(&label(tr("Removing the fingerprint…"), &["title-3"]));
        stack.add_named(&rm, Some("removing"));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&toasts));
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(tr("Fingerprints"))
            .icon_name(APP_ID)
            .default_width(600)
            .default_height(720)
            .content(&view)
            .build();
        window.add_css_class("fingerprint-settings");

        Widgets {
            window,
            toasts,
            stack,
            problem,
            problem_retry,
            problem_details,
            empty_add,
            fingers_group,
            fingers_rows: RefCell::new(Vec::new()),
            fingers_add,
            fingers_test,
            reader,
            choose_rows,
            enroll_title,
            enroll_instruction,
            enroll_progress,
            enroll_step,
            enroll_cancel,
            test_instruction,
            test_cancel,
            result,
            result_buttons,
            result_details,
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
        let f = self.on(Input::TryAgain);
        w.problem_retry.connect_clicked(move |_| f());
        let f = self.on(Input::AddFinger);
        w.empty_add.connect_clicked(move |_| f());
        let f = self.on(Input::AddFinger);
        w.fingers_add.connect_clicked(move |_| f());
        let f = self.on(Input::TestFinger);
        w.fingers_test.connect_clicked(move |_| f());
        for (finger, row, _) in &w.choose_rows {
            let f = self.on(Input::ChooseFinger(*finger));
            row.connect_activated(move |_| f());
        }

        // "Cancel" / "Back", also on Escape.
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
                m.page = Page::Problem(Failure::service(&e));
            }
            None => {}
        }
        self.render();
        if self.quitting.get() && !self.model.borrow().holds_reader() {
            self.w.window.close();
        }
    }

    fn render(self: &Rc<Self>) {
        // A copy: GTK calls below may run handlers that dispatch again.
        let (m, notice) = {
            let mut model = self.model.borrow_mut();
            let notice = model.take_notice();
            (model.clone(), notice)
        };
        let name = match &m.page {
            Page::Loading => "loading",
            Page::Problem(f) => {
                self.render_problem(f);
                "problem"
            }
            Page::Fingers if m.fingers.is_empty() => "empty",
            Page::Fingers => {
                self.render_fingers(&m);
                "fingers"
            }
            Page::Choose => {
                self.render_choose(&m);
                "choose"
            }
            Page::Enroll(e) => match &e.outcome {
                Some(o) if e.step == fingerprint_settings::machine::Step::Done => {
                    self.render_enroll_result(e.finger, o, &m);
                    "result"
                }
                _ => {
                    self.w.enroll_title.set_label(&text(&e.title()));
                    self.w.enroll_instruction.set_label(&text(&e.instruction));
                    match e.fraction() {
                        Some(f) => self.w.enroll_progress.set_fraction(f),
                        None => self.w.enroll_progress.pulse(),
                    }
                    let step = text(&e.progress_text());
                    self.w.enroll_step.set_label(&step);
                    self.w
                        .enroll_progress
                        .update_property(&[gtk::accessible::Property::ValueText(&step)]);
                    self.announce(&self.w.enroll_instruction, &text(&e.instruction));
                    "enroll"
                }
            },
            Page::Test(t) => match &t.outcome {
                Some(o) if t.step == fingerprint_settings::machine::Step::Done => {
                    self.render_test_result(o);
                    "result"
                }
                _ => {
                    self.w.test_instruction.set_label(&text(&t.instruction));
                    self.announce(&self.w.test_instruction, &text(&t.instruction));
                    "test"
                }
            },
            Page::Removing(_) => "removing",
        };
        if self.shown.get() != name {
            self.shown.set(name);
            self.w.stack.set_visible_child_name(name);
            self.focus(name);
        }
        if let Some(n) = notice {
            self.w.toasts.add_toast(adw::Toast::new(&text(&n)));
        }
        self.render_dialog(m.dialog.as_ref());
    }

    /// Puts the keyboard focus on the page's main control.
    fn focus(&self, page: &str) {
        let w = &self.w;
        match page {
            "problem" => {
                w.problem_retry.grab_focus();
            }
            "empty" => {
                w.empty_add.grab_focus();
            }
            "fingers" => {
                w.fingers_add.grab_focus();
            }
            "choose" => {
                if let Some((_, row, _)) = w.choose_rows.iter().find(|(_, r, _)| r.is_sensitive()) {
                    row.grab_focus();
                }
            }
            "enroll" => {
                w.enroll_cancel.grab_focus();
            }
            "test" => {
                w.test_cancel.grab_focus();
            }
            "result" => {
                if let Some(b) = w.result_buttons.first_child() {
                    b.grab_focus();
                }
            }
            _ => {}
        }
    }

    /// Tells screen readers about a new instruction or result.
    fn announce(&self, w: &impl IsA<gtk::Widget>, s: &str) {
        if *self.announced.borrow() != s {
            *self.announced.borrow_mut() = s.to_string();
            w.upcast_ref::<gtk::Widget>()
                .announce(s, gtk::AccessibleAnnouncementPriority::Medium);
        }
    }

    fn render_problem(&self, f: &Failure) {
        self.w.problem.set_title(&text(&f.title));
        self.w.problem.set_description(Some(&text(&f.description)));
        self.w.problem_details.show(f.detail.as_deref());
        self.announce(&self.w.problem, &text(&f.title));
    }

    fn render_fingers(self: &Rc<Self>, m: &Model) {
        let w = &self.w;
        w.fingers_add
            .set_visible(m.fingers.len() < Finger::ALL.len());
        let reader = m
            .device
            .as_ref()
            .map(|d| d.name.as_str())
            .filter(|n| !n.is_empty());
        w.reader.set_visible(reader.is_some());
        if let Some(name) = reader {
            w.reader
                .set_label(&trf("Fingerprint reader: {name}", &[("name", name)]));
        }
        if self.listed.borrow().as_ref() == Some(&m.fingers) {
            return;
        }
        *self.listed.borrow_mut() = Some(m.fingers.clone());
        for row in w.fingers_rows.borrow_mut().drain(..) {
            w.fingers_group.remove(&row);
        }
        for f in &m.fingers {
            // Not a Tab stop of its own: it does nothing; its button does.
            let row = adw::ActionRow::builder()
                .title(tr(f.name()))
                .focusable(false)
                .build();
            let icon = gtk::Image::from_icon_name(FINGER_ICON);
            icon.set_accessible_role(gtk::AccessibleRole::Presentation);
            row.add_prefix(&icon);
            let remove = gtk::Button::builder()
                .label(tr("Remove"))
                .valign(gtk::Align::Center)
                .build();
            set_accessible_label(
                &remove,
                &text(&msg("Remove: {finger}").with("finger", Arg::Finger(*f))),
            );
            let handler = self.on(Input::RemoveFinger(*f));
            remove.connect_clicked(move |_| handler());
            row.add_suffix(&remove);
            w.fingers_group.add(&row);
            w.fingers_rows.borrow_mut().push(row);
        }
    }

    fn render_choose(&self, m: &Model) {
        for (f, row, mark) in &self.w.choose_rows {
            let enrolled = m.is_enrolled(*f);
            row.set_sensitive(!enrolled);
            row.set_subtitle(if enrolled {
                tr("Already registered")
            } else {
                ""
            });
            mark.set_icon_name(Some(if enrolled {
                OK_ICON
            } else {
                "go-next-symbolic"
            }));
        }
    }

    /// Fills the result page: icon, title, text, and the buttons (rebuilt
    /// only when the result changes, so that focus is not lost).
    fn show_result(
        self: &Rc<Self>,
        icon: &str,
        title: &Message,
        body: &Message,
        detail: Option<&str>,
        buttons: &[(Message, Input, bool)],
    ) {
        let w = &self.w;
        let title = text(title);
        let key = format!("{icon}\n{title}\n{}", text(body));
        w.result.set_icon_name(Some(icon));
        w.result.set_title(&title);
        w.result.set_description(Some(&text(body)));
        w.result_details.show(detail);
        self.announce(&w.result, &title);
        if *self.result_key.borrow() == key {
            return;
        }
        *self.result_key.borrow_mut() = key;
        while let Some(c) = w.result_buttons.first_child() {
            w.result_buttons.remove(&c);
        }
        for (label, input, suggested) in buttons {
            let b = button(&text(label), *suggested);
            let f = self.on(input.clone());
            b.connect_clicked(move |_| f());
            w.result_buttons.append(&b);
        }
        if self.shown.get() == "result" {
            self.focus("result");
        }
    }

    fn render_enroll_result(self: &Rc<Self>, finger: Finger, o: &EnrollOutcome, m: &Model) {
        let back = (msg("Back to my fingerprints"), Input::Cancel, false);
        match o {
            EnrollOutcome::Completed => {
                let mut buttons = vec![(msg("Test a finger"), Input::TestFinger, true)];
                if m.fingers.len() < Finger::ALL.len() {
                    buttons.push((msg("Add another finger"), Input::AddFinger, false));
                }
                buttons.push(back);
                self.show_result(
                    OK_ICON,
                    &msg("Your {finger} is registered").with("finger", Arg::FingerInSentence(finger)),
                    &msg("You can now touch the reader instead of typing your password. Your password still works too."),
                    None,
                    &buttons,
                );
            }
            EnrollOutcome::Failed { failure, duplicate } => {
                let first = if *duplicate {
                    (msg("Choose another finger"), Input::AddFinger, true)
                } else {
                    (msg("Try again"), Input::TryAgain, true)
                };
                self.show_result(
                    ERROR_ICON,
                    &failure.title,
                    &failure.description,
                    failure.detail.as_deref(),
                    &[first, back],
                );
            }
            // Never shown: a cancelled enrolment goes back to the list.
            EnrollOutcome::Cancelled => {}
        }
    }

    fn render_test_result(self: &Rc<Self>, o: &TestOutcome) {
        let Some((title, body)) = o.texts() else {
            return;
        };
        let back = (msg("Back to my fingerprints"), Input::Cancel, false);
        let (icon, detail, again) = match o {
            TestOutcome::Match(_) => (OK_ICON, None, msg("Test again")),
            TestOutcome::NoMatch => (WARNING_ICON, None, msg("Test again")),
            TestOutcome::Failed(f) => (ERROR_ICON, f.detail.as_deref(), msg("Try again")),
            TestOutcome::Cancelled => return,
        };
        self.show_result(
            icon,
            &title,
            &body,
            detail,
            &[(again, Input::TryAgain, true), back],
        );
    }

    fn render_dialog(self: &Rc<Self>, want: Option<&Dialog>) {
        let current = self.dialog.borrow().as_ref().map(|(d, _)| d.clone());
        if current.as_ref() == want {
            return;
        }
        // Take it out first: closing it runs its handler.
        let old = self.dialog.borrow_mut().take();
        if let Some((_, d)) = old {
            d.force_close();
        }
        let Some(want) = want else {
            return;
        };
        let (title, body, ok, cancel) = want.texts();
        let d = adw::AlertDialog::new(Some(&text(&title)), Some(&text(&body)));
        if let Some(c) = &cancel {
            d.add_response("cancel", &text(c));
            d.set_close_response("cancel");
        } else {
            d.set_close_response("ok");
        }
        d.add_response("ok", &text(&ok));
        match want {
            Dialog::ConfirmRemove(_) | Dialog::ConfirmRemoveAll(_) => {
                d.set_response_appearance("ok", adw::ResponseAppearance::Destructive);
                // The safe choice is the default.
                d.set_default_response(Some("cancel"));
            }
            Dialog::Error(_) => d.set_default_response(Some("ok")),
        }
        let weak: Weak<Ui> = Rc::downgrade(self);
        let shown = want.clone();
        d.connect_response(None, move |_, response| {
            let Some(ui) = weak.upgrade() else { return };
            // Only the dialog the model still wants answers.
            if ui.dialog.borrow().as_ref().map(|(d, _)| d) != Some(&shown) {
                return;
            }
            ui.dialog.borrow_mut().take();
            let input = if response == "ok" {
                Input::Confirm
            } else {
                Input::Dismiss
            };
            ui.dispatch(input);
        });
        *self.dialog.borrow_mut() = Some((want.clone(), d.clone()));
        d.present(Some(&self.w.window));
    }
}
