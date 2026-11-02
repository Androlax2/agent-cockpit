//! The installer is an owner command that works before a daemon has started.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn install_reports_a_download_failure_without_connecting_to_a_daemon() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let curl = bin.join("curl");
    fs::write(&curl, "#!/bin/sh\nexit 22\n").unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cockpit"))
        .args(["sandbox", "install"])
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("download"), "{stderr}");
    assert!(!stderr.contains("daemon.sock"), "{stderr}");
}
