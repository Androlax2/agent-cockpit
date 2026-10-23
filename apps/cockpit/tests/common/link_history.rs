use cockpit::teams::{HostAction, LinkProposal, VerifiedSession, links, store};
use rusqlite::Connection;

pub fn populate(connection: &mut Connection, live: &[VerifiedSession], proposal: &LinkProposal) {
    for index in 0..80 {
        let now = (index + 1) * 60_001;
        let proposal = LinkProposal { submission_key: format!("history-{index}"), purpose: "🦀".repeat(1000), ..proposal.clone() };
        let pending = links::propose(connection, &live[0], &proposal, live, now).unwrap().result.unwrap();
        let proposal_id = pending["proposal_id"].as_str().unwrap().to_string();
        if index % 2 == 0 {
            let hash = connection.query_row("SELECT mapping_hash FROM link_proposals WHERE id=?1", [&proposal_id], |row| row.get(0)).unwrap();
            let approved = store::control(connection, &HostAction::LinkApprove { request_id:format!("approve-{index}"), proposal_id,
                expected_revision:1, mapping_hash:hash }, live, now).unwrap().result.unwrap();
            store::control(connection, &HostAction::LinkRevoke { request_id:format!("revoke-{index}"),
                link_id:approved["link_id"].as_str().unwrap().into() }, live, now).unwrap();
        } else {
            store::control(connection, &HostAction::LinkReject { request_id:format!("reject-{index}"), proposal_id,
                expected_revision:1 }, live, now).unwrap();
        }
    }
    let historical_bytes: i64 = connection.query_row("SELECT sum(length(CAST(purpose AS BLOB))) FROM link_proposals WHERE submission_key LIKE 'history-%'",
        [], |row| row.get(0)).unwrap();
    assert_eq!(historical_bytes, 320_000);
}

/// Tune a valid purpose so the old Context budget admits exactly 65,536 JSON bytes, before framing.
pub fn fill_reply_boundary(connection: &mut Connection, live: &[VerifiedSession], proposal: &LinkProposal) {
    use cockpit::teams::{TeamReply, client};
    use serde_json::json;
    let activate = |connection: &mut Connection, proposal: &LinkProposal| {
        let pending = links::propose(connection, &live[0], proposal, live, 6_000_010).unwrap().result.unwrap();
        let proposal_id = pending["proposal_id"].as_str().unwrap().to_string();
        let hash = connection.query_row("SELECT mapping_hash FROM link_proposals WHERE id=?1", [&proposal_id], |row| row.get(0)).unwrap();
        store::control(connection, &HostAction::LinkApprove { request_id:"boundary-approval".into(), proposal_id, expected_revision:1,
            mapping_hash:hash }, live, 6_000_011).unwrap().result.unwrap()["link_id"].as_str().unwrap().to_string()
    };
    let path = std::env::temp_dir().join(format!("context-boundary-{}.sqlite", cockpit::util::token_hex(6)));
    connection.execute("VACUUM INTO ?1", [path.to_str().unwrap()]).unwrap();
    let mut probe = Connection::open(&path).unwrap();
    let mut proposal = LinkProposal { submission_key:"boundary".into(), purpose:"x".into(), ..proposal.clone() };
    let link = activate(&mut probe, &proposal);
    let mut context = store::context(&probe, &live[0]).unwrap();
    let current_bytes = serde_json::to_vec(&TeamReply { request_id:"context".into(), result:Ok(context.clone()) }).unwrap().len();
    let purpose_bytes = client::AGENT_REPLY_MAX - current_bytes + 1;
    assert!((1..=4000).contains(&purpose_bytes), "boundary purpose needs {purpose_bytes} bytes");
    proposal.purpose = "\u{1f980}".repeat(purpose_bytes / 4) + match purpose_bytes % 4 { 0 => "", 1 => "x", 2 => "é", _ => "€" };
    let entry = context["links"]["approved"].as_array_mut().unwrap().iter_mut().find(|entry| entry["link_id"] == link).unwrap();
    entry["purpose"] = json!(proposal.purpose);
    assert_eq!(serde_json::to_vec(&TeamReply { request_id:"context".into(), result:Ok(context) }).unwrap().len(), 65_536);
    drop(probe);
    std::fs::remove_file(path).unwrap();
    activate(connection, &proposal);
}
