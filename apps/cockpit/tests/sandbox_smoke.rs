//! The test sandbox itself: shims log their calls, and the environment reaches the scripts.

mod common;
use common::sandbox::{Sandbox, stdout};

#[test]
fn a_shim_logs_each_call() {
    let sandbox = Sandbox::new();
    sandbox.shim("hyprctl", "echo answered");
    let output = sandbox.run("hyprctl", &["-j", "activeworkspace"], &[]);
    assert_eq!(stdout(&output), "answered\n");
    assert_eq!(sandbox.calls("hyprctl"), ["hyprctl -j activeworkspace"]);
}

#[test]
fn scripts_see_the_sandbox_home() {
    let sandbox = Sandbox::new();
    let output = sandbox.run("sh", &["-c", "echo $HOME"], &[]);
    assert_eq!(stdout(&output).trim(), sandbox.home.display().to_string());
}
