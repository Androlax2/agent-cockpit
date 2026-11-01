//! `cockpit`: the command-line frontend. Verbs that must work without the daemon (report 09)
//! call their capability crate directly; that list is fixed in report 02 (#77). Every other
//! verb goes to the daemon through `cockpit-client`.
#![deny(
    clippy::print_stdout,
    reason = "`print!` panics on a closed stdout (#275): write through `out!`/`outln!`"
)]

/// `print!` through [`write_stdout`]: exits instead of panicking when stdout is closed.
macro_rules! out {
    ($($argument:tt)*) => {
        $crate::write_stdout(format_args!($($argument)*))
    };
}

/// `println!` through [`write_stdout`]: exits instead of panicking when stdout is closed.
macro_rules! outln {
    ($($argument:tt)*) => {
        out!("{}\n", format_args!($($argument)*))
    };
}

mod agent;
mod approvals;
mod ask;
mod check;
mod daemon;
mod desktop_session;
mod projects;
mod resources;
mod sessions;
mod todo;
mod tui;
mod version;
mod workflow;

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: cockpit config show [--project DIR] [--set KEY=VALUE]...
       cockpit config reload
       cockpit agent report [--state working|waiting] [--label TEXT] [--doing TEXT] [--needs-input QUESTION | --dialog QUESTION] [--prompt] [--outcome completed|failed|quota-stopped] [--outcome-reason TEXT] [--conversation ID [--conversation-ends]]
       cockpit agent reports
       cockpit project add DIR [--name NAME]
       cockpit project list
       cockpit checkout list [PROJECT]
       cockpit checkout clone PROJECT BRANCH
       cockpit checkout review PROJECT PULL_REQUEST
       cockpit checkout claim CHECKOUT HOLDER
       cockpit checkout release CHECKOUT HOLDER
       cockpit checkout sweep [--dry-run]
       cockpit checkout push CHECKOUT
       cockpit ask run [--cwd DIR] --reason TEXT COMMAND | --wait ID | --check
       cockpit ask guide [--cwd DIR] < GUIDE | --wait ID
       cockpit approval list | show ID | decline ID | input ID < LINE
       cockpit approval approve ID --revision REVISION --digest DIGEST
       cockpit approval edit ID --revision REVISION COMMAND
       cockpit guide show ID | answer ID STEP [ANSWER] | stuck ID STEP NOTE | close ID
       cockpit session new DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH]
       cockpit session list [--ended]
       cockpit session restart SESSION|--all
       cockpit session sleep|wake|resume|end|show SESSION
       cockpit session end --finished
       cockpit session link SESSION URL
       cockpit provider list
       cockpit desktop-session import
       cockpit tui
       cockpit version [--all]
       cockpit todo add TITLE [--project P] [--notes TEXT] [--kind KIND] [--priority 1-6] [--after ID]...
       cockpit todo propose TITLE --why TEXT [--project P] [--notes TEXT] [--kind KIND]
       cockpit todo blocked TITLE --why TEXT [--notes TEXT]
       cockpit todo list [--project P] [--all] | show ID | delete ID
       cockpit todo start ID | done ID [--summary TEXT] | reopen ID
       cockpit todo edit ID [--revision R] [--title T] [--notes TEXT] [--kind KIND]
       cockpit todo priority ID LEVEL | queue ID | unqueue ID | approve ID | decline ID
       cockpit todo after ID ON... | unwait ID ON...
       cockpit todo image add ID FILE | image get ID NUMBER FILE
       cockpit todo draft save (ID | --new PROJECT) [--title T] [--notes TEXT] [--revision R]
       cockpit todo drafts | draft discard (ID | --new PROJECT)
       cockpit workflow presets | start ITEM [--preset NAME] | cancel ID
       cockpit workflow list [--project P] [--all] | show ID | packet ID
       cockpit workflow begin ID | checkpoint ID NOTE | unblock ID
       cockpit workflow complete ID --changed T --checked T --outcome T [--needs-human T] [--evidence REF]... [--waive REQUIREMENT]...
       cockpit workflow release ID [--changed T --checked T --outcome T [--needs-human T] [--evidence REF]...]
       cockpit workflow block ID ACTION --detail TEXT
       cockpit check run CHECK --project P [--checkout PATH] | gate [--project P] [--checkout PATH]
       cockpit check claim CHECK STATEMENT
       cockpit resources status | profile normal|foreground | pause-queue | resume-queue
       cockpit resources check WORKLOAD --cpu MILLICORES --memory MIB [--require CONTROL]...
       cockpit resources checkpoint RESERVATION | stop RESERVATION";

fn main() -> ExitCode {
    // OsString: a project path need not be UTF-8.
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let verb: Vec<Option<&str>> = arguments.iter().take(2).map(|word| word.to_str()).collect();
    match verb[..] {
        [Some("config"), Some("show")] => config_show(&arguments[2..]),
        [Some("config"), Some("reload")] => config_reload(&arguments[2..]),
        [Some("agent"), Some("report")] => {
            agent::report(&arguments[2..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        [Some("agent"), Some("reports")] => {
            agent::reports(&arguments[2..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        [Some("ask"), Some("run")] => {
            ask::run(&arguments[2..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        [Some("ask"), Some("guide")] => {
            ask::guide(&arguments[2..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        [Some(verb @ ("approval" | "guide")), ..] => {
            match approvals::parse(verb, &arguments[1..]) {
                Ok(command) => approvals::run(command),
                Err(problem) => usage_error(Some(problem)),
            }
        }
        [Some(verb @ ("project" | "checkout")), ..] => {
            match projects::parse(verb, &arguments[1..]) {
                Ok((command, deadline)) => projects::run(command, deadline),
                Err(problem) => usage_error(Some(problem)),
            }
        }
        [Some("session"), ..] => match sessions::parse(&arguments[1..]) {
            Ok(command) => sessions::run(command),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("version"), ..] => {
            version::run(&arguments[1..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        [Some("todo"), ..] => match todo::parse(&arguments[1..]) {
            Ok(request) => todo::run(request),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("workflow"), ..] => match workflow::parse(&arguments[1..]) {
            Ok(request) => workflow::run(request),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("check"), ..] => match check::parse(&arguments[1..]) {
            Ok(request) => check::run(request),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("resources"), ..] => match resources::parse(&arguments[1..]) {
            Ok(command) => resources::run(command),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("provider"), ..] => match sessions::parse_provider(&arguments[1..]) {
            Ok(command) => sessions::run(command),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("desktop-session"), ..] => match desktop_session::parse(&arguments[1..]) {
            Ok(()) => desktop_session::import(),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("tui"), ..] => {
            tui::run(&arguments[1..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        _ => usage_error(None),
    }
}

/// Writes and flushes `text` so a closed reader (`cockpit ... | head`) surfaces here, at the
/// write. Rust ignores SIGPIPE, so `print!` would panic; this exits 141 instead, the status a
/// shell reports for a writer SIGPIPE ended (#275). Any other write failure exits 1 naming it.
fn write_stdout(text: std::fmt::Arguments) {
    let written = {
        let mut stdout = std::io::stdout().lock();
        stdout.write_fmt(text).and_then(|()| stdout.flush())
    };
    let Err(error) = written else {
        return;
    };
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        std::process::exit(141);
    }
    // `eprintln!` would panic if stderr is broken too; exit 1 still reports the failure.
    let _ = writeln!(
        std::io::stderr(),
        "cockpit: cannot write to stdout: {error}"
    );
    std::process::exit(1);
}

/// Exit 2 with the usage line, after naming the rejected argument when there is one.
fn usage_error(problem: Option<String>) -> ExitCode {
    if let Some(problem) = problem {
        eprintln!("cockpit: {problem}");
    }
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

/// Has the running daemon re-read the owner's file and apply its session settings (#354);
/// prints each one it applies, and the value it replaced when that differs.
fn config_reload(arguments: &[OsString]) -> ExitCode {
    if let Some(argument) = arguments.first() {
        return usage_error(Some(format!(
            "unknown argument {}",
            argument.to_string_lossy()
        )));
    }
    match daemon::request(
        cockpit_protocol::Command::ConfigReload,
        daemon::CONTROL_DEADLINE,
    ) {
        Ok(cockpit_protocol::Reply::ConfigReloaded { settings }) => {
            for setting in settings {
                if setting.value == setting.before {
                    outln!("{} = {}", setting.key, setting.value);
                } else {
                    outln!(
                        "{} = {}  # was {}",
                        setting.key,
                        setting.value,
                        setting.before
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Ok(other) => daemon::unexpected(&other),
        Err(code) => code,
    }
}

/// Prints every effective configuration value with its source; reads files only.
fn config_show(flags: &[OsString]) -> ExitCode {
    let mut flags = flags.iter();
    let mut project_dir: Option<PathBuf> = None;
    let mut overrides = Vec::new();
    while let Some(flag) = flags.next() {
        let flag_name = flag.to_string_lossy();
        let is_project = match &*flag_name {
            "--project" => true,
            "--set" => false,
            _ => return usage_error(Some(format!("unknown argument {flag_name}"))),
        };
        let Some(value) = flags
            .next()
            .filter(|value| !value.to_string_lossy().starts_with("--"))
        else {
            return usage_error(Some(format!("{flag_name} needs a value")));
        };
        if is_project {
            if project_dir.is_some() {
                return usage_error(Some("--project given twice".to_string()));
            }
            project_dir = Some(PathBuf::from(value));
        } else {
            let Some(value) = value.to_str() else {
                return usage_error(Some(format!(
                    "--set {}: not UTF-8",
                    value.to_string_lossy()
                )));
            };
            overrides.push(value.to_string());
        }
    }

    match cockpit_config::load(
        &cockpit_config::Environment::from_process(),
        project_dir.as_deref(),
        &overrides,
    ) {
        Ok(config) => {
            out!("{}", cockpit_config::render(&config));
            // The daemon read the file at its start (#354); stdout stays the values alone.
            eprintln!(
                "cockpit: a running daemon applies a changed agents.desktop_session, \
                 sessions.idle_sleep_after_minutes or sessions.asleep_expiry_days after \
                 `cockpit config reload`, and start files from its next start"
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cockpit: {error}");
            ExitCode::FAILURE
        }
    }
}
