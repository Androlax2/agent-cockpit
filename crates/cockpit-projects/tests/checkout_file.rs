//! Reading a checkout's top-level file without following it elsewhere (#1371).

use std::process::Command;

use cockpit_projects::read_checkout_file;

const MAX_BYTES: u64 = 64;

#[test]
fn a_plain_file_is_read_and_a_missing_one_is_none() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("compose.yaml"), "services: {}\n").unwrap();

    assert_eq!(
        read_checkout_file(dir.path(), "compose.yaml", MAX_BYTES),
        Ok(Some("services: {}\n".to_owned()))
    );
    assert_eq!(
        read_checkout_file(dir.path(), "compose.yml", MAX_BYTES),
        Ok(None)
    );
}

#[test]
fn a_symbolic_link_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("elsewhere"), "secret\n").unwrap();
    std::os::unix::fs::symlink("elsewhere", dir.path().join("compose.yaml")).unwrap();

    let read = read_checkout_file(dir.path(), "compose.yaml", MAX_BYTES);

    assert_eq!(
        read,
        Err("it is a symbolic link, which is not followed".to_owned())
    );
}

#[test]
fn a_fifo_is_refused_without_waiting_for_a_writer() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("compose.yaml");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success());

    let read = read_checkout_file(dir.path(), "compose.yaml", MAX_BYTES);

    assert_eq!(read, Err("it is not a plain file".to_owned()));
}

#[test]
fn a_file_over_the_limit_or_a_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("compose.yaml"), "x".repeat(65)).unwrap();

    assert_eq!(
        read_checkout_file(dir.path(), "compose.yaml", MAX_BYTES),
        Err("it is larger than 64 bytes".to_owned())
    );
    assert!(read_checkout_file(dir.path(), "../compose.yaml", MAX_BYTES).is_err());
}
