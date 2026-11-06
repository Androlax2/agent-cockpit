//! `version-report` (report 13, #64): the request names nothing, the reply carries the daemon's
//! release, its schema and each live session's bundle.
use cockpit_protocol::{
    Command, LaunchBundleView, Reply, SchemaOwnerView, SessionBundleView, SessionStateView,
    VersionReportView, decode_request,
};
use serde_json::json;

#[test]
fn the_request_round_trips_through_its_wire_form() {
    let wire = json!({"id": 1, "command": {"kind": "version-report"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::VersionReport);
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn the_reply_names_release_schema_and_each_sessions_bundle() {
    let reply = Reply::VersionReport {
        report: Box::new(VersionReportView {
            release: "0.1.0-alpha.5".to_owned(),
            source_revision: Some("0123abcd".to_owned()),
            instruction_manifest: "m-0123456789abcdef".to_owned(),
            protocol_versions: vec![1],
            schema: vec![SchemaOwnerView {
                owner: "sessions".to_owned(),
                applied: 13,
                known: 13,
            }],
            sessions: vec![
                SessionBundleView {
                    session: "s1".to_owned(),
                    state: SessionStateView::Running,
                    bundle: Some(LaunchBundleView {
                        release: "0.1.0-alpha.4".to_owned(),
                        instruction_manifest: "m-aaaaaaaaaaaaaaaa".to_owned(),
                    }),
                },
                SessionBundleView {
                    session: "s2".to_owned(),
                    state: SessionStateView::Asleep,
                    bundle: None,
                },
            ],
        }),
    };

    let wire = serde_json::to_value(&reply).unwrap();

    assert_eq!(wire["kind"], "version-report");
    assert_eq!(
        wire["report"]["sessions"][0]["bundle"]["release"],
        "0.1.0-alpha.4"
    );
    assert_eq!(wire["report"]["sessions"][1]["bundle"], json!(null));
    assert_eq!(serde_json::from_value::<Reply>(wire).unwrap(), reply);
}
