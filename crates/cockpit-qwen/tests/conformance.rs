//! Conformance against an installed Qwen Code. Ignored by default: CI has no Qwen Code, and no
//! test may run the owner's. The owner runs it on a version before it is
//! added to `tested-versions.txt` (#1446); none needs an account:
//!
//! ```text
//! COCKPIT_QWEN_PROGRAM="$(command -v qwen)" \
//!   cargo nextest run --locked -p cockpit-qwen --test conformance --run-ignored only
//! ```

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use cockpit_provider_api::Provider;
use cockpit_qwen::QwenCode;

#[test]
#[ignore = "needs an installed Qwen Code (COCKPIT_QWEN_PROGRAM)"]
fn the_installed_qwen_prints_the_version_the_fixtures_come_from() {
    let qwen = QwenCode::discover(&common::installed(), None).unwrap();

    assert_eq!(qwen.version(), Some(cockpit_qwen::CONFORMANCE_VERSION));
}

#[test]
#[ignore = "needs an installed Qwen Code (COCKPIT_QWEN_PROGRAM)"]
fn the_installed_qwen_probes_clean() {
    let program = common::installed();
    let qwen = QwenCode::discover(&program, None).unwrap();

    let probe = cockpit_qwen::probe(&program, qwen.version().unwrap()).unwrap();

    assert!(probe.is_clean(), "{}", probe.summary());
}

#[test]
#[ignore = "needs an installed Qwen Code (COCKPIT_QWEN_PROGRAM)"]
fn the_installed_qwen_documents_every_flag_the_adapter_passes() {
    let output = Command::new(common::installed())
        .arg("--help")
        .output()
        .unwrap();
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    for flag in cockpit_qwen::LAUNCH_FLAGS {
        assert!(
            cockpit_provider_api::documents_flag(&help, flag),
            "{flag} is not in --help"
        );
    }
}

/// A package that no longer reads the variable would load no hooks: no events, no outcomes,
/// no brief. The bundle is split into chunks, so every script file of the package is read.
#[test]
#[ignore = "needs an installed Qwen Code (COCKPIT_QWEN_PROGRAM)"]
fn the_installed_package_reads_the_system_defaults_variable() {
    let package = package_root(&common::installed());
    let names = [cockpit_qwen::SYSTEM_DEFAULTS_VARIABLE];

    let is_read = script_files(&package).iter().any(|file| {
        cockpit_provider_api::names_missing_from_file(file, &names)
            .unwrap()
            .is_empty()
    });

    assert!(is_read, "no file under {} reads it", package.display());
}

/// Every hook event the defaults subscribe the bridge to is one the package fires.
#[test]
#[ignore = "needs an installed Qwen Code (COCKPIT_QWEN_PROGRAM)"]
fn the_installed_package_fires_every_hook_event_the_bridge_subscribes_to() {
    let package = package_root(&common::installed());
    let files = script_files(&package);

    for event in cockpit_qwen::hooks::HOOK_EVENTS {
        let is_named = files.iter().any(|file| {
            cockpit_provider_api::names_missing_from_file(file, &[event])
                .unwrap()
                .is_empty()
        });
        assert!(is_named, "{event} is not in the package");
    }
}

/// The package folder the program belongs to: the nearest folder above the resolved program
/// with a `package.json`, or the standalone install's `lib/qwen-code` beside its `bin/qwen`.
fn package_root(program: &Path) -> PathBuf {
    let resolved = std::fs::canonicalize(program).unwrap();
    let standalone = resolved
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("lib/qwen-code"))
        .filter(|dir| dir.is_dir());
    if let Some(dir) = standalone {
        return dir;
    }
    resolved
        .ancestors()
        .find(|dir| dir.join("package.json").is_file())
        .unwrap_or_else(|| panic!("no package.json above {}", resolved.display()))
        .to_path_buf()
}

/// Every `.js` and `.mjs` file under `dir`, `node_modules` and the bundled Node left out.
fn script_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            if path.is_dir() {
                if name != "node_modules" && name != "node" {
                    pending.push(path);
                }
            } else if name.ends_with(".js") || name.ends_with(".mjs") {
                found.push(path);
            }
        }
    }
    found
}
