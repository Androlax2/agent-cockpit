//! The wire form of `ask run`, `ask guide` and the owner's decisions (#34): requests name no
//! session, and an approval names the revision and digest it was made on.
use cockpit_protocol::{
    Command, FoldedView, HiddenView, Outcome, Reply, RunRequestView, RunResultView, RunStateView,
    ShownText, decode_request,
};
use serde_json::json;

fn shown(text: &str) -> ShownText {
    ShownText {
        text: text.to_owned(),
        has_hidden_characters: false,
        non_ascii: Vec::new(),
        warnings: Vec::new(),
    }
}

#[test]
fn an_ask_run_round_trips_through_its_wire_form() {
    let wire = json!({
        "id": 3,
        "command": {
            "kind": "ask-run",
            "launch_token": "abc",
            "command": "make test",
            "cwd": "/home/theo/project",
            "reason": "Runs the tests the sandbox cannot."
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::AskRun {
            launch_token: Some("abc".to_owned()),
            command: "make test".to_owned(),
            cwd: "/home/theo/project".to_owned(),
            reason: "Runs the tests the sandbox cannot.".to_owned(),
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_session_claimed_in_an_ask_is_not_read() {
    let wire = json!({
        "id": 1,
        "command": {"kind": "ask-guide", "session": "someone-else", "guide": "# T\nStep", "cwd": "/"}
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::AskGuide {
            launch_token: None,
            guide: "# T\nStep".to_owned(),
            cwd: "/".to_owned(),
        }
    );
}

#[test]
fn an_approval_without_its_revision_and_digest_is_malformed() {
    for command in [
        json!({"kind": "approval-approve", "request": 1}),
        json!({"kind": "approval-approve", "request": 1, "revision": 1}),
        json!({"kind": "approval-approve", "request": 1, "digest": "ab"}),
    ] {
        let wire = json!({"id": 1, "command": command});
        let rejection = decode_request(&wire.to_string()).unwrap_err();
        assert_eq!(
            rejection.error.code,
            cockpit_protocol::ErrorCode::InvalidRequest,
            "{command}"
        );
    }
}

#[test]
fn each_owner_decision_has_its_kebab_case_kind() {
    let cases = [
        (
            json!({"kind": "approval-approve", "request": 4, "revision": 2, "digest": "d"}),
            Command::ApprovalApprove {
                request: 4,
                revision: 2,
                digest: "d".to_owned(),
                through: None,
            },
        ),
        (
            json!({"kind": "approval-edit", "request": 4, "revision": 1, "command": "ls"}),
            Command::ApprovalEdit {
                request: 4,
                revision: 1,
                command: "ls".to_owned(),
            },
        ),
        (
            json!({"kind": "approval-input", "request": 4, "text": "y"}),
            Command::ApprovalInput {
                request: 4,
                text: "y".to_owned(),
                shown: false,
            },
        ),
        (
            json!({"kind": "approval-input", "request": 4, "text": "y", "shown": true}),
            Command::ApprovalInput {
                request: 4,
                text: "y".to_owned(),
                shown: true,
            },
        ),
        (
            json!({"kind": "approval-stop", "request": 4}),
            Command::ApprovalStop { request: 4 },
        ),
        (
            json!({"kind": "approval-purge-output", "request": 4}),
            Command::ApprovalPurgeOutput { request: 4 },
        ),
        (
            json!({"kind": "guide-answer", "guide": 2, "step": 0}),
            Command::GuideAnswer {
                guide: 2,
                step: 0,
                answer: None,
            },
        ),
        (
            json!({"kind": "guide-stuck", "guide": 2, "step": 1, "note": "no button"}),
            Command::GuideStuck {
                guide: 2,
                step: 1,
                note: "no button".to_owned(),
            },
        ),
        (json!({"kind": "approval-queue"}), Command::ApprovalQueue),
    ];
    for (command, expected) in cases {
        let wire = json!({"id": 1, "command": command});
        assert_eq!(
            decode_request(&wire.to_string()).unwrap().command,
            expected,
            "{command}"
        );
    }
}

/// A finished `make test` that failed.
fn finished_run() -> RunRequestView {
    RunRequestView {
        id: 4,
        session: "s1".to_owned(),
        revision: 2,
        state: RunStateView::Finished,
        command: "make test".to_owned(),
        asked_command: Some("make".to_owned()),
        cwd: "/p".to_owned(),
        reason: "tests".to_owned(),
        shown_command: shown("make test"),
        shown_cwd: shown("/p"),
        shown_reason: shown("tests"),
        project: Some("p".to_owned()),
        outside_project: false,
        folded_command: None,
        timeout_without_foreground: false,
        unchecked_shell: None,
        changed_build_files: Some(Vec::new()),
        digest: "d".repeat(64),
        asked_at: 1,
        expires_at: 901,
        waiting_for_input: false,
        waiting_prompt: None,
        sudo_ask: None,
        sudo_ran_unasked: 0,
        typed_secret_redacted: 0,
        root_runs: None,
        decision: None,
        result: Some(RunResultView {
            exit_code: 2,
            output: "fail".to_owned(),
            omitted_lines: 3,
            truncated_bytes: 0,
            redacted: 1,
            output_retired: false,
            stopped_by_owner: false,
            sudo_ran_unasked: 0,
            typed_secret_redacted: 0,
        }),
        note: None,
        already_asked: false,
    }
}

#[test]
fn a_finished_run_request_reply_round_trips() {
    let reply = Outcome::Ok(Reply::RunRequest {
        request: Box::new(finished_run()),
    });

    let wire = serde_json::to_value(&reply).unwrap();

    assert_eq!(wire["ok"]["kind"], "run-request");
    assert_eq!(wire["ok"]["request"]["state"], "finished");
    assert_eq!(wire["ok"]["request"]["result"]["exit_code"], 2);
    assert_eq!(serde_json::from_value::<Outcome>(wire).unwrap(), reply);
}

#[test]
fn the_typed_secret_redactions_are_counted_live_then_by_the_result() {
    let running = RunRequestView {
        state: RunStateView::Running,
        result: None,
        typed_secret_redacted: 2,
        ..finished_run()
    };
    let mut finished = finished_run();
    finished.result.as_mut().unwrap().typed_secret_redacted = 3;

    assert_eq!(running.typed_secret_redacted_so_far(), 2);
    assert_eq!(finished.typed_secret_redacted_so_far(), 3);
}

#[test]
fn the_sudo_that_ran_unasked_is_counted_live_then_by_the_result() {
    let running = RunRequestView {
        state: RunStateView::Running,
        result: None,
        sudo_ran_unasked: 2,
        ..finished_run()
    };
    let mut finished = finished_run();
    finished.result.as_mut().unwrap().sudo_ran_unasked = 3;

    assert_eq!(running.sudo_ran_unasked_so_far(), 2);
    assert_eq!(finished.sudo_ran_unasked_so_far(), 3);
}

#[test]
fn each_run_state_has_its_kebab_case_name() {
    for (state, name) in [
        (RunStateView::NotRun, "not-run"),
        (RunStateView::OutcomeUnknown, "outcome-unknown"),
        (RunStateView::Asking, "asking"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), json!(name));
    }
}

#[test]
fn each_hidden_kind_has_its_kebab_case_name_and_an_unknown_one_is_kept_as_a_warning() {
    for (kind, name) in [
        (HiddenView::BidiControl, "bidi-control"),
        (HiddenView::Invisible, "invisible"),
        (HiddenView::Control, "control"),
        (HiddenView::BlankLines, "blank-lines"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(name));
    }
    let later: HiddenView = serde_json::from_value(json!("lookalike-script")).unwrap();
    assert_eq!(later, HiddenView::Unknown);
}

#[test]
fn the_owner_hints_round_trip_on_a_run_request() {
    let wire = json!({
        "id": 1, "session": "s1", "revision": 1, "state": "asking",
        "command": "timeout 5 make", "asked_command": null, "cwd": "/p", "reason": "r",
        "shown_command": {"text": "timeout 5 make", "has_hidden_characters": false,
            "non_ascii": [], "warnings": ["control"]},
        "shown_cwd": {"text": "/p", "has_hidden_characters": false, "non_ascii": []},
        "shown_reason": {"text": "r", "has_hidden_characters": false, "non_ascii": []},
        "project": "p", "outside_project": false,
        "folded_command": {"preview": "timeout", "more_lines": 0, "more_chars": 7},
        "timeout_without_foreground": true,
        "changed_build_files": ["Makefile"],
        "digest": "d", "asked_at": 1, "expires_at": 2, "waiting_for_input": false,
        "result": null, "note": null
    });

    let view: RunRequestView = serde_json::from_value(wire).unwrap();

    assert_eq!(view.shown_command.warnings, [HiddenView::Control]);
    assert!(view.shown_cwd.warnings.is_empty());
    assert!(view.timeout_without_foreground);
    assert_eq!(view.changed_build_files, Some(vec!["Makefile".to_owned()]));
    assert_eq!(
        view.folded_command,
        Some(FoldedView {
            preview: "timeout".to_owned(),
            more_lines: 0,
            more_chars: 7
        })
    );
}

#[test]
fn a_run_result_marks_retired_output_and_reads_without_the_mark() {
    let older = json!({"exit_code":0,"output":"ok","omitted_lines":0,"truncated_bytes":0,
        "redacted":0});
    let result: RunResultView = serde_json::from_value(older.clone()).unwrap();
    assert!(!result.output_retired);
    assert_eq!(serde_json::to_value(&result).unwrap(), older);
    let retired = json!({"exit_code":0,"output":"","omitted_lines":0,"truncated_bytes":0,
        "redacted":0,"output_retired":true});
    let result: RunResultView = serde_json::from_value(retired.clone()).unwrap();
    assert!(result.output_retired);
    assert_eq!(serde_json::to_value(&result).unwrap(), retired);
}

#[test]
fn a_run_result_marks_the_owners_stop_and_reads_without_the_mark() {
    let older = json!({"exit_code":143,"output":"","omitted_lines":0,"truncated_bytes":0,
        "redacted":0});
    let result: RunResultView = serde_json::from_value(older).unwrap();
    assert!(!result.stopped_by_owner);
    let stopped = json!({"exit_code":143,"output":"","omitted_lines":0,"truncated_bytes":0,
        "redacted":0,"stopped_by_owner":true});
    let result: RunResultView = serde_json::from_value(stopped.clone()).unwrap();
    assert!(result.stopped_by_owner);
    assert_eq!(serde_json::to_value(&result).unwrap(), stopped);
}

#[test]
fn the_decided_lists_round_trip_through_their_wire_forms() {
    for (command, wire) in [
        (
            Command::ApprovalDecided { since: None },
            json!({"kind": "approval-decided"}),
        ),
        (
            Command::ApprovalDecided {
                since: Some(1_767_323_045),
            },
            json!({"kind": "approval-decided", "since": 1_767_323_045}),
        ),
        (Command::NetworkDecided, json!({"kind": "network-decided"})),
    ] {
        let wire = json!({"id": 1, "command": wire});

        let request = decode_request(&wire.to_string()).unwrap();

        assert_eq!(request.command, command);
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
    }
}
