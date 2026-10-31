//! `cockpit-config show [--project DIR] [--set KEY=VALUE]...`: config inspection that works
//! without the daemon (report 09). Becomes `cockpit config show` once a CLI crate exists.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: cockpit-config show [--project DIR] [--set KEY=VALUE]...";

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() != Some("show") {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let mut project_dir: Option<PathBuf> = None;
    let mut overrides = Vec::new();
    while let Some(flag) = arguments.next() {
        let (Some(value), "--project" | "--set") = (arguments.next(), flag.as_str()) else {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        };
        if flag == "--project" {
            if project_dir.is_some() {
                eprintln!("cockpit-config: --project given twice\n{USAGE}");
                return ExitCode::from(2);
            }
            project_dir = Some(PathBuf::from(value));
        } else {
            overrides.push(value);
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
            eprintln!("cockpit-config: {error}");
            ExitCode::FAILURE
        }
    }
}
