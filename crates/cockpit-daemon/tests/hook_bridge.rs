//! Issue #203 through the real daemon binary and the real `cockpit`: the daemon runs each
//! adapter's bridge installed beside it at its start (the Claude hook, the Codex hook and
//! session launcher (#411), the OpenCode bridge with the plugin it writes (#415)), and
//! `cockpit provider list` says whether each provider reports its agents' activity, which
//! the release's clean-install check (`tools/release.py verify-install`) reads.

mod real_daemon;

use real_daemon::{Home, cockpit_program};

/// The bridge programs the release ships beside the daemon.
const BRIDGE_PROGRAMS: [&str; 4] = [
    "cockpit-claude-hook",
    "cockpit-codex-hook",
    "cockpit-codex-session",
    "cockpit-opencode-bridge",
];

#[tokio::test(flavor = "multi_thread")]
async fn provider_list_shows_each_provider_reports_events_through_the_bridges_beside_the_daemon() {
    for program in BRIDGE_PROGRAMS {
        assert!(
            cockpit_program().with_file_name(program).is_file(),
            "{program} is missing beside the daemon: run the workspace's tests \
             (`make crates-test`), which build it"
        );
    }
    let home = Home::new();
    let _daemon = home.start();

    let listed = home.cockpit(&["provider", "list"], &[]).await;

    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "claude\tresume\tevents\ncodex\tresume\tevents\nopencode\tresume\tevents\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_opencode_plugin_the_daemon_writes_is_this_releases() {
    let home = Home::new();
    let _daemon = home.start();

    let plugin = home
        .path()
        .join("state/cockpit/opencode-plugin/cockpit-opencode-plugin.js");

    assert_eq!(
        std::fs::read_to_string(&plugin).unwrap(),
        cockpit_opencode::PLUGIN_SOURCE
    );
}
