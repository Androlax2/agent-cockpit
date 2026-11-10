//! Network requests and grants on the wire (#674, #880): a host request keeps the shape every
//! owner client already reads, and a service request names its kind and service.

use cockpit_protocol::{
    Command, NetworkGrantView, NetworkRequestStateView, NetworkRequestView, NetworkSubjectView,
    NetworkTargetView, ShownText,
};
use serde_json::{Value, json};

fn request_json(kind: Value) -> Value {
    let mut request = json!({
        "id": 7,
        "subject": {"kind": "session", "id": "s1"},
        "project": "app",
        "target": "unknown.example.com:443",
        "state": "asking",
        "asked_at": 1000,
        "decision": null,
        "grant": null
    });
    request
        .as_object_mut()
        .unwrap()
        .extend(kind.as_object().unwrap().clone());
    request
}

fn request(asks: NetworkTargetView, target: &str) -> NetworkRequestView {
    NetworkRequestView {
        id: 7,
        subject: NetworkSubjectView::Session {
            id: "s1".to_owned(),
        },
        project: "app".to_owned(),
        asks,
        target: target.to_owned(),
        state: NetworkRequestStateView::Asking,
        asked_at: 1000,
        decision: None,
        grant: None,
        shown_reason: None,
    }
}

#[test]
fn a_host_request_keeps_its_kind_host_and_port_fields() {
    let host = request(
        NetworkTargetView::Host {
            host: "unknown.example.com".to_owned(),
            port: 443,
        },
        "unknown.example.com:443",
    );

    assert_eq!(
        serde_json::to_value(&host).unwrap(),
        request_json(json!({"kind": "host", "host": "unknown.example.com", "port": 443}))
    );
}

#[test]
fn a_service_request_names_its_service_and_its_record() {
    let record = "service figcraft listen 127.0.0.1:3845 target 127.0.0.1:3845";
    let service = request(
        NetworkTargetView::Service {
            service: "figcraft".to_owned(),
        },
        record,
    );

    let wire = serde_json::to_value(&service).unwrap();

    assert_eq!(wire["kind"], json!("service"));
    assert_eq!(wire["service"], json!("figcraft"));
    assert_eq!(wire["target"], json!(record));
    assert_eq!(
        serde_json::from_value::<NetworkRequestView>(wire).unwrap(),
        service
    );
}

#[test]
fn a_host_request_without_its_host_or_port_is_refused() {
    for partial in [
        json!({"kind": "host", "port": 443}),
        json!({"kind": "host", "host": "unknown.example.com"}),
    ] {
        let read = serde_json::from_value::<NetworkRequestView>(request_json(partial.clone()));
        assert!(read.is_err(), "{partial} was read");
    }
}

#[test]
fn a_grant_carries_its_kind_like_a_request() {
    let grant: NetworkGrantView = serde_json::from_value(json!({
        "id": 1,
        "project": "app",
        "kind": "host",
        "host": "github.com",
        "port": 443,
        "target": "github.com:443",
        "request": null,
        "granted": {"at": 1000, "through": "cli"},
        "revoked": null
    }))
    .unwrap();

    assert_eq!(
        grant.allows,
        NetworkTargetView::Host {
            host: "github.com".to_owned(),
            port: 443
        }
    );
}

#[test]
fn a_service_asked_ahead_carries_the_agents_reason() {
    let asked = NetworkRequestView {
        shown_reason: Some(ShownText {
            text: "read the design file".to_owned(),
            has_hidden_characters: false,
            non_ascii: Vec::new(),
            warnings: Vec::new(),
        }),
        ..request(
            NetworkTargetView::Service {
                service: "figcraft".to_owned(),
            },
            "service figcraft listen 127.0.0.1:3845 target 127.0.0.1:3845",
        )
    };

    let wire = serde_json::to_value(&asked).unwrap();

    assert_eq!(wire["shown_reason"]["text"], json!("read the design file"));
    assert_eq!(
        serde_json::from_value::<NetworkRequestView>(wire).unwrap(),
        asked
    );
}

#[test]
fn ask_service_names_the_service_and_the_reason() {
    let ask = Command::AskService {
        launch_token: None,
        service: "figcraft".to_owned(),
        reason: "read the design file".to_owned(),
    };

    assert_eq!(
        serde_json::to_value(&ask).unwrap(),
        json!({"kind": "ask-service", "service": "figcraft", "reason": "read the design file"})
    );
}
