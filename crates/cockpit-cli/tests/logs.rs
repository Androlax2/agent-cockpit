mod common;

use common::TestHome;

#[test]
fn owner_can_set_inspect_and_revert_the_running_daemons_level() {
    let mut home = TestHome::new();
    home.daemon_env
        .push(("TORLEK_LOG_LEVEL".into(), "warn".into()));
    let _daemon = home.start();
    let baseline = home.cockpit_ok(&["logs", "level"]);
    assert!(
        baseline.contains("warn") && baseline.contains("no active override"),
        "{baseline}"
    );
    let active = home.cockpit_ok(&["logs", "level", "debug"]);
    assert!(
        active.contains("debug") && active.contains("expires"),
        "{active}"
    );
    assert!(home.cockpit_ok(&["logs", "level"]).contains("debug"));
    for _ in 0..2 {
        let reverted = home.cockpit_ok(&["logs", "level", "--revert"]);
        assert!(
            reverted.contains("warn") && reverted.contains("no active override"),
            "{reverted}"
        );
    }
    assert!(
        home.cockpit_ok(&["logs", "level", "tmux=trace", "--for", "15m"])
            .contains("tmux=trace")
    );
}

#[test]
fn invalid_override_leaves_the_previous_override_in_effect() {
    let home = TestHome::new();
    let _daemon = home.start();
    home.cockpit_ok(&["logs", "level", "debug"]);
    for (level, duration) in [
        ("loud", "1s"),
        ("trace", "0s"),
        ("trace", "16m"),
        ("trace", "18446744073709551615m"),
    ] {
        let failed = home.cockpit(&["logs", "level", level, "--for", duration]);
        assert_eq!(failed.status.code(), Some(1), "{failed:?}");
        assert!(
            String::from_utf8_lossy(&failed.stderr).contains("invalid-argument"),
            "{failed:?}"
        );
        assert!(home.cockpit_ok(&["logs", "level"]).contains("debug"));
    }
}

#[test]
fn malformed_log_arguments_are_usage_errors_without_a_daemon() {
    let home = TestHome::new();
    for arguments in [
        vec!["logs"],
        vec!["logs", "level", "--for"],
        vec!["logs", "level", "--revert", "--for", "1s"],
        vec!["logs", "level", "debug", "--for"],
        vec!["logs", "level", "debug", "--for", "--revert"],
        vec!["logs", "level", "debug", "--for", "--unknown"],
        vec!["logs", "level", "debug", "--for", "1s", "--for", "2s"],
        vec!["logs", "level", "--unknown"],
    ] {
        let output = home.cockpit(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
    }
}
