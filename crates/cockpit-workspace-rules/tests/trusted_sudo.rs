//! A release build trusts the host's `/usr/bin/sudo` alone for sudo's askpass (#1374, the
//! owner's decision C): the one constructor that names another sudo, for tests' stand-ins,
//! is compiled only with debug assertions, no product source calls it, and the release
//! profile keeps debug assertions off. No caller is the guarantee: a release built with
//! debug assertions forced on (`RUSTFLAGS`) holds the constructor and still never calls it.

use std::fs;
use std::path::{Path, PathBuf};

const STAND_IN: &str = "with_stand_in";
const DEFINED_IN: &str = "crates/cockpit-approvals/src/sudo.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate lives in crates/<name>/")
        .to_path_buf()
}

/// Every `.rs` file under each crate's `src/`.
fn product_sources() -> Vec<PathBuf> {
    let mut sources = Vec::new();
    for member in fs::read_dir(repo_root().join("crates")).unwrap().flatten() {
        let mut pending = vec![member.path().join("src")];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for path in entries.flatten().map(|entry| entry.path()) {
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    sources.push(path);
                }
            }
        }
    }
    sources.sort();
    sources
}

#[test]
fn the_stand_in_sudo_constructor_exists_only_with_debug_assertions() {
    let text = fs::read_to_string(repo_root().join(DEFINED_IN)).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let definition = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .starts_with(&format!("pub fn {STAND_IN}("))
        })
        .expect("the stand-in constructor is defined");

    let attributes: Vec<&str> = lines[..definition]
        .iter()
        .rev()
        .map(|line| line.trim())
        .take_while(|line| line.starts_with("#[") || line.starts_with("///"))
        .filter(|line| line.starts_with("#["))
        .collect();

    assert_eq!(attributes, ["#[cfg(debug_assertions)]"]);
}

#[test]
fn no_product_source_trusts_a_stand_in_sudo() {
    let callers: Vec<String> = product_sources()
        .into_iter()
        .filter(|path| !path.ends_with(DEFINED_IN))
        .filter(|path| fs::read_to_string(path).is_ok_and(|text| text.contains(STAND_IN)))
        .map(|path| path.display().to_string())
        .collect();

    assert_eq!(callers, Vec::<String>::new());
}

#[test]
fn the_release_profile_keeps_debug_assertions_off() {
    let manifest = fs::read_to_string(repo_root().join("Cargo.toml")).unwrap();
    let release = manifest
        .split("\n[")
        .find(|section| section.starts_with("profile.release]"))
        .unwrap_or_default();

    assert!(!release.contains("debug-assertions"), "{release}");
}
