//! Offline owner setup of the pinned engines and profile-owned Greyproxy service.

use std::ffi::OsString;
use std::process::ExitCode;

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let engines_only = match arguments {
        [] => false,
        [flag] if flag == "--engines-only" => true,
        _ => return Err("sandbox install accepts only --engines-only".into()),
    };
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
            if !engines_only {
                let result = cockpit_sandbox::ManagedProxy::new(
                    &environment,
                    &cockpit_sandbox::Platform::detect(),
                )
                .and_then(|proxy| proxy.install());
                if let Err(error) = result {
                    return Ok(proxy_failure(&error));
                }
                outln!("this profile's managed Greyproxy service is healthy");
            }
            outln!(
                "restart this profile's daemon, then run `torlek --profile {} doctor`; existing sessions keep their previous engines until restarted",
                environment.profile.value
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!("torlek sandbox install: {error}");
            Ok(ExitCode::FAILURE)
        }
    }
}

pub(crate) fn proxy(action: &str, arguments: &[OsString]) -> Result<ExitCode, String> {
    if !arguments.is_empty() {
        return Err(format!("sandbox {action} takes no arguments"));
    }
    let environment = crate::profile::environment();
    let result = cockpit_sandbox::ManagedProxy::new(&environment, &cockpit_sandbox::Platform::detect())
        .and_then(|proxy| match action {
            "status" => proxy.health(),
            "restart" => proxy.restart().map(|()| "this profile's managed Greyproxy service was restarted and is healthy".into()),
            "stop" => proxy.stop().map(|()| "this profile's managed Greyproxy service is stopped; required-sandbox launches will be refused".into()),
            _ => unreachable!("only the three offline commands are routed here"),
        });
    match result {
        Ok(message) => {
            outln!("{message}");
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => Ok(proxy_failure(&error)),
    }
}

fn proxy_failure(error: &cockpit_sandbox::ProxyError) -> ExitCode {
    eprintln!(
        "torlek sandbox: {error}\nnext: {}",
        error.next(&crate::profile::active().value)
    );
    ExitCode::FAILURE
}
