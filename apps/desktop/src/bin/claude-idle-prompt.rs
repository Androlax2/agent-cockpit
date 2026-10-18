//! The "still there?" screen of the Claude workspace: claude-idle-prompt <seconds>.
//!
//! A layer-shell window over the whole screen (Hyprland blurs what is behind it, apps/claude.lua)
//! with a card in the middle: the question, a countdown, and what to do. Any key, click or mouse
//! movement answers it and the process exits 0; when the countdown runs out it exits 1, which
//! claude-idle.sh turns into the lock. The window takes the keyboard so the key goes nowhere else.

use gtk4::prelude::*;
use gtk4::{Align, Application, Box as GtkBox, CssProvider, EventControllerKey, EventControllerMotion, GestureClick, Justification, Label,
           Orientation, Window, gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const ANSWERED: u8 = 0;
const TIMED_OUT: u8 = 1;
const SIGTERM: i32 = 15;

fn activate(app: &Application, seconds: u32, outcome: Rc<Cell<u8>>) {
    let provider = CssProvider::new();
    provider.load_from_path(cockpit::paths::home().join(".config/hypr/scripts/claude-idle.css"));
    gtk4::style_context_add_provider_for_display(&gdk::Display::default().expect("a display"), &provider, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let window = Window::builder().application(app).title("Still there?").build();
    window.add_css_class("claude-idle");
    window.init_layer_shell();
    window.set_namespace(Some("claude-idle"));
    window.set_layer(Layer::Overlay);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.set_keyboard_mode(KeyboardMode::Exclusive);

    let countdown = Label::new(Some(&seconds.to_string()));
    countdown.add_css_class("countdown");
    let title = Label::new(Some("Still there?"));
    title.add_css_class("title");
    let hint = Label::new(Some("The screen locks when the count reaches zero.\nAny key, click or move keeps the session on screen."));
    hint.add_css_class("hint");
    hint.set_justify(Justification::Center);
    let card = GtkBox::builder().orientation(Orientation::Vertical).spacing(18).halign(Align::Center).valign(Align::Center).build();
    card.add_css_class("card");
    for widget in [&countdown, &title, &hint] {
        card.append(widget);
    }
    window.set_child(Some(&card));

    let finish = {
        let app = app.clone();
        move |code: u8| {
            outcome.set(code);
            app.quit();
        }
    };
    let keys = EventControllerKey::new();
    let on_key = finish.clone();
    keys.connect_key_pressed(move |_, _, _, _| {
        on_key(ANSWERED);
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    let click = GestureClick::new();
    let on_click = finish.clone();
    click.connect_pressed(move |_, _, _, _| on_click(ANSWERED));
    window.add_controller(click);
    let motion = EventControllerMotion::new();
    let on_motion = finish.clone();
    motion.connect_motion(move |_, _, _| on_motion(ANSWERED));
    window.add_controller(motion);

    let remaining = Cell::new(seconds);
    glib::timeout_add_local(Duration::from_secs(1), move || {
        remaining.set(remaining.get().saturating_sub(1));
        if remaining.get() == 0 {
            finish(TIMED_OUT);
            return glib::ControlFlow::Break;
        }
        countdown.set_label(&remaining.get().to_string());
        glib::ControlFlow::Continue
    });
    window.present();
}

fn main() -> glib::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(seconds) = arguments.first().filter(|_| arguments.len() == 1).and_then(|text| text.parse::<u32>().ok()) else {
        eprintln!("usage: claude-idle-prompt <seconds>");
        return glib::ExitCode::from(2);
    };
    // Until answered, the countdown is what ends it.
    let outcome = Rc::new(Cell::new(TIMED_OUT));
    let app = Application::builder().application_id("dotfiles.claude-idle-prompt").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    let activated_outcome = outcome.clone();
    app.connect_activate(move |app| activate(app, seconds, activated_outcome.clone()));
    // claude-idle.sh cancel: activity resumed, which is an answer.
    let (quitting, terminated_outcome) = (app.clone(), outcome.clone());
    glib_unix::unix_signal_add_local_once(SIGTERM, move || {
        terminated_outcome.set(ANSWERED);
        quitting.quit();
    });
    app.run_with_args::<&str>(&[]);
    glib::ExitCode::from(outcome.get())
}
