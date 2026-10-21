mod common;
use common::sandbox::*;
use cockpit::costs;

fn archived_replies() -> Sandbox {
    let mut sandbox = Sandbox::new();
    sandbox.env.insert("COCKPIT_TRANSCRIPTS".into(), sandbox.home.join("missing-transcripts").display().to_string());
    sandbox.env.insert("COCKPIT_OPENCODE_DB".into(), sandbox.home.join("missing-opencode.db").display().to_string());
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    archive.execute_batch("insert into replies values
        ('missing', '2026-10-20', '2026-10-20T10:00:00Z', 'gpt-6.1-sol', 'shop', 's', 100000, 1000000, 100000, 0, 0, 'standard', null),
        ('known', '2026-10-20', '2026-10-20T10:01:00Z', 'gpt-6.1-sol', 'shop', 's', 100000, 1000000, 100000, 0, 0, 'standard', 42),
        ('unknown', '2026-10-20', '2026-10-20T10:02:00Z', 'big-pickle', 'shop', 's', 0, 72, 0, 0, 0, 'standard', null);").unwrap();
    sandbox
}

#[test]
fn the_cost_report_backfills_saved_tokens_without_source_history() {
    let sandbox = archived_replies();
    for _ in 0..2 {
        let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-sidebar"), &["costs"], &[]);
        assert!(output.status.success(), "{}", stderr(&output));
        let report = String::from_utf8(output.stdout).unwrap();
        assert!(report.contains("$52.21"), "{report}");
    }
    let archive = rusqlite::Connection::open(sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    let (_, unpriced, total) = costs::archived_costs(&archive).unwrap();
    assert!((total - 52.21).abs() < 1e-9);
    assert_eq!(unpriced, std::collections::HashMap::from([("big-pickle".into(), 72)]));
}

#[test]
fn an_unknown_price_is_not_reported_as_free() {
    let sandbox = archived_replies();
    let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-sidebar"), &["costs"], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report = String::from_utf8(output.stdout).unwrap();
    let model = report.lines().find(|line| line.trim_start().starts_with("big-pickle")).unwrap();
    assert!(model.contains("unpriced"), "{model}");
    assert!(!model.contains("$0.00"), "{model}");
}
