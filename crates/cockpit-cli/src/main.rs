//! `cockpit`: the command-line frontend. Verbs that must work without the daemon (report 09)
//! call their capability crate directly; that list is fixed in report 02 (#77).

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: cockpit config show [--project DIR] [--set KEY=VALUE]...";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["config", "show", ..] => config_show(&arguments[2..]),
        _ => usage_error(),
    }
}

fn usage_error() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

/// Prints every effective configuration value with its source; reads files only.
fn config_show(flags: &[String]) -> ExitCode {
    let mut flags = flags.iter();
    let mut project_dir: Option<PathBuf> = None;
    let mut overrides = Vec::new();
    while let Some(flag) = flags.next() {
        let (Some(value), "--project" | "--set") = (flags.next(), flag.as_str()) else {
            return usage_error();
        };
        if flag == "--project" {
            if project_dir.is_some() {
                eprintln!("cockpit: --project given twice\n{USAGE}");
                return ExitCode::from(2);
            }
            project_dir = Some(PathBuf::from(value));
        } else {
            overrides.push(value.clone());
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
