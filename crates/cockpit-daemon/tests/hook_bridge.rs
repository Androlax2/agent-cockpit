//! Issue #203 through the real daemon binary and the real `cockpit`: the daemon runs the
//! Claude hook bridge installed beside it at its start, and `cockpit provider list` says
//! whether Claude Code reports its agents' activity, which the release's clean-install check
//! (`tools/release.py verify-install`) reads.

mod real_daemon;

use real_daemon::{Home, cockpit_program};

#[tokio::test(flavor = "multi_thread")]
async fn provider_list_shows_claude_reports_events_through_the_bridge_beside_the_daemon() {
    assert!(
        cockpit_program()
            .with_file_name("cockpit-claude-hook")
            .is_file(),
        "the hook bridge is missing beside the daemon: run the workspace's tests \
         (`make crates-test`), which build it"
    );
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
        "claude\tresume\tevents\n"
    );
}
