//! The wire form of bug reports (#427): an agent's `agent-report-bug`, the owner's verbs, and
//! the report the owner reviews.
use cockpit_protocol::{
    BugBundleView, BugFiledView, BugKindView, BugOriginView, BugReportView, BugStatusView, Command,
    Reply, decode_request,
};
use serde_json::json;

#[test]
fn an_agent_report_round_trips_with_only_what_it_says() {
    for (wire, command) in [
        (
            json!({"id": 1, "command": {"kind": "agent-report-bug",
                "description": "todo add hung", "bug-kind": "timeout"}}),
            None,
        ),
        (
            json!({"id": 2, "command": {"kind": "agent-report-bug", "launch_token": "t",
                "description": "x", "bug_kind": "internal", "component": "todo-add",
                "steps": "1. todo add", "requested_by_owner": true}}),
            Some(Command::AgentReportBug {
                launch_token: Some("t".to_owned()),
                description: "x".to_owned(),
                bug_kind: BugKindView::Internal,
                component: Some("todo-add".to_owned()),
                steps: "1. todo add".to_owned(),
                requested_by_owner: true,
            }),
        ),
    ] {
        let decoded = decode_request(&wire.to_string());
        match command {
            // Field names are snake_case like every other command's.
            None => assert!(decoded.is_err(), "{decoded:?}"),
            Some(command) => {
                let request = decoded.unwrap();
                assert_eq!(request.command, command);
                assert_eq!(serde_json::to_value(request).unwrap(), wire);
            }
        }
    }
}

#[test]
fn an_agent_report_without_a_claim_is_not_owner_requested() {
    let request = decode_request(
        &json!({"id": 1, "command": {"kind": "agent-report-bug", "description": "x",
            "bug_kind": "other"}})
        .to_string(),
    )
    .unwrap();

    assert!(matches!(
        request.command,
        Command::AgentReportBug {
            requested_by_owner: false,
            ..
        }
    ));
}

#[test]
fn the_owner_verbs_round_trip() {
    for (wire, command) in [
        (
            json!({"id": 1, "command": {"kind": "bug-report-new", "description": "it broke"}}),
            Command::BugReportNew {
                description: "it broke".to_owned(),
                title: None,
                steps: String::new(),
            },
        ),
        (
            json!({"id": 2, "command": {"kind": "bug-report-list", "all": true}}),
            Command::BugReportList { all: true },
        ),
        (
            json!({"id": 3, "command": {"kind": "bug-report-sent", "report": 4,
                "issue_url": "https://github.com/o/r/issues/1"}}),
            Command::BugReportSent {
                report: 4,
                issue_url: Some("https://github.com/o/r/issues/1".to_owned()),
            },
        ),
        (
            json!({"id": 4, "command": {"kind": "bug-report-discard", "report": 4}}),
            Command::BugReportDiscard { report: 4 },
        ),
    ] {
        let request = decode_request(&wire.to_string()).unwrap();

        assert_eq!(request.command, command);
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
    }
}

#[test]
fn a_filed_answer_names_the_report_and_what_happened() {
    let reply = Reply::BugReportFiled {
        report: 7,
        filed: BugFiledView::AlreadyHandled,
    };

    let wire = serde_json::to_value(&reply).unwrap();

    assert_eq!(
        wire,
        json!({"kind": "bug-report-filed", "report": 7, "filed": "already-handled"})
    );
    assert_eq!(serde_json::from_value::<Reply>(wire).unwrap(), reply);
}

#[test]
fn a_report_view_carries_its_bundle_state() {
    let report = BugReportView {
        id: 1,
        origin: BugOriginView::Daemon,
        requested_by_owner: false,
        session: None,
        status: BugStatusView::Draft,
        title: "a request panicked".to_owned(),
        description: "d".to_owned(),
        steps: String::new(),
        fingerprint: "cockpit-0.1.0/panic/bulk/todo-add".to_owned(),
        environment: "cockpit 0.1.0".to_owned(),
        bundle: BugBundleView::Failed {
            problem: "disk full".to_owned(),
        },
        occurrences: 1,
        issue_url: None,
        created_at: 1,
        changed_at: 2,
        issue: None,
    };

    let wire = serde_json::to_value(&report).unwrap();

    assert_eq!(
        wire["bundle"],
        json!({"state": "failed", "problem": "disk full"})
    );
    assert!(wire.get("issue").is_none());
    assert_eq!(
        serde_json::from_value::<BugReportView>(wire).unwrap(),
        report
    );
}
