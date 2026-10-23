//! The window: shows the [`Model`] and turns clicks into [`Input`]s.
//!
//! All the logic is in the library's state machine; this module only builds
//! widgets, renders the model into them, and wires the D-Bus driver's inputs
//! into the GLib main loop.
//!
//! Accessibility: every control has a visible text label (no icon-only
//! button), screen readers get the fingerprint's name with each "Rename" and
//! "Remove" button, the add page's selection has a check mark,
//! instructions and results are announced when they change, every action is
//! reachable with the keyboard (Tab, Enter, Escape to cancel or go back),
//! and nothing is told by colour alone: results carry an icon *and* words.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use fingerprint_settings::APP_ID;
use fingerprint_settings::error::{ErrorKind, ServiceError};
use fingerprint_settings::fingers::{self, Finger};
use fingerprint_settings::fprint::{Bus, Service};
use fingerprint_settings::i18n::{tr, trf};
use fingerprint_settings::labels::Store;
use fingerprint_settings::machine::{
    Dialog, EnrollOutcome, Failure, Input, Model, Page, TestOutcome,
};
use fingerprint_settings::message::{Message, msg};
use gtk::{gdk, glib};

/// Larger text than the desktop default, for readability; the instruction
/// larger still.
const CSS: &str = "
window.fingerprint-settings { font-size: 1.15em; }
.instruction { font-size: 1.6em; font-weight: bold; }
.big-button { font-size: 1.1em; min-height: 44px; padding-left: 28px; padding-right: 28px; }
.choice { font-size: 1.15em; min-height: 56px; }
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

/// One choice of the add page: a toggle button with a check mark (shown
/// when selected, so that the selection is not told by its shade alone)
/// and its text.
struct Choice {
    /// The finger, or `None` for "just a name, any finger".
    finger: Option<Finger>,
    button: gtk::ToggleButton,
    text: gtk::Label,
}

impl Choice {
    fn new(finger: Option<Finger>, text: &str) -> Self {
        let check = gtk::Image::from_icon_name(OK_ICON);
        check.set_visible(false);
        check.set_accessible_role(gtk::AccessibleRole::Presentation);
        // Wraps only when the button is too narrow for the text.
        let label = gtk::Label::builder()
            .label(text)
            .wrap(true)
            .natural_wrap_mode(gtk::NaturalWrapMode::None)
            .justify(gtk::Justification::Center)
            .build();
        let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        inner.set_halign(gtk::Align::Center);
        inner.append(&check);
        inner.append(&label);
        let button = gtk::ToggleButton::builder()
            .child(&inner)
            .hexpand(true)
            .build();
        button.add_css_class("choice");
        set_accessible_label(&button, text);
        let c = check.clone();
        button.connect_toggled(move |b| c.set_visible(b.is_active()));
        Choice {
            finger,
            button,
            text: label,
        }
    }

    fn set_text(&self, text: &str) {
        self.text.set_label(text);
        set_accessible_label(&self.button, text);
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

    /// The add page's choices: a finger, or `None` for "just a name".
    add_choices: Vec<Choice>,
    add_others: gtk::Expander,
    add_name: gtk::Entry,
    add_start: gtk::Button,

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
    /// The fingers and names the list shows, to rebuild it only on change.
    listed: RefCell<Option<(Vec<Finger>, fingerprint_settings::labels::Labels)>>,
    /// Where the names are kept (`None`: no home directory).
    store: RefCell<Option<Store>>,
    /// The add page's name was typed by the user: choosing another finger
    /// no longer replaces it.
    name_edited: Cell<bool>,
    /// The add page is being filled in by the program, not the user.
    filling: Cell<bool>,
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
        // The names: a missing or broken file is no names, and a warning.
        let mut store = Store::from_env();
        let labels = match store.as_mut() {
            Some(s) => {
                let loaded = s.load();
                for warning in &loaded.warnings {
                    glib::g_warning!("fingerprint-settings", "{warning}");
                }
                loaded.labels
            }
            None => Default::default(),
        };
        let ui = Rc::new(Ui {
            model: RefCell::new(Model::with_labels(labels)),
            service,
            w,
            shown: Cell::new(""),
            listed: RefCell::new(None),
            store: RefCell::new(store),
            name_edited: Cell::new(false),
            filling: Cell::new(false),
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
        let empty_add = button(tr("Add a fingerprint"), true);
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
            .title(tr("Your fingerprints"))
            .description(tr(
                "Touch the reader with one of these fingers to unlock the computer.",
            ))
            .build();
        fb.append(&fingers_group);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        actions.set_halign(gtk::Align::Center);
        let fingers_add = button(tr("Add a fingerprint"), true);
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

        // Adding a fingerprint: which finger, and its name.
        let ab = vbox(18);
        ab.set_margin_top(24);
        ab.set_margin_bottom(24);
        ab.set_margin_start(18);
        ab.set_margin_end(18);
        ab.append(&label(tr("Add a fingerprint"), &["title-2"]));
        ab.append(&label(
            tr("Which finger? An index or a thumb is the easiest."),
            &[],
        ));
        let mut add_choices: Vec<Choice> = Vec::new();
        let grid = |cols: usize, fingers: &[Finger], out: &mut Vec<Choice>| {
            let g = gtk::Grid::builder()
                .column_spacing(12)
                .row_spacing(12)
                .column_homogeneous(true)
                .build();
            for (i, f) in fingers.iter().enumerate() {
                let c = Choice::new(Some(*f), tr(f.name()));
                g.attach(&c.button, (i % cols) as i32, (i / cols) as i32, 1, 1);
                out.push(c);
            }
            g
        };
        ab.append(&grid(2, &fingers::COMMON, &mut add_choices));
        let others_grid = grid(2, &fingers::OTHERS, &mut add_choices);
        others_grid.set_margin_top(12);
        let add_others = gtk::Expander::builder()
            .label(tr("Other finger…"))
            .child(&others_grid)
            .build();
        ab.append(&add_others);
        let any = Choice::new(None, tr("Just a name, any finger"));
        ab.append(&any.button);
        add_choices.push(any);
        // One choice at a time.
        let leader = add_choices[0].button.clone();
        for c in &add_choices[1..] {
            c.button.set_group(Some(&leader));
        }
        let name_label = label(tr("Name"), &["heading"]);
        name_label.set_xalign(0.0);
        name_label.set_margin_top(6);
        ab.append(&name_label);
        let add_name = gtk::Entry::builder()
            .placeholder_text(tr("For example: Thumb, when I'm in bed"))
            .max_length(fingerprint_settings::labels::MAX_LEN as i32)
            .build();
        add_name.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
            name_label.upcast_ref()
        ])]);
        ab.append(&add_name);
        let hint = label(tr("This is how the fingerprint appears in the list."), &[]);
        hint.set_xalign(0.0);
        hint.set_justify(gtk::Justification::Left);
        ab.append(&hint);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        buttons.set_halign(gtk::Align::Center);
        buttons.set_margin_top(6);
        let add_back = button(tr("Back"), false);
        add_back.set_action_name(Some("win.cancel"));
        let add_start = button(tr("Start"), true);
        buttons.append(&add_back);
        buttons.append(&add_start);
        ab.append(&buttons);
        stack.add_named(&clamped(&ab), Some("add"));

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
            add_choices,
            add_others,
            add_name,
            add_start,
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
        // The add page: a choice fills in its name, unless the user typed one.
        for c in &w.add_choices {
            let weak = Rc::downgrade(self);
            let choice = c.finger;
            c.button.connect_toggled(move |b| {
                let Some(ui) = weak.upgrade() else { return };
                if b.is_active() && !ui.filling.get() && !ui.name_edited.get() {
                    ui.fill_name(choice);
                }
                ui.update_start();
            });
        }
        let weak = Rc::downgrade(self);
        w.add_name.connect_changed(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            if !ui.filling.get() {
                ui.name_edited.set(true);
            }
            ui.update_start();
        });
        let weak = Rc::downgrade(self);
        w.add_name.connect_activate(move |_| {
            if let Some(ui) = weak.upgrade()
                && ui.w.add_start.is_sensitive()
            {
                ui.w.add_start.emit_clicked();
            }
        });
        let weak = Rc::downgrade(self);
        w.add_start.connect_clicked(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(finger) = ui.add_choice() else {
                return;
            };
            let text = ui.w.add_name.text().trim().to_string();
            // The standard name is not stored: it follows the language.
            let label = match finger {
                Some(f) if text.is_empty() || text == tr(f.name()) => None,
                _ => Some(text),
            };
            ui.dispatch(Input::StartEnroll { finger, label });
        });

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

    /// The add page's selected choice: `Some(Some(finger))`, `Some(None)`
    /// for "just a name", `None` when nothing is selected.
    fn add_choice(&self) -> Option<Option<Finger>> {
        self.w
            .add_choices
            .iter()
            .find(|c| c.button.is_active())
            .map(|c| c.finger)
    }

    /// Puts the standard name of `choice` in the name field.
    fn fill_name(&self, choice: Option<Finger>) {
        self.filling.set(true);
        self.w
            .add_name
            .set_text(choice.map(|f| tr(f.name())).unwrap_or_default());
        self.filling.set(false);
    }

    /// "Start" needs a free finger, or a name when there is no finger.
    fn update_start(&self) {
        let ok = match self.add_choice() {
            Some(Some(f)) => !self.model.borrow().is_enrolled(f),
            Some(None) => !self.w.add_name.text().trim().is_empty(),
            None => false,
        };
        self.w.add_start.set_sensitive(ok);
    }

    /// Applies `input`, sends the resulting calls, shows the result.
    fn dispatch(self: &Rc<Self>, input: Input) {
        let cmds = self.model.borrow_mut().update(input);
        let to_save = self.model.borrow_mut().take_labels_to_save();
        if let Some(labels) = to_save {
            let saved = match self.store.borrow_mut().as_mut() {
                Some(store) => store
                    .save(&labels)
                    .map_err(|e| format!("{}: {e}", store.path().display())),
                None => Err("no configuration directory (HOME is not set)".to_string()),
            };
            if let Err(e) = saved {
                glib::g_warning!("fingerprint-settings", "the names were not saved: {e}");
                self.w.toasts.add_toast(adw::Toast::new(&text(&msg(
                    "The names could not be saved.",
                ))));
            }
        }
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
            Page::Add => {
                self.render_add(&m, self.shown.get() != "add");
                "add"
            }
            Page::Enroll(e) => match &e.outcome {
                Some(o) if e.step == fingerprint_settings::machine::Step::Done => {
                    self.render_enroll_result(e, o, &m);
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
                    self.render_test_result(o, &m);
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
        self.render_dialog(m.dialog.as_ref(), &m);
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
            "add" => {
                w.add_start.grab_focus();
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
        let key = (m.fingers.clone(), m.labels.clone());
        if self.listed.borrow().as_ref() == Some(&key) {
            return;
        }
        *self.listed.borrow_mut() = Some(key);
        for row in w.fingers_rows.borrow_mut().drain(..) {
            w.fingers_group.remove(&row);
        }
        for f in &m.fingers {
            // Not a Tab stop of its own: it does nothing; its buttons do.
            // A name the user typed: plain text, never markup.
            let row = adw::ActionRow::builder()
                .title(m.display_name(*f).text())
                .use_markup(false)
                .focusable(false)
                .build();
            let icon = gtk::Image::from_icon_name(FINGER_ICON);
            icon.set_accessible_role(gtk::AccessibleRole::Presentation);
            row.add_prefix(&icon);
            let rename = gtk::Button::builder()
                .label(tr("Rename"))
                .valign(gtk::Align::Center)
                .build();
            set_accessible_label(
                &rename,
                &text(&msg("Rename: {name}").with("name", m.display_name(*f))),
            );
            let handler = self.on(Input::RenameFinger(*f));
            rename.connect_clicked(move |_| handler());
            row.add_suffix(&rename);
            let remove = gtk::Button::builder()
                .label(tr("Remove"))
                .valign(gtk::Align::Center)
                .build();
            set_accessible_label(
                &remove,
                &text(&msg("Remove: {name}").with("name", m.display_name(*f))),
            );
            let handler = self.on(Input::RemoveFinger(*f));
            remove.connect_clicked(move |_| handler());
            row.add_suffix(&remove);
            w.fingers_group.add(&row);
            w.fingers_rows.borrow_mut().push(row);
        }
    }

    /// The add page. `entering`: the page was just opened, so it starts
    /// afresh on the first free finger (indexes and thumbs first).
    fn render_add(&self, m: &Model, entering: bool) {
        for c in &self.w.add_choices {
            let Some(f) = c.finger else { continue };
            let enrolled = m.is_enrolled(f);
            c.button.set_sensitive(!enrolled);
            // Said in words, not only by the greyed-out look.
            let label = if enrolled {
                format!("{}\n{}", tr(f.name()), tr("Already registered"))
            } else {
                tr(f.name()).to_string()
            };
            c.set_text(&label);
        }
        if entering {
            let first = m.default_choice();
            self.filling.set(true);
            if let Some(c) = self.w.add_choices.iter().find(|c| c.finger == first) {
                c.button.set_active(true);
            }
            self.filling.set(false);
            self.fill_name(first);
            self.name_edited.set(false);
            self.w
                .add_others
                .set_expanded(first.is_some_and(|f| !fingers::COMMON.contains(&f)));
        }
        self.update_start();
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

    fn render_enroll_result(
        self: &Rc<Self>,
        e: &fingerprint_settings::machine::Enroll,
        o: &EnrollOutcome,
        m: &Model,
    ) {
        let back = (msg("Back to my fingerprints"), Input::Cancel, false);
        match o {
            EnrollOutcome::Completed => {
                let mut buttons = vec![(msg("Test a finger"), Input::TestFinger, true)];
                if m.fingers.len() < Finger::ALL.len() {
                    buttons.push((msg("Add another fingerprint"), Input::AddFinger, false));
                }
                buttons.push(back);
                self.show_result(
                    OK_ICON,
                    &msg("The fingerprint \u{201c}{name}\u{201d} is registered")
                        .with("name", e.name()),
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

    fn render_test_result(self: &Rc<Self>, o: &TestOutcome, m: &Model) {
        let Some((title, body)) = o.texts(&m.labels) else {
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

    fn render_dialog(self: &Rc<Self>, want: Option<&Dialog>, m: &Model) {
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
        let (title, body, ok, cancel) = want.texts(&m.labels);
        let d = adw::AlertDialog::new(Some(&text(&title)), Some(&text(&body)));
        // Renaming: a text field with the current name, Enter confirms.
        let entry = match want {
            Dialog::Rename(f) => {
                let e = gtk::Entry::builder()
                    .text(m.display_name(*f).text())
                    .activates_default(true)
                    .max_length(fingerprint_settings::labels::MAX_LEN as i32)
                    .build();
                set_accessible_label(&e, tr("Name"));
                d.set_extra_child(Some(&e));
                Some((*f, e))
            }
            _ => None,
        };
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
            Dialog::Rename(_) | Dialog::Error(_) => d.set_default_response(Some("ok")),
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
            let input = if let (Some((f, e)), "ok") = (&entry, response) {
                let t = e.text().trim().to_string();
                // Empty, or the standard name: back to the standard name.
                let label = (!t.is_empty() && t != tr(f.name())).then_some(t);
                Input::SetLabel(*f, label)
            } else if response == "ok" {
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
