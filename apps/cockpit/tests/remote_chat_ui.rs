mod common;
use common::sandbox::*;

#[test]
fn mobile_chat_preserves_input_and_keeps_approvals_session_scoped() {
    let output = std::process::Command::new("node")
        .arg(repo().join("apps/cockpit/tests/remote_chat_ui.mjs"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
}

#[test]
fn mobile_page_navigates_badges_formats_and_keeps_reading_position() {
    let output = std::process::Command::new("node")
        .arg(repo().join("apps/cockpit/tests/remote_mobile_ui.mjs"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
}
