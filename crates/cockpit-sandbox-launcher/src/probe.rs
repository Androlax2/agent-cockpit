//! The probe the daemon runs at its start: can this host start a sandbox at all, and which
//! Landlock ABI does a sandboxed process get? Prints one JSON line the daemon reads:
//! `{"bubblewrap":"0.13.1","landlock_abi":9}`. A failure is the reason required launches
//! are refused.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::stage2::PROBE_VERB;

#[derive(Debug, Serialize)]
struct Report {
    bubblewrap: String,
    landlock_abi: u8,
}

pub fn run(bubblewrap: PathBuf) -> Result<(), String> {
    let version = bubblewrap_version(&bubblewrap)?;
    let launcher = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("cannot find the launcher's own program: {error}"))?;
    let output = Command::new(&bubblewrap)
        .args(["--unshare-all", "--die-with-parent", "--ro-bind", "/", "/"])
        .args(["--proc", "/proc", "--dev", "/dev", "--"])
        .arg(&launcher)
        .arg(PROBE_VERB)
        .output()
        .map_err(|error| {
            format!(
                "cannot start bubblewrap at {}: {error}",
                bubblewrap.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "the probe launch failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let landlock_abi = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| {
            format!(
                "the probe launch printed no Landlock ABI: {:?}",
                String::from_utf8_lossy(&output.stdout)
            )
        })?;
    let report = Report {
        bubblewrap: version,
        landlock_abi,
    };
    let line = serde_json::to_string(&report)
        .map_err(|error| format!("cannot write the probe report: {error}"))?;
    println!("{line}");
    Ok(())
}

/// `bubblewrap 0.13.1` gives `0.13.1`.
fn bubblewrap_version(bubblewrap: &Path) -> Result<String, String> {
    let output = Command::new(bubblewrap)
        .arg("--version")
        .output()
        .map_err(|error| {
            format!(
                "cannot start bubblewrap at {}: {error}",
                bubblewrap.display()
            )
        })?;
    let text = String::from_utf8_lossy(&output.stdout);
    match text.trim().split_once(' ') {
        Some(("bubblewrap", version)) if output.status.success() => Ok(version.to_owned()),
        _ => Err(format!(
            "{} --version printed {:?}, not bubblewrap's version",
            bubblewrap.display(),
            text.trim()
        )),
    }
}
