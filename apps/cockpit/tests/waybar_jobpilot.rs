//! Jobpilot's Waybar consumer runs against synthetic state and systemd/date responses. The
//! producer's state stays read-only; no live Jobpilot commands or services are invoked.

mod common;
use common::sandbox::*;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

struct Waybar {
    sandbox: Sandbox,
    state: PathBuf,
}

impl Waybar {
    fn new() -> Self {
        let mut sandbox = Sandbox::new();
        sandbox.env.insert("TZ".into(), "UTC".into());
        sandbox.env.insert("TEST_DAY_START".into(), "1792454400".into());
        sandbox.env.insert("TEST_DAY_END".into(), "1792540800".into());
        sandbox.env.insert("TEST_RUN_STATE".into(), "inactive".into());
        sandbox.env.insert("TEST_LAST_EXIT".into(), "0".into());
        sandbox.env.insert("TEST_NEXT_RUN".into(), "Tue 2026-10-20 13:00:00 UTC".into());
        sandbox.shim("date", r#"case "$2" in
    'today 00:00') echo "$TEST_DAY_START" ;;
    'tomorrow 00:00') echo "$TEST_DAY_END" ;;
    *) exit 1 ;;
esac"#);
        sandbox.shim("systemctl", r#"case "$2" in
    is-active) echo "$TEST_RUN_STATE" ;;
    show) case "$5" in
        ExecMainStatus) echo "$TEST_LAST_EXIT" ;;
        NextElapseUSecRealtime) echo "$TEST_NEXT_RUN" ;;
        *) exit 1 ;;
    esac ;;
    *) exit 1 ;;
esac"#);
        let state = sandbox.root.join("state.json");
        Self { sandbox, state }
    }

    fn poll(&self) -> Value {
        let output = self.sandbox.run(waybar_scripts().join("jobpilot.sh"), &[], &[("JOBPILOT_STATE", self.state.to_str().unwrap())]);
        assert!(output.status.success(), "{}", stderr(&output));
        serde_json::from_str(&stdout(&output)).expect("Waybar JSON")
    }

    fn render(&self, state: Value) -> Value {
        fs::write(&self.state, state.to_string()).unwrap();
        self.poll()
    }
}

fn pending_offer(availability: Option<&str>) -> Value {
    let mut offer = json!({"status":"pending", "firstSeenAt":"2026-10-20T09:00:00.000Z",
        "offer":{"platform":"example", "title":"Engineer", "company":"Acme"}});
    if let Some(status) = availability {
        offer["availability"] = json!({"status":status, "checkedAt":"2026-10-20T09:30:00.000Z"});
    }
    offer
}

fn tooltip(module: &Value) -> &str { module["tooltip"].as_str().unwrap() }

#[test]
fn the_bar_counts_recent_offers_and_the_tooltip_keeps_the_rest_visible() {
    let waybar = Waybar::new();
    let module = waybar.render(json!({"applications":{},"manual":{
        "example:recent":pending_offer(Some("recently-found")),
        "example:missing":pending_offer(Some("not-returned")),
        "example:legacy":pending_offer(None),
        "example:future":pending_offer(Some("a-future-status"))
    }}));
    assert_eq!(module["text"], "0 (1)");
    assert!(tooltip(&module).contains("Recently found  1 (+1 new today)"));
    assert!(tooltip(&module).contains("Not returned  1"));
    assert!(tooltip(&module).contains("Unknown  2"));
}

#[test]
fn an_unknown_only_backlog_still_requires_attention() {
    let waybar = Waybar::new();
    let module = waybar.render(json!({"manual":{"example:legacy":pending_offer(None)}}));
    assert_eq!(module["text"], "0");
    assert_eq!(module["class"], json!(["attention"]));
    assert!(tooltip(&module).contains("Unknown  1"));
    assert!(tooltip(&module).contains("example: never"));
}

#[test]
fn a_not_returned_offer_is_not_claimed_to_be_closed() {
    let waybar = Waybar::new();
    let module = waybar.render(json!({"manual":{"example:missing":pending_offer(Some("not-returned"))}}));
    assert_eq!(module["text"], "0");
    assert_eq!(module["class"], json!(["attention"]));
    assert!(tooltip(&module).contains("Search absence does not prove a job is closed."));
    assert!(tooltip(&module).contains("Full backlog: jobpilot manual --all"));
}

#[test]
fn latest_platform_checks_are_compared_as_instants_not_timestamp_strings() {
    let waybar = Waybar::new();
    let mut earlier = pending_offer(Some("recently-found"));
    earlier["availability"]["checkedAt"] = json!("2026-10-20T10:00:00+02:00");
    let mut later = pending_offer(Some("not-returned"));
    later["availability"]["checkedAt"] = json!("2026-10-20T09:30:00Z");
    let module = waybar.render(json!({"manual":{"example:a":earlier,"example:b":later}}));
    assert!(tooltip(&module).contains("example: 2026-10-20T09:30:00Z"), "{}", tooltip(&module));
}

#[test]
fn latest_platform_checks_keep_fractional_second_ordering() {
    let waybar = Waybar::new();
    let mut later = pending_offer(Some("recently-found"));
    later["availability"]["checkedAt"] = json!("2026-10-20T09:30:00.120Z");
    let mut earlier = pending_offer(Some("recently-found"));
    earlier["availability"]["checkedAt"] = json!("2026-10-20T09:30:00.090Z");
    let module = waybar.render(json!({"manual":{"example:a":later,"example:b":earlier}}));
    assert!(tooltip(&module).contains("example: 2026-10-20T09:30:00.120Z"), "{}", tooltip(&module));
}

#[test]
fn legacy_offers_without_a_platform_have_an_explicit_unknown_platform() {
    let waybar = Waybar::new();
    for platform in [Value::Null, json!("")] {
        let mut offer = pending_offer(Some("recently-found"));
        offer["offer"]["platform"] = platform;
        let module = waybar.render(json!({"manual":{"legacy":offer}}));
        assert_eq!(module["text"], "0 (1)");
        assert!(tooltip(&module).contains("unknown: 2026-10-20T09:30:00.000Z"), "{}", tooltip(&module));
    }
}

#[test]
fn no_pending_offers_have_no_attention_class_or_check_times() {
    let waybar = Waybar::new();
    let module = waybar.render(json!({"applications":{},"manual":{}}));
    assert_eq!(module["text"], "0");
    assert_eq!(module["class"], json!([]));
    assert!(tooltip(&module).contains("no pending offers"));
}

#[test]
fn resolved_and_already_applied_offers_are_not_in_the_recent_backlog() {
    let waybar = Waybar::new();
    let mut resolved = pending_offer(Some("recently-found"));
    resolved["status"] = json!("sent");
    resolved["resolvedAt"] = json!("2026-10-20T08:00:00Z");
    let module = waybar.render(json!({"applications":{"example:applied":{"sentAt":"2026-10-20T08:00:00Z","title":"Sent","company":"Acme"}},
        "manual":{"example:applied":pending_offer(Some("recently-found")),"example:resolved":resolved}}));
    assert_eq!(module["text"], "1");
    assert!(tooltip(&module).contains("By hand today  1"));
    assert!(tooltip(&module).contains("Recently found  0"));
    assert_eq!(module["class"], json!([]));
}

#[test]
fn tooltip_fields_are_escaped_including_platform_and_run_metadata() {
    let mut waybar = Waybar::new();
    waybar.sandbox.env.insert("TEST_LAST_EXIT".into(), "<bad>".into());
    waybar.sandbox.env.insert("TEST_NEXT_RUN".into(), "next<&>".into());
    let mut offer = pending_offer(Some("recently-found"));
    offer["offer"] = json!({"platform":"example<&>","title":"<role>","company":"<company&>"});
    let module = waybar.render(json!({"manual":{"example:offer":offer}}));
    for escaped in ["example&lt;&amp;&gt;", "&lt;role&gt;", "&lt;company&amp;&gt;", "&lt;bad&gt;", "next&lt;&amp;&gt;"] {
        assert!(tooltip(&module).contains(escaped), "{escaped}: {}", tooltip(&module));
    }
}

#[test]
fn recent_offer_lists_are_capped_without_losing_the_total() {
    let waybar = Waybar::new();
    let manual: serde_json::Map<String, Value> = (0..12).map(|number| {
        let mut offer = pending_offer(Some("recently-found"));
        offer["offer"]["title"] = json!(format!("Offer {number}"));
        (format!("example:{number}"), offer)
    }).collect();
    let module = waybar.render(json!({"manual":manual}));
    assert_eq!(module["text"], "0 (12)");
    assert_eq!(tooltip(&module).matches("Offer ").count(), 10);
    assert!(tooltip(&module).contains("+2 more"));
}

#[test]
fn run_and_credential_status_classes_are_preserved() {
    let mut waybar = Waybar::new();
    for (run_state, exit, class) in [("inactive","0",json!([])), ("inactive","1",json!(["error"])),
        ("inactive","2",json!(["critical"])), ("active","0",json!(["running"])), ("activating","0",json!(["running"]))] {
        waybar.sandbox.env.insert("TEST_RUN_STATE".into(), run_state.into());
        waybar.sandbox.env.insert("TEST_LAST_EXIT".into(), exit.into());
        assert_eq!(waybar.render(json!({"manual":{}}))["class"], class);
    }
}

#[test]
fn local_day_counts_keep_both_repeated_hours_on_the_dst_fallback_day() {
    let mut waybar = Waybar::new();
    waybar.sandbox.env.insert("TZ".into(), "Europe/Paris".into());
    waybar.sandbox.env.insert("TEST_DAY_START".into(), "1792879200".into());
    waybar.sandbox.env.insert("TEST_DAY_END".into(), "1792969200".into());
    let applications = json!({
        "example:first":{"sentAt":"2026-10-25T02:30:00.100+02:00","title":"first","company":"Acme"},
        "example:second":{"sentAt":"2026-10-25T02:30:00.100+01:00","title":"second","company":"Acme"},
        "example:late":{"sentAt":"2026-10-25T23:30:00+01:00","title":"late","company":"Acme"},
        "example:next":{"sentAt":"2026-10-26T00:00:00+01:00","title":"tomorrow","company":"Acme"}
    });
    assert_eq!(waybar.render(json!({"applications":applications,"manual":{}}))["text"], "3");
}

#[test]
fn polling_never_rewrites_jobpilots_state() {
    let waybar = Waybar::new();
    let state = json!({"manual":{"example:recent":pending_offer(Some("recently-found"))}}).to_string();
    fs::write(&waybar.state, &state).unwrap();
    let modified = fs::metadata(&waybar.state).unwrap().modified().unwrap();
    waybar.poll();
    assert_eq!(fs::read_to_string(&waybar.state).unwrap(), state);
    assert_eq!(fs::metadata(&waybar.state).unwrap().modified().unwrap(), modified);
}
