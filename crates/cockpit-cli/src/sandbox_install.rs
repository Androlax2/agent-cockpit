//! Offline owner installation; the daemon uses this profile's same immutable engine pair.

use std::ffi::OsString;
use std::process::ExitCode;

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    if !arguments.is_empty() {
        return Err("sandbox install takes no arguments".into());
    }
    let environment = crate::profile::environment();
    let home = environment.home_dir().map_err(|error| error.to_string())?;
    let layout = cockpit_config::Layout::resolve(&environment, &home);
    match cockpit_sandbox::install::install(&layout.state_dir.value) {
        Ok(programs) => {
            outln!(
                "verified engines installed for profile {}",
                environment.profile.value
            );
            outln!("greywall: {}", programs.greywall.display());
            outln!("greyproxy: {}", programs.greyproxy.display());
            outln!(
                "restart this profile's daemon, then run `cockpit --profile {} doctor`; existing sessions keep their previous engines until restarted",
                environment.profile.value
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!("cockpit sandbox install: {error}");
            Ok(ExitCode::FAILURE)
        }
    }
}
