//! `cockpit`: the command-line frontend. Verbs that must work without the daemon (report 09)
//! call their capability crate directly; that list is fixed in report 02 (#77). Every other
//! verb goes to the daemon through `cockpit-client`. `--profile NAME` before the verb selects
//! the profile (#196, [`profile`]).
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
mod bug_report;
mod check;
mod coordination;
mod daemon;
mod daemon_install;
mod desktop_session;
mod doctor;
mod installations;
mod instructions;
mod mcp;
mod onboarding;
mod profile;
mod projects;
mod queue;
mod resources;
mod sandbox_install;
mod sessions;
mod todo;
mod tui;
mod version;
mod workflow;

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: torlek [--profile NAME] VERB ...
       torlek config show [--project DIR] [--set KEY=VALUE]...
       torlek config reload
       torlek agent report [--state working|waiting] [--label TEXT] [--doing TEXT] [--needs-input QUESTION | --dialog QUESTION] [--prompt] [--outcome completed|failed|quota-stopped] [--outcome-reason TEXT] [--conversation ID [--conversation-ends]]
       torlek agent reports
       torlek agent sessions [--watch]
       torlek agent message SESSION TEXT
       torlek agent start DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH]
       torlek agent report-bug DESCRIPTION [--kind timeout|unreachable|internal|other] [--component NAME] [--steps TEXT] [--requested-by-owner]
       torlek bug-report new DESCRIPTION [--title TEXT] [--steps TEXT]
       torlek bug-report list [--all] | show ID | discard ID
       torlek bug-report edit ID [--title TEXT] [--description TEXT] [--steps TEXT]
       torlek bug-report send ID [--gh | --print] [--anyway] | send ID --filed URL
       torlek project add DIR [--name NAME]
       torlek project list
       torlek checkout list [PROJECT]
       torlek checkout clone PROJECT BRANCH
       torlek checkout review PROJECT PULL_REQUEST
       torlek checkout claim CHECKOUT HOLDER
       torlek checkout release CHECKOUT HOLDER
       torlek checkout sweep [--dry-run]
       torlek checkout push CHECKOUT
       torlek ask run [--cwd DIR] --reason TEXT COMMAND | --wait ID | --check
       torlek ask guide [--cwd DIR] < GUIDE | --wait ID
       torlek approval list | show ID | decline ID | input ID < LINE
       torlek approval approve ID --revision REVISION --digest DIGEST
       torlek approval edit ID --revision REVISION COMMAND
       torlek guide show ID | answer ID STEP [ANSWER] | stuck ID STEP NOTE | close ID
       torlek session send ID MESSAGE | cancel ID
       torlek session new DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH]
       torlek session list [--ended]
       torlek session restart SESSION|--all
       torlek session sleep|wake|resume|end|show SESSION
       torlek session end --finished
       torlek session link SESSION URL
       torlek session grant SESSION observe-sessions|message-sessions|start-sessions [--per-hour N [--without-confirmation]]
       torlek session revoke SESSION observe-sessions|message-sessions|start-sessions
       torlek session grants SESSION
       torlek session sandbox SESSION
       torlek session sandbox-record SESSION --client CLIENT --observed TEXT...
       torlek sandbox install [--engines-only]
       torlek sandbox status|restart|stop
       torlek sandbox show DIR [--agent NAME]
       torlek provider list
       torlek desktop-session import
       torlek init
       torlek doctor [--project DIR]
       torlek tui [--version]
       torlek version [--all]
       torlek daemon install
       torlek todo add TITLE [--project P] [--notes TEXT] [--kind KIND] [--priority 1-6] [--after ID]...
       torlek todo propose TITLE --why TEXT [--project P] [--notes TEXT] [--kind KIND]
       torlek todo blocked TITLE --why TEXT [--notes TEXT]
       torlek todo list [--project P] [--all] | show ID | delete ID
       torlek todo start ID | done ID [--summary TEXT] | reopen ID
       torlek todo edit ID [--revision R] [--title T] [--notes TEXT] [--kind KIND]
       torlek todo priority ID LEVEL | queue ID | unqueue ID | approve ID | decline ID
       torlek todo after ID ON... | unwait ID ON...
       torlek todo image add ID FILE | image get ID NUMBER FILE
       torlek todo draft save (ID | --new PROJECT) [--title T] [--notes TEXT] [--revision R]
       torlek todo drafts | draft discard (ID | --new PROJECT)
       torlek workflow presets | start ITEM [--preset NAME] | cancel ID
       torlek workflow list [--project P] [--all] | show ID | packet ID
       torlek workflow begin ID | checkpoint ID NOTE | unblock ID
       torlek workflow complete ID --changed T --checked T --outcome T [--needs-human T] [--evidence REF]... [--waive REQUIREMENT]...
       torlek workflow release ID [--changed T --checked T --outcome T [--needs-human T] [--evidence REF]...]
       torlek workflow block ID ACTION --detail TEXT
       torlek check run CHECK --project P [--checkout PATH] | gate [--project P] [--checkout PATH]
       torlek check claim CHECK STATEMENT
       torlek resources status | profile normal|foreground | pause-queue | resume-queue
       torlek resources check WORKLOAD --cpu MILLICORES --memory MIB [--require CONTROL]...
       torlek resources checkpoint RESERVATION | stop RESERVATION
       torlek queue explain [ITEM] | dispatch
       torlek --skill
       torlek docs [TOPIC]
       torlek brief [--measure]
       torlek mcp";

fn main() -> ExitCode {
    let mut all_arguments = std::env::args_os();
    let command_name = all_arguments.next();
    // OsString: a project path need not be UTF-8.
    let mut arguments: Vec<OsString> = all_arguments.collect();
    let flag = match arguments.first().and_then(|word| word.to_str()) {
        Some("--profile") => match arguments.get(1).map(|name| name.to_str()) {
            Some(Some(name)) => Some(name.to_owned()),
            Some(None) => return usage_error(Some("--profile: not UTF-8".to_owned())),
            None => return usage_error(Some("--profile needs a name".to_owned())),
        },
        _ => None,
    };
    if flag.is_some() {
        arguments.drain(..2);
    }
    if let Err(problem) = profile::select(flag.as_deref(), command_name.as_deref()) {
        eprintln!("torlek: {problem}");
        return ExitCode::from(2);
    }
    let verb: Vec<Option<&str>> = arguments.iter().take(2).map(|word| word.to_str()).collect();
    match verb[..] {
        [] => onboarding::first_run(),
        [Some("init"), ..] => {
            onboarding::init(&arguments[1..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        [Some("--skill"), ..] => instructions::skill(&arguments[1..])
            .unwrap_or_else(|problem| usage_error(Some(problem))),
        [Some("docs"), ..] => {
            instructions::docs(&arguments[1..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        [Some("brief"), ..] => instructions::brief(&arguments[1..])
            .unwrap_or_else(|problem| usage_error(Some(problem))),
        [Some("mcp"), ..] => {
            mcp::run(&arguments[1..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        [Some("config"), Some("show")] => config_show(&arguments[2..]),
        [Some("config"), Some("reload")] => config_reload(&arguments[2..]),
        [Some("agent"), Some("report")] => {
            agent::report(&arguments[2..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        [Some("agent"), Some("reports")] => {
            agent::reports(&arguments[2..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
        [Some("agent"), Some("sessions")] => coordination::sessions(&arguments[2..])
            .unwrap_or_else(|problem| usage_error(Some(problem))),
        [Some("agent"), Some("message")] => coordination::message(&arguments[2..])
            .unwrap_or_else(|problem| usage_error(Some(problem))),
        [Some("agent"), Some("start")] => coordination::start(&arguments[2..])
            .unwrap_or_else(|problem| usage_error(Some(problem))),
        [Some("agent"), Some("report-bug")] => bug_report::agent_report(&arguments[2..])
            .unwrap_or_else(|problem| usage_error(Some(problem))),
        [Some("bug-report"), ..] => match bug_report::parse(&arguments[1..]) {
            Ok(verb) => bug_report::run(verb),
            Err(problem) => usage_error(Some(problem)),
        },
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
        [Some("daemon"), Some("install")] => daemon_install::run(&arguments[2..])
            .unwrap_or_else(|problem| usage_error(Some(problem))),
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
        [Some("queue"), ..] => match queue::parse(&arguments[1..]) {
            Ok(command) => queue::run(command),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("provider"), ..] => match sessions::parse_provider(&arguments[1..]) {
            Ok(command) => sessions::run(command),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("sandbox"), Some("install")] => sandbox_install::run(&arguments[2..])
            .unwrap_or_else(|problem| usage_error(Some(problem))),
        [
            Some("sandbox"),
            Some(action @ ("status" | "restart" | "stop")),
        ] => sandbox_install::proxy(action, &arguments[2..])
            .unwrap_or_else(|problem| usage_error(Some(problem))),
        [Some("sandbox"), ..] => match sessions::parse_sandbox(&arguments[1..]) {
            Ok(command) => sessions::run(command),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("desktop-session"), ..] => match desktop_session::parse(&arguments[1..]) {
            Ok(()) => desktop_session::import(),
            Err(problem) => usage_error(Some(problem)),
        },
        [Some("doctor"), ..] => {
            doctor::run(&arguments[1..]).unwrap_or_else(|problem| usage_error(Some(problem)))
        }
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
    let _ = writeln!(std::io::stderr(), "torlek: cannot write to stdout: {error}");
    std::process::exit(1);
}

/// Exit 2 with the usage line, after naming the rejected argument when there is one.
fn usage_error(problem: Option<String>) -> ExitCode {
    if let Some(problem) = problem {
        eprintln!("torlek: {problem}");
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

    match cockpit_config::load(&profile::environment(), project_dir.as_deref(), &overrides) {
        Ok(config) => {
            out!("{}", cockpit_config::render(&config));
            // The daemon read the file at its start (#354); stdout stays the values alone.
            eprintln!(
                "torlek: a running daemon applies a changed agents.desktop_session, \
                 sessions.idle_sleep_after_minutes or sessions.asleep_expiry_days after \
                 `torlek config reload`, and start files from its next start"
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("torlek: {error}");
            ExitCode::FAILURE
        }
    }
}
