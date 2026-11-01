//! The wire form of the readiness report (#50): `readiness`, with or without a project, and
//! the requirements it answers.
use cockpit_protocol::{Command, ReadinessStatusView, Reply, RequirementView, decode_request};
use serde_json::json;

#[test]
fn a_readiness_request_round_trips_with_and_without_a_project() {
    for (wire, command) in [
        (
            json!({"id": 1, "command": {"kind": "readiness"}}),
            Command::Readiness { project: None },
        ),
        (
            json!({"id": 2, "command": {"kind": "readiness", "project": "/p/app"}}),
            Command::Readiness {
                project: Some("/p/app".to_owned()),
            },
        ),
    ] {
        let request = decode_request(&wire.to_string()).unwrap();

        assert_eq!(request.command, command);
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
    }
}

#[test]
fn each_requirement_carries_its_status_and_only_a_next_action_it_has() {
    let reply = Reply::Readiness {
        requirements: vec![
            RequirementView {
                name: "tmux".to_owned(),
                status: ReadinessStatusView::Available,
                detail: "tmux 3.5".to_owned(),
                next: None,
            },
            RequirementView {
                name: "docker".to_owned(),
                status: ReadinessStatusView::MissingPrerequisite,
                detail: "no engine answered".to_owned(),
                next: Some("start a Docker engine".to_owned()),
            },
        ],
    };

    let wire = serde_json::to_value(&reply).unwrap();

    assert_eq!(
        wire,
        json!({
            "kind": "readiness",
            "requirements": [
                {"name": "tmux", "status": "available", "detail": "tmux 3.5"},
                {"name": "docker", "status": "missing-prerequisite",
                 "detail": "no engine answered", "next": "start a Docker engine"}
            ]
        })
    );
    assert_eq!(serde_json::from_value::<Reply>(wire).unwrap(), reply);
}

#[test]
fn every_status_has_its_report_09_name_on_the_wire() {
    for (status, name) in [
        (ReadinessStatusView::Available, "available"),
        (
            ReadinessStatusView::NeedsAuthorization,
            "needs-authorization",
        ),
        (
            ReadinessStatusView::MissingPrerequisite,
            "missing-prerequisite",
        ),
        (ReadinessStatusView::Unsupported, "unsupported"),
        (ReadinessStatusView::Unknown, "unknown"),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), json!(name));
        assert_eq!(status.as_str(), name);
    }
}
