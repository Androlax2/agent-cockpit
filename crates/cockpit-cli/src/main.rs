//! `cockpit`: the command-line frontend. Verbs that must work without the daemon (report 09)
//! call their capability crate directly; that list is fixed in report 02 (#77). Every other
//! verb goes to the daemon through `cockpit-client`.

mod agent;
mod approvals;
mod ask;
mod projects;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: cockpit config show [--project DIR] [--set KEY=VALUE]...
       cockpit agent report [--state working|waiting] [--label TEXT] [--doing TEXT] [--needs-input QUESTION]
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
       cockpit approval list | show ID | decline ID | input ID TEXT
       cockpit approval approve ID --revision REVISION --digest DIGEST
       cockpit approval edit ID --revision REVISION COMMAND
       cockpit guide show ID | answer ID STEP [ANSWER] | stuck ID STEP NOTE | close ID";

fn main() -> ExitCode {
    // OsString: a project path need not be UTF-8.
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let verb: Vec<Option<&str>> = arguments.iter().take(2).map(|word| word.to_str()).collect();
    match verb[..] {
        [Some("config"), Some("show")] => config_show(&arguments[2..]),
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
        _ => usage_error(None),
    }
}

/// Exit 2 with the usage line, after naming the rejected argument when there is one.
fn usage_error(problem: Option<String>) -> ExitCode {
    if let Some(problem) = problem {
        eprintln!("cockpit: {problem}");
    }
    eprintln!("{USAGE}");
    ExitCode::from(2)
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
            print!("{}", cockpit_config::render(&config));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cockpit: {error}");
            ExitCode::FAILURE
        }
    }
}
