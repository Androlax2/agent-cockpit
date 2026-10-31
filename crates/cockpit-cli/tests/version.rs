//! `cockpit version` driven as a process, from a copy laid out the way a release bundle is.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const MANIFEST: &str = "{\n  \"name\": \"cockpit\",\n  \"source_revision\": \"0123abcd\"\n}\n";

/// An unpacked bundle: `bin/cockpit` (a copy of the binary under test) and, when written,
/// `share/cockpit/manifest.json`.
struct Bundle {
    prefix: TempDir,
}

impl Bundle {
    fn unpacked() -> Self {
        let prefix = tempfile::tempdir().expect("temporary prefix");
        fs::create_dir(prefix.path().join("bin")).unwrap();
        fs::copy(
            env!("CARGO_BIN_EXE_cockpit"),
            prefix.path().join("bin/cockpit"),
        )
        .unwrap();
        Self { prefix }
    }

    fn with_manifest(self, contents: &str) -> Self {
        let manifest = self.manifest_path();
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(manifest, contents).unwrap();
        self
    }

    fn manifest_path(&self) -> PathBuf {
        self.prefix.path().join("share/cockpit/manifest.json")
    }

    fn binary(&self) -> PathBuf {
        self.prefix.path().join("bin/cockpit")
    }
}

fn version(binary: &Path, arguments: &[&str]) -> Output {
    Command::new(binary)
        .arg("version")
        .args(arguments)
        .env_clear()
        .output()
        .expect("cockpit runs")
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "exit {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn version_names_the_package_version() {
    let printed = stdout(&version(Bundle::unpacked().binary().as_path(), &[]));
    assert!(
        printed.starts_with(&format!("cockpit {} (revision ", env!("CARGO_PKG_VERSION"))),
        "{printed}"
    );
    assert_eq!(printed.lines().count(), 1, "{printed}");
}

#[test]
fn version_all_shows_the_bundle_manifest_installed_beside_the_binary() {
    let bundle = Bundle::unpacked().with_manifest(MANIFEST);
    let printed = stdout(&version(&bundle.binary(), &["--all"]));
    let manifest_path = bundle.manifest_path().canonicalize().unwrap();
    assert!(
        printed.contains(&format!(
            "bundle manifest {}:\n{MANIFEST}",
            manifest_path.display()
        )),
        "{printed}"
    );
}

#[cfg(unix)]
#[test]
fn version_all_follows_a_symlinked_binary_to_its_bundle() {
    let bundle = Bundle::unpacked().with_manifest(MANIFEST);
    let elsewhere = tempfile::tempdir().unwrap();
    let link = elsewhere.path().join("cockpit");
    std::os::unix::fs::symlink(bundle.binary(), &link).unwrap();
    let printed = stdout(&version(&link, &["--all"]));
    assert!(
        printed.contains("\"source_revision\": \"0123abcd\""),
        "{printed}"
    );
}

#[test]
fn version_all_without_a_bundle_says_so() {
    let bundle = Bundle::unpacked();
    let printed = stdout(&version(&bundle.binary(), &["--all"]));
    assert!(
        printed.contains("bundle manifest: none at ")
            && printed.contains("(a build outside a release bundle)"),
        "{printed}"
    );
}

#[test]
fn version_all_fails_when_the_manifest_cannot_be_read() {
    let bundle = Bundle::unpacked();
    fs::create_dir_all(bundle.manifest_path()).unwrap();
    let output = version(&bundle.binary(), &["--all"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("manifest.json"),
        "{output:?}"
    );
}

#[test]
fn version_rejects_another_argument_as_a_usage_error() {
    let output = version(Bundle::unpacked().binary().as_path(), &["--every"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("version takes no argument but --all"),
        "{output:?}"
    );
}
