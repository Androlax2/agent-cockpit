use cockpit::chief::{summary, view::ChiefView};
use serde_json::json;

#[test]
fn headless_status_matches_the_view_for_startup_and_retained_grants() {
    let session = json!({"session_id":"chief-a","runtime":"runtime-a"});
    let cases = [
        (json!({"records":[]}), "Off"),
        (json!({"records":[],"startup":{"state":"ended"}}), "Off"),
        (
            json!({"records":[],"startup":{"state":"starting"}}),
            "Starting",
        ),
        (
            json!({"records":[],"startup":{"state":"failed","can_retry":true}}),
            "Could not start",
        ),
        (
            json!({"records":[],"startup":{"state":"failed","can_retry":false}}),
            "Checking",
        ),
        (
            json!({"records":[{"kind":"chief","state":"active","live_verified":false}]}),
            "Unavailable · retained active grant",
        ),
        (
            json!({"records":[{"kind":"chief","state":"active","live_verified":true}]}),
            "Active",
        ),
        (
            json!({"records":[{"kind":"chief","state":"paused","live_verified":1}]}),
            "Paused · workers continue",
        ),
        (
            json!({"records":[{"kind":"chief","state":"other","live_verified":true}]}),
            "Unknown chief state — host review required",
        ),
        (
            json!({"records":[
                {"kind":"chief","state":"active","live_verified":true,"session":session},
                {"kind":"worker","session":session,"blocked_by":["fresh_native_availability_required"]}
            ]}),
            "Unavailable · retained active grant · fresh_native_availability_required",
        ),
    ];
    for (snapshot, expected) in cases {
        assert_eq!(summary::status(&snapshot), expected, "{snapshot}");
        assert_eq!(ChiefView::new(snapshot).status(), expected);
    }
}

#[test]
fn a_failed_review_does_not_present_retained_grant_data_as_current() {
    let mut view = ChiefView::new(json!({"records":[
        {"kind":"chief","state":"active","live_verified":true}
    ]}));
    view.read_failed("fixture unavailable".into());
    assert_eq!(
        view.status(),
        "Unknown / host review unavailable — retained records are stale"
    );
}
