//! The wire form of the commands the TUI and the CLI add for #35: the provider list for the
//! new-session form, the manager session and showing a session in its viewer; and the owner's
//! desktop-session import (#307) and config reload (#354); and the sandbox a session ran in
//! or would get (#61).
use cockpit_protocol::{
    AppliedSettingView, Command, ManagerPaneView, ProviderView, Reply, SandboxEnforcementView,
    SandboxEvidenceView, SandboxRuleView, decode_request,
};
use serde_json::json;

#[test]
fn a_manager_open_carries_the_words_of_both_panes() {
    let wire = json!({
        "id": 1,
        "command": {
            "kind": "manager-open",
            "dir": "/home/owner",
            "column": ["/usr/bin/cockpit-tui", "--column"],
            "viewer": ["/usr/bin/cockpit-tui", "--viewer"]
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::ManagerOpen {
            dir: "/home/owner".to_owned(),
            column: vec!["/usr/bin/cockpit-tui".to_owned(), "--column".to_owned()],
            viewer: vec!["/usr/bin/cockpit-tui".to_owned(), "--viewer".to_owned()],
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_session_show_and_a_provider_list_round_trip() {
    for (wire, command) in [
        (
            json!({"id": 2, "command": {"kind": "session-show", "session": "s0123456789ab"}}),
            Command::SessionShow {
                session: "s0123456789ab".to_owned(),
            },
        ),
        (
            json!({"id": 2, "command": {"kind": "provider-list"}}),
            Command::ProviderList,
        ),
    ] {
        let request = decode_request(&wire.to_string()).unwrap();
        assert_eq!(request.command, command);
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
    }
}

#[test]
fn the_providers_and_manager_replies_name_their_fields_on_the_wire() {
    let providers = Reply::Providers {
        providers: vec![ProviderView {
            name: "claude".to_owned(),
            resume: true,
            events: true,
        }],
    };
    let manager = Reply::Manager {
        attach: vec!["tmux".to_owned(), "-L".to_owned(), "cockpit".to_owned()],
        created: true,
    };

    assert_eq!(
        (
            serde_json::to_value(providers).unwrap(),
            serde_json::to_value(manager).unwrap()
        ),
        (
            json!({"kind": "providers", "providers": [{"name": "claude", "resume": true, "events": true}]}),
            json!({"kind": "manager", "attach": ["tmux", "-L", "cockpit"], "created": true})
        )
    );
}

#[test]
fn a_manager_focus_names_the_pane_that_takes_the_keys() {
    let wire = json!({"id": 3, "command": {"kind": "manager-focus", "pane": "viewer"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::ManagerFocus {
            pane: ManagerPaneView::Viewer
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_manager_focus_without_its_pane_gives_the_column_the_keys_as_before_345() {
    let wire = json!({"id": 3, "command": {"kind": "manager-focus"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::ManagerFocus {
            pane: ManagerPaneView::Column
        }
    );
}

#[test]
fn manager_focused_says_whether_the_viewer_had_the_keys() {
    assert_eq!(
        serde_json::to_value(Reply::ManagerFocused {
            took_from_viewer: true
        })
        .unwrap(),
        json!({"kind": "manager-focused", "took_from_viewer": true})
    );
}

#[test]
fn a_desktop_session_import_carries_only_the_set_variables_and_its_reply_says_withheld() {
    let wire = json!({
        "id": 3,
        "command": {
            "kind": "desktop-session-import",
            "variables": {"DISPLAY": ":1", "WAYLAND_DISPLAY": "wayland-1"}
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::DesktopSessionImport {
            variables: [
                ("DISPLAY".to_owned(), ":1".to_owned()),
                ("WAYLAND_DISPLAY".to_owned(), "wayland-1".to_owned()),
            ]
            .into(),
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
    assert_eq!(
        serde_json::to_value(Reply::DesktopSessionImported { withheld: true }).unwrap(),
        json!({"kind": "desktop-session-imported", "withheld": true})
    );
}

#[test]
fn a_config_reload_carries_nothing_and_its_reply_lists_each_setting_with_its_old_value() {
    let wire = json!({"id": 4, "command": {"kind": "config-reload"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::ConfigReload);
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
    assert_eq!(
        serde_json::to_value(Reply::ConfigReloaded {
            settings: vec![AppliedSettingView {
                key: "agents.desktop_session".to_owned(),
                value: "\"withheld\"".to_owned(),
                before: "\"shared\"".to_owned(),
            }],
        })
        .unwrap(),
        json!({
            "kind": "config-reloaded",
            "settings": [{
                "key": "agents.desktop_session",
                "value": "\"withheld\"",
                "before": "\"shared\""
            }]
        })
    );
}

#[test]
fn a_session_sandbox_and_a_sandbox_show_round_trip() {
    for (wire, command) in [
        (
            json!({"id": 5, "command": {"kind": "session-sandbox", "session": "s0123456789ab"}}),
            Command::SessionSandbox {
                session: "s0123456789ab".to_owned(),
            },
        ),
        (
            json!({"id": 6, "command": {"kind": "sandbox-show", "dir": "/p/app"}}),
            Command::SandboxShow {
                dir: "/p/app".to_owned(),
                agent: None,
            },
        ),
        (
            json!({"id": 7, "command": {"kind": "sandbox-show", "dir": "/p/app", "agent": "codex"}}),
            Command::SandboxShow {
                dir: "/p/app".to_owned(),
                agent: Some("codex".to_owned()),
            },
        ),
    ] {
        let request = decode_request(&wire.to_string()).unwrap();

        assert_eq!(request.command, command);
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
    }
}

fn sandboxed_evidence() -> SandboxEvidenceView {
    SandboxEvidenceView {
        enforcement: SandboxEnforcementView::Sandboxed {
            backend: "greywall".to_owned(),
            engine: "greywall 0.3.7 (7c06d51), greyproxy 0.4.5 (cdd5cde)".to_owned(),
            client_identity: "cockpit-s1-0a1b2c3d4e5f".to_owned(),
        },
        platform: "linux".to_owned(),
        rules: vec![SandboxRuleView {
            rule: "read everything else: denied".to_owned(),
            enforced_by: "greywall bubblewrap+Landlock (defaultDenyRead)".to_owned(),
        }],
        observed: vec!["landlock: yes".to_owned()],
    }
}

#[test]
fn sandbox_evidence_travels_with_its_enforcement_tagged() {
    assert_eq!(
        serde_json::to_value(Reply::SessionSandbox {
            evidence: Some(Box::new(sandboxed_evidence())),
        })
        .unwrap(),
        json!({
            "kind": "session-sandbox",
            "evidence": {
                "enforcement": {
                    "kind": "sandboxed",
                    "backend": "greywall",
                    "engine": "greywall 0.3.7 (7c06d51), greyproxy 0.4.5 (cdd5cde)",
                    "client_identity": "cockpit-s1-0a1b2c3d4e5f"
                },
                "platform": "linux",
                "rules": [{
                    "rule": "read everything else: denied",
                    "enforced_by": "greywall bubblewrap+Landlock (defaultDenyRead)"
                }],
                "observed": ["landlock: yes"]
            }
        })
    );
    assert_eq!(
        serde_json::to_value(Reply::SessionSandbox { evidence: None }).unwrap(),
        json!({"kind": "session-sandbox", "evidence": null})
    );
}

#[test]
fn sandbox_evidence_prints_one_line_per_fact_in_a_fixed_order() {
    assert_eq!(
        sandboxed_evidence().lines(),
        [
            "sandbox: enforced by greywall on linux",
            "engine: greywall 0.3.7 (7c06d51), greyproxy 0.4.5 (cdd5cde)",
            "proxy client: cockpit-s1-0a1b2c3d4e5f",
            "rule: read everything else: denied <- greywall bubblewrap+Landlock (defaultDenyRead)",
            "observed: landlock: yes",
        ]
    );
}

#[test]
fn unsandboxed_evidence_says_so_first_with_the_reason() {
    let evidence = SandboxEvidenceView {
        enforcement: SandboxEnforcementView::Unsandboxed {
            reason: "the owner set agents.sandbox = \"off\"".to_owned(),
        },
        platform: "macos".to_owned(),
        rules: Vec::new(),
        observed: Vec::new(),
    };

    assert_eq!(
        evidence.lines(),
        ["sandbox: NONE on macos, unsandboxed: the owner set agents.sandbox = \"off\""]
    );
}
