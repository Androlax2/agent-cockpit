//! The hot corner (hypr/hotcorner.sh): locks in the bottom-left corner, cheaply.

mod common;
use common::sandbox::*;
use std::fs;
use std::os::unix::fs::symlink;
use std::time::Duration;

fn corner_sandbox() -> Sandbox {
    let sandbox = Sandbox::new();
    let scripts = sandbox.home.join(".config/hypr/scripts");
    fs::create_dir_all(&scripts).unwrap();
    symlink(repo().join("config/hyprland/.config/hypr/scripts/is-gaming.sh"), scripts.join("is-gaming.sh")).unwrap();
    sandbox.shim("lock", "");
    sandbox
}

fn watch(sandbox: &Sandbox, cursor: &str) {
    sandbox.shim("hyprctl", &format!(r#"case "$*" in
  cursorpos) echo "{cursor}" ;;
  "monitors -j") echo '[{{"x": 0, "y": 0, "width": 3840, "height": 1600}}]' ;;
  "activewindow -j") echo '{{"class": "kitty", "fullscreen": 0}}' ;;
esac"#));
    let script = repo().join("config/hyprland/.config/hypr/hotcorner.sh");
    let lock = sandbox.shims.join("lock").display().to_string();
    let mut corner = sandbox.command("bash", &[("HOTCORNER_COMMAND", &lock)]).arg(script).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(1600));
    corner.kill().unwrap();
    corner.wait().unwrap();
}

#[test]
fn the_corner_locks() {
    let sandbox = corner_sandbox();
    watch(&sandbox, "3, 1597");
    assert!(!sandbox.calls("lock").is_empty());
}

#[test]
fn elsewhere_nothing_happens() {
    let sandbox = corner_sandbox();
    watch(&sandbox, "1920, 800");
    assert!(sandbox.calls("lock").is_empty());
}

#[test]
fn the_layout_is_not_read_on_every_poll() {
    let sandbox = corner_sandbox();
    watch(&sandbox, "1920, 800");
    let calls = sandbox.calls("hyprctl");
    assert!(calls.iter().filter(|call| *call == "hyprctl cursorpos").count() >= 6);
    assert_eq!(calls.iter().filter(|call| *call == "hyprctl monitors -j").count(), 1);
}
