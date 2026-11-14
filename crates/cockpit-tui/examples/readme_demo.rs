//! Exports the current TUI renderer with fictional, checked-in fixture data.
//! No daemon connection, provider process, user home or host command is used.

#[path = "../tests/common/mod.rs"]
mod common;

use std::error::Error;
use std::fmt::Write;
use std::time::{Duration, Instant};

use cockpit_protocol::{ChangeBatch, Command, Reply};
use cockpit_tui::{App, INPUT_GRACE, draw};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let output = args.next().ok_or("usage: readme_demo OUTPUT.svg")?;
    if args.next().is_some() {
        return Err("usage: readme_demo OUTPUT.svg".into());
    }
    let now = Instant::now();
    let mut app = App::for_profile("demo".to_owned());
    common::answer(
        &mut app,
        Command::ProjectList,
        Reply::Projects {
            projects: vec![common::project("web"), common::project("api")],
        },
        now,
    );
    for line in include_str!("../tests/fixtures/two-projects.feed").lines() {
        let batch: ChangeBatch = serde_json::from_str(line)?;
        app.apply_changes(&batch)?;
    }
    let mut request = common::run(1, "s00000000000b", "cargo test --locked");
    request.project = Some("api".to_owned());
    request.cwd = "/p/api".to_owned();
    request.shown_cwd = common::shown("/p/api");
    let focus = common::queue(&mut app, vec![request], Vec::new(), now)
        .ok_or("fixture did not request column focus")?;
    common::answer(
        &mut app,
        focus,
        Reply::ManagerFocused {
            took_from_viewer: false,
        },
        now,
    );

    const WIDTH: u16 = 80;
    const HEIGHT: u16 = 24;
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT))?;
    terminal.draw(|frame| draw(&app, frame, now + INPUT_GRACE + Duration::from_millis(1)))?;
    let buffer = terminal.backend().buffer();
    let text = (0..HEIGHT)
        .map(|row| {
            (0..WIDTH)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "profile demo",
        "api",
        "web",
        "Run request 1",
        "cargo test --locked",
        "y run",
    ] {
        assert!(
            text.contains(expected),
            "fixture did not render {expected:?}: {text}"
        );
    }

    let pixel_width = WIDTH * 12 + 40;
    let pixel_height = HEIGHT * 20 + 40;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pixel_width}\" height=\"{pixel_height}\" viewBox=\"0 0 {pixel_width} {pixel_height}\" role=\"img\" aria-labelledby=\"title description\">\n\
<title id=\"title\">Alpha TUI demo fixture</title>\n\
<desc id=\"description\">The current renderer with fictional api and web sessions and an approval request, in a neutral monochrome terminal.</desc>\n\
<rect width=\"{pixel_width}\" height=\"{pixel_height}\" fill=\"#000\"/>\n\
<g font-family=\"monospace\" font-size=\"20\">\n",
    );
    for row in 0..HEIGHT {
        for column in 0..WIDTH {
            let cell = &buffer[(column, row)];
            // shortcut: the fixture uses reset colors, bold and reverse; extend when the renderer adds styles.
            if cell.fg != Color::Reset
                || cell.bg != Color::Reset
                || !(cell.modifier & !(Modifier::BOLD | Modifier::REVERSED)).is_empty()
            {
                return Err("fixture has a style the screenshot exporter does not support".into());
            }
            let x = 20 + column * 12;
            let y = 20 + row * 20;
            let reverse = cell.modifier.contains(Modifier::REVERSED);
            if reverse {
                writeln!(
                    svg,
                    "<rect x=\"{x}\" y=\"{y}\" width=\"12\" height=\"20\" fill=\"#fff\"/>"
                )?;
            }
            if cell.symbol().trim().is_empty() {
                continue;
            }
            let symbol = cell
                .symbol()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            let color = if reverse { "#000" } else { "#fff" };
            let weight = if cell.modifier.contains(Modifier::BOLD) {
                "bold"
            } else {
                "normal"
            };
            writeln!(
                svg,
                "<text x=\"{x}\" y=\"{}\" fill=\"{color}\" font-weight=\"{weight}\">{symbol}</text>",
                y + 16
            )?;
        }
    }
    svg.push_str("</g>\n</svg>\n");
    std::fs::write(output, svg)?;
    Ok(())
}
