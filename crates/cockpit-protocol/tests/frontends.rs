//! The wire form of the commands the TUI and the CLI add for #35: the provider list for the
//! new-session form, the manager session and showing a session in its viewer; and the owner's
//! desktop-session import (#307) and config reload (#354).
use cockpit_protocol::{AppliedSettingView, Command, ProviderView, Reply, decode_request};
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
fn a_manager_focus_and_its_reply_carry_only_their_kind() {
    let wire = json!({"id": 3, "command": {"kind": "manager-focus"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::ManagerFocus);
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
    assert_eq!(
        serde_json::to_value(Reply::ManagerFocused).unwrap(),
        json!({"kind": "manager-focused"})
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
