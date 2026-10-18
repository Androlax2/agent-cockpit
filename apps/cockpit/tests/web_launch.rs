//! web-launch.sh: the web scratchpads' Zen profiles get the shared prefs on every start.

mod common;
use common::sandbox::*;
use std::fs;
use std::os::unix::fs::symlink;

fn launch(sandbox: &Sandbox, url: &str) -> String {
    let script = repo().join("config/hyprland/.config/hypr/scripts/web-launch.sh");
    let output = sandbox.run("bash", &[script.to_str().unwrap(), "prs", url], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    fs::read_to_string(sandbox.home.join(".zen/prs/user.js")).unwrap()
}

#[test]
fn a_profile_keeps_its_home_page_and_gets_the_one_tab_prefs() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.join(".config/zen")).unwrap();
    symlink(repo().join("config/zen/.config/zen/scratchpad.user.js"), sandbox.home.join(".config/zen/scratchpad.user.js")).unwrap();
    // -CreateProfile makes the folder; a start just logs its arguments.
    sandbox.shim("zen-browser", r#"[[ $1 == -CreateProfile ]] && mkdir -p "${2#* }"; exit 0"#);
    let first = launch(&sandbox, "https://github.com/pulls");
    assert!(first.contains(r#"user_pref("browser.startup.homepage", "https://github.com/pulls");"#));
    let again = launch(&sandbox, "https://github.com/o/r/pull/7"); // started straight on a pull request
    assert!(again.contains(r#"user_pref("browser.startup.homepage", "https://github.com/pulls");"#));
    assert!(!again.contains("pull/7"));
    assert!(again.contains(r#"user_pref("browser.link.open_newwindow.override.external", 1);"#));
    assert_eq!(sandbox.calls("zen-browser").last().unwrap(), "zen-browser -P prs https://github.com/o/r/pull/7");
}
