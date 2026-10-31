//! The wire form of the resource commands and views (#42).
use cockpit_protocol::{
    Command, EffectiveView, LimitView, ProfileView, Reply, VerdictView, decode_request,
};
use serde_json::json;

#[test]
fn a_resource_check_round_trips_without_required_controls_when_none_are_given() {
    let wire = json!({
        "id": 1,
        "command": {"kind": "resource-check", "workload": "build", "cpu_millis": 2000, "memory_mib": 1024}
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::ResourceCheck {
            workload: "build".to_owned(),
            cpu_millis: 2000,
            memory_mib: 1024,
            required: vec![],
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_profile_switch_names_the_profile_on_the_wire() {
    let wire = json!({"id": 2, "command": {"kind": "resource-profile", "profile": "foreground"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::ResourceProfile {
            profile: ProfileView::Foreground
        }
    );
}

#[test]
fn a_profile_this_version_does_not_know_reads_as_unknown() {
    let profile: ProfileView = serde_json::from_value(json!("eco")).unwrap();

    assert_eq!(profile, ProfileView::Unknown);
}

#[test]
fn a_held_verdict_carries_its_reason() {
    let reply = Reply::ResourceVerdict {
        verdict: VerdictView::Held {
            reason: "the queue is paused: nothing new starts".to_owned(),
        },
    };

    assert_eq!(
        serde_json::to_value(reply).unwrap(),
        json!({
            "kind": "resource-verdict",
            "verdict": {"kind": "held", "reason": "the queue is paused: nothing new starts"}
        })
    );
}

#[test]
fn a_limit_keeps_requested_supported_and_effective_apart() {
    let limit = LimitView {
        control: "cpu-quota".to_owned(),
        requested: Some("50%".to_owned()),
        supported: false,
        effective: EffectiveView::Unknown {
            why: "a process group has no cgroup".to_owned(),
        },
    };

    assert_eq!(
        serde_json::to_value(limit).unwrap(),
        json!({
            "control": "cpu-quota",
            "requested": "50%",
            "supported": false,
            "effective": {"kind": "unknown", "why": "a process group has no cgroup"}
        })
    );
}
