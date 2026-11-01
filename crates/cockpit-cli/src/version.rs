//! `cockpit version [--all]`: this binary's version and source revision; `--all` adds the
//! active profile and build (#196) and the release bundle's manifest installed beside it
//! (report 13, #96). The running daemon, each session's bundle and the schema compatibility
//! join `--all` with #64.

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::process::ExitCode;

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let shows_all = match arguments {
        [] => false,
        [flag] if flag == "--all" => true,
        _ => return Err("version takes no argument but --all".to_string()),
    };
    outln!("cockpit {} ({})", env!("CARGO_PKG_VERSION"), revision());
    if !shows_all {
        return Ok(ExitCode::SUCCESS);
    }
    let profile = crate::profile::active();
    outln!("profile {} ({})", profile.value, profile.source);
    outln!("build {}", crate::profile::build().as_str());
    let manifest_path = match crate::profile::this_program().and_then(|program| {
        cockpit_config::release_manifest(&program).map_err(|error| error.to_string())
    }) {
        Ok(path) => path,
        Err(problem) => {
            eprintln!("cockpit: cannot locate this binary's bundle: {problem}");
            return Ok(ExitCode::FAILURE);
        }
    };
    match fs::read_to_string(&manifest_path) {
        Ok(manifest) => {
            outln!("bundle manifest {}:", manifest_path.display());
            out!("{manifest}");
        }
        Err(error) if error.kind() == ErrorKind::NotFound => outln!(
            "bundle manifest: none at {} (a build outside a release bundle)",
            manifest_path.display()
        ),
        Err(error) => {
            eprintln!("cockpit: {}: {error}", manifest_path.display());
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The release workflow sets `COCKPIT_SOURCE_REVISION` while compiling; rustc tracks it, so a
/// changed revision rebuilds.
fn revision() -> String {
    match option_env!("COCKPIT_SOURCE_REVISION") {
        Some(revision) => format!("revision {revision}"),
        None => "revision unknown: not a release build".to_string(),
    }
}
