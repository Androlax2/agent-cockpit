//! `cockpit version [--all]`: this binary's version and source revision; `--all` adds the
//! release bundle's manifest installed beside it (report 13, #96). The running daemon, each
//! session's bundle and the schema compatibility join `--all` with #64.

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::ExitCode;

/// Where a bundle puts its manifest under the prefix holding `bin/cockpit`: the same for the
/// unpacked archive and an install under `/usr` or `/opt/cockpit` (`tools/release.py`).
const MANIFEST_IN_PREFIX: &str = "share/cockpit/manifest.json";

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let shows_all = match arguments {
        [] => false,
        [flag] if flag == "--all" => true,
        _ => return Err("version takes no argument but --all".to_string()),
    };
    println!("cockpit {} ({})", env!("CARGO_PKG_VERSION"), revision());
    if !shows_all {
        return Ok(ExitCode::SUCCESS);
    }
    let manifest_path = match installed_manifest_path() {
        Ok(path) => path,
        Err(problem) => {
            eprintln!("cockpit: cannot locate this binary's bundle: {problem}");
            return Ok(ExitCode::FAILURE);
        }
    };
    match fs::read_to_string(&manifest_path) {
        Ok(manifest) => {
            println!("bundle manifest {}:", manifest_path.display());
            print!("{manifest}");
        }
        Err(error) if error.kind() == ErrorKind::NotFound => println!(
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

/// Resolves symlinks first, so `/usr/local/bin/cockpit -> /opt/cockpit/bin/cockpit` finds
/// `/opt/cockpit/share/`.
fn installed_manifest_path() -> std::io::Result<PathBuf> {
    let binary = std::env::current_exe()?.canonicalize()?;
    let prefix = binary
        .parent()
        .and_then(|binary_dir| binary_dir.parent())
        .ok_or_else(|| std::io::Error::other(format!("{} has no prefix", binary.display())))?;
    Ok(prefix.join(MANIFEST_IN_PREFIX))
}
