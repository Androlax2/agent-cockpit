mod common;
use common::sandbox::*;
use cockpit::costs;
use std::fs;
use std::io::Write;
use std::process::{Output, Stdio};

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

#[test]
fn billing_buckets_preserve_the_combined_total() {
    let sandbox = Sandbox::new();
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    let day = chrono::Local::now().date_naive().to_string();
    for key in ["api", "subscription", "legacy"] {
        archive.execute("insert into replies values (?, ?, ?, 'claude-opus-5-5', 'shop', 's', 0, 1000000, 0, 0, 0, 'standard', 20)",
            rusqlite::params![key, day, format!("{day}T10:00:00Z")]).unwrap();
    }
    archive.execute("insert into reply_billing values ('api', 'api'), ('subscription', 'subscription')", []).unwrap();
    let totals = costs::archived_billing_totals(&archive).unwrap();
    assert_eq!((totals.api.today, totals.subscription.today, totals.unknown.today), (20.0, 20.0, 20.0));
    assert_eq!(costs::archived_costs(&archive).unwrap().2, 60.0);
}

#[test]
fn late_billing_evidence_does_not_change_the_usage_total() {
    let sandbox = Sandbox::new();
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    let day = chrono::Local::now().date_naive().to_string();
    archive.execute("insert into replies values ('reply', ?, ?, 'claude-opus-5-5', 'shop', 's', 0, 1000000, 0, 0, 0, 'standard', 20)",
        rusqlite::params![day, format!("{day}T10:00:00Z")]).unwrap();
    assert_eq!(costs::archived_billing_totals(&archive).unwrap().unknown.today, 20.0);
    archive.execute("insert into reply_billing values ('reply', 'api')", []).unwrap();
    let totals = costs::archived_billing_totals(&archive).unwrap();
    assert_eq!((totals.api.today, totals.unknown.today), (20.0, 0.0));
    assert_eq!(costs::archived_costs(&archive).unwrap().2, 20.0);
}

#[test]
fn billing_evidence_does_not_make_an_unknown_model_free() {
    let sandbox = archived_replies();
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    archive.execute("insert into reply_billing values ('unknown', 'api')", []).unwrap();
    let (_, unpriced, _) = costs::archived_costs(&archive).unwrap();
    assert_eq!(unpriced.get("big-pickle"), Some(&72));
    assert_eq!(archive.query_row("select cost from replies where key = 'unknown'", [], |row| row.get::<_, Option<f64>>(0)).unwrap(), None);
}

fn record_billing(sandbox: &Sandbox, input: &str) -> Output {
    let mut child = sandbox.command("python3", &[]).arg(hook().with_file_name("billing-capture.py"))
        .arg("record-billing").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn repeated_and_conflicting_billing_evidence_is_imported_without_double_counting() {
    let sandbox = archived_replies();
    for mode in ["api", "api", "subscription", "api"] {
        let output = record_billing(&sandbox, &format!(r#"{{"key":"known","billing":"{mode}"}}"#));
        assert!(output.status.success(), "{}", stderr(&output));
        let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-sidebar"), &["costs"], &[]);
        assert!(output.status.success(), "{}", stderr(&output));
    }
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    assert_eq!(archive.query_row("select billing from reply_billing where key='known'", [], |row| row.get::<_, String>(0)).unwrap(), "unknown");
    assert_eq!(archive.query_row("select count(*) from reply_billing", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert!((costs::archived_costs(&archive).unwrap().2 - 52.21).abs() < 1e-9);
}

#[test]
fn billing_capture_rejects_invalid_or_sensitive_metadata() {
    let sandbox = Sandbox::new();
    for input in [r#"{"key":"reply","billing":"guess"}"#, r#"{"key":"","billing":"api"}"#,
        r#"{"key":"reply","billing":"api","api_key":"must-not-store"}"#, "not json"] {
        let output = record_billing(&sandbox, input);
        assert!(!output.status.success());
    }
    assert!(!sandbox.home.join(".cache/claude/billing").exists());
}

#[test]
fn a_billing_switch_only_attributes_replies_from_the_current_launch() {
    let mut sandbox = Sandbox::new();
    sandbox.env.insert("COCKPIT_OPENCODE_DB".into(), sandbox.home.join("missing.db").display().to_string());
    let directory = sandbox.home.join(".claude/projects/shop");
    fs::create_dir_all(&directory).unwrap();
    let transcript = directory.join("session.jsonl");
    let reply = |id: &str, minute: &str| format!(r#"{{"timestamp":"2026-10-23T10:{minute}:00Z","sessionId":"s","requestId":"r-{id}","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"output_tokens":1000000}},"content":"private conversation"}}}}"#);
    fs::write(&transcript, reply("old", "00") + "\n").unwrap();
    let capture = |mode: &str, since: &str| sandbox.command("python3", &[]).arg(hook().with_file_name("billing-capture.py"))
        .args(["record-transcript-billing", mode, since, transcript.to_str().unwrap()]).output().unwrap();
    assert!(capture("subscription", "2026-10-23T09:59:00Z").status.success());
    fs::write(&transcript, reply("old", "00") + "\n" + &reply("new", "02") + "\n").unwrap();
    for _ in 0..2 { assert!(capture("api", "2026-10-23T10:01:00Z").status.success()); }
    let report = sandbox.run(env!("CARGO_BIN_EXE_cockpit-sidebar"), &["costs"], &[]);
    assert!(report.status.success(), "{}", stderr(&report));
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    for (key, mode) in [("old:r-old", "subscription"), ("new:r-new", "api")] {
        assert_eq!(archive.query_row("select billing from reply_billing where key=?", [key], |row| row.get::<_, String>(0)).unwrap(), mode);
    }
    for entry in fs::read_dir(sandbox.home.join(".cache/claude/billing/processed")).unwrap() {
        let metadata: serde_json::Value = serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap();
        assert_eq!(metadata.as_object().unwrap().len(), 2);
        assert!(metadata.get("key").is_some() && metadata.get("billing").is_some());
    }
    assert_eq!(costs::archived_costs(&archive).unwrap().2, 40.0);
}

#[test]
fn symlinked_billing_metadata_is_not_imported() {
    let sandbox = archived_replies();
    let directory = sandbox.home.join(".cache/claude/billing");
    fs::create_dir_all(&directory).unwrap();
    let target = sandbox.root.join("elsewhere.json");
    fs::write(&target, r#"{"key":"known","billing":"api"}"#).unwrap();
    std::os::unix::fs::symlink(target, directory.join("planted.json")).unwrap();
    let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-sidebar"), &["costs"], &[]);
    assert!(stderr(&output).contains("billing"), "{}", stderr(&output));
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    assert_eq!(archive.query_row("select count(*) from reply_billing", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn corrupt_metadata_does_not_block_valid_conflicting_evidence() {
    let sandbox = archived_replies();
    assert!(record_billing(&sandbox, r#"{"key":"known","billing":"api"}"#).status.success());
    assert!(sandbox.run(env!("CARGO_BIN_EXE_cockpit-sidebar"), &["costs"], &[]).status.success());
    assert!(record_billing(&sandbox, r#"{"key":"known","billing":"subscription"}"#).status.success());
    fs::write(sandbox.home.join(".cache/claude/billing/corrupt.json"), "not json").unwrap();
    let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-sidebar"), &["costs"], &[]);
    assert!(stderr(&output).contains("billing metadata"), "{}", stderr(&output));
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    assert_eq!(archive.query_row("select billing from reply_billing where key='known'", [], |row| row.get::<_, String>(0)).unwrap(), "unknown");
}

fn pending_billing(sandbox: &Sandbox, count: usize) -> std::path::PathBuf {
    let directory = sandbox.home.join(".cache/claude/billing");
    fs::create_dir_all(&directory).unwrap();
    for index in 0..count {
        fs::write(directory.join(format!("{index}.json")), format!(r#"{{"key":"reply-{index}","billing":"api"}}"#)).unwrap();
    }
    directory
}

fn refresh_billing(sandbox: &Sandbox) -> Output {
    sandbox.run(env!("CARGO_BIN_EXE_cockpit-sidebar"), &["costs"], &[])
}

fn billing_count(sandbox: &Sandbox) -> usize {
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    archive.query_row("select count(*) from reply_billing", [], |row| row.get::<_, i64>(0)).unwrap() as usize
}

#[test]
fn billing_backlogs_are_imported_in_bounded_batches_without_losing_evidence() {
    let sandbox = archived_replies();
    let directory = pending_billing(&sandbox, 600);
    let mut previous = 0;
    for _ in 0..4 {
        let output = refresh_billing(&sandbox);
        assert!(output.status.success(), "{}", stderr(&output));
        let count = billing_count(&sandbox);
        assert!(count - previous <= 256, "imported {} in one refresh", count - previous);
        previous = count;
    }
    assert_eq!(previous, 600);
    assert_eq!(fs::read_dir(directory.join("processed")).unwrap().count(), 600);
    assert_eq!(fs::read_dir(&directory).unwrap().filter(|entry| entry.as_ref().unwrap().path().extension().is_some_and(|extension| extension == "json")).count(), 0);
}

#[test]
fn normal_refreshes_do_not_visit_retained_history() {
    let sandbox = archived_replies();
    let directory = pending_billing(&sandbox, 1);
    refresh_billing(&sandbox);
    let processed = directory.join("processed");
    assert!(processed.is_dir());
    for index in 0..5000 {
        fs::write(processed.join(format!("invalid-{index}.json")), "not json").unwrap();
    }
    let output = refresh_billing(&sandbox);
    assert!(!stderr(&output).contains("billing metadata"), "{}", stderr(&output));
    assert_eq!(billing_count(&sandbox), 1);
}

#[test]
fn a_rebuilt_archive_replays_retained_evidence_and_preserves_conflicts() {
    let sandbox = archived_replies();
    for mode in ["api", "subscription", "api"] {
        assert!(record_billing(&sandbox, &format!(r#"{{"key":"known","billing":"{mode}"}}"#)).status.success());
        refresh_billing(&sandbox);
    }
    let directory = sandbox.home.join(".cache/claude/billing/processed");
    assert!(directory.is_dir());
    fs::rename(sandbox.state.join("cockpit/usage.sqlite"), sandbox.state.join("cockpit/old-usage.sqlite")).unwrap();
    refresh_billing(&sandbox);
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    assert_eq!(archive.query_row("select billing from reply_billing where key='known'", [], |row| row.get::<_, String>(0)).unwrap(), "unknown");
    assert_eq!(fs::read_dir(directory).unwrap().count(), 2);
}

#[test]
fn recaptured_evidence_is_idempotent_on_disk_and_in_the_archive() {
    let sandbox = archived_replies();
    for _ in 0..3 {
        assert!(record_billing(&sandbox, r#"{"key":"known","billing":"api"}"#).status.success());
        refresh_billing(&sandbox);
    }
    assert_eq!(billing_count(&sandbox), 1);
    assert_eq!(fs::read_dir(sandbox.home.join(".cache/claude/billing/processed")).unwrap().count(), 1);
}

#[test]
fn interrupted_processing_is_retried_before_new_pending_files() {
    let sandbox = archived_replies();
    let directory = pending_billing(&sandbox, 600);
    let processing = directory.join("processing");
    fs::create_dir(&processing).unwrap();
    fs::write(processing.join("interrupted.json"), r#"{"key":"known","billing":"subscription"}"#).unwrap();
    refresh_billing(&sandbox);
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    assert_eq!(archive.query_row("select billing from reply_billing where key='known'", [], |row| row.get::<_, String>(0)).unwrap(), "subscription");
    assert_eq!(fs::read_dir(processing).unwrap().count(), 0);
    assert!(billing_count(&sandbox) <= 256);
}

#[test]
fn failed_archive_commits_leave_processing_evidence_available_for_retry() {
    let sandbox = archived_replies();
    let directory = pending_billing(&sandbox, 1);
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    archive.execute_batch("create trigger fail_billing before insert on reply_billing begin select raise(abort, 'injected failure'); end;").unwrap();
    let output = refresh_billing(&sandbox);
    assert!(stderr(&output).contains("injected failure"), "{}", stderr(&output));
    assert_eq!(fs::read_dir(directory.join("processing")).unwrap().count(), 1);
    assert_eq!(fs::read_dir(directory.join("processed")).unwrap().count(), 0);
    archive.execute_batch("drop trigger fail_billing").unwrap();
    refresh_billing(&sandbox);
    assert_eq!(billing_count(&sandbox), 1);
    assert_eq!(fs::read_dir(directory.join("processed")).unwrap().count(), 1);
}

#[test]
fn interruption_after_commit_replays_processing_without_clearing_a_conflict() {
    let sandbox = archived_replies();
    let directory = pending_billing(&sandbox, 0);
    let processing = directory.join("processing");
    fs::create_dir(&processing).unwrap();
    fs::write(processing.join("committed.json"), r#"{"key":"known","billing":"api"}"#).unwrap();
    let archive = costs::open_archive(&sandbox.state.join("cockpit/usage.sqlite")).unwrap();
    archive.execute("insert into reply_billing values ('known', 'unknown')", []).unwrap();
    refresh_billing(&sandbox);
    assert_eq!(archive.query_row("select billing from reply_billing where key='known'", [], |row| row.get::<_, String>(0)).unwrap(), "unknown");
    assert_eq!(fs::read_dir(processing).unwrap().count(), 0);
    assert_eq!(fs::read_dir(directory.join("processed")).unwrap().count(), 1);
}

#[test]
fn malformed_metadata_is_reported_and_retained_without_starving_the_queue() {
    let sandbox = archived_replies();
    let directory = pending_billing(&sandbox, 1);
    for index in 0..300 {
        fs::write(directory.join(format!("bad-{index}.json")), "not json").unwrap();
    }
    let mut warnings = false;
    for _ in 0..3 {
        warnings |= stderr(&refresh_billing(&sandbox)).contains("billing metadata");
    }
    assert!(warnings);
    assert_eq!(billing_count(&sandbox), 1);
    assert_eq!(fs::read_dir(directory.join("rejected")).unwrap().count(), 300);
    assert!(!stderr(&refresh_billing(&sandbox)).contains("billing metadata"));
}

#[test]
fn symlinked_spool_directories_are_rejected_without_touching_their_targets() {
    for name in ["processing", "processed", "rejected"] {
        let sandbox = archived_replies();
        let directory = pending_billing(&sandbox, 1);
        let target = sandbox.root.join("outside");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, directory.join(name)).unwrap();
        let output = refresh_billing(&sandbox);
        assert!(stderr(&output).contains("billing metadata"), "{name}: {}", stderr(&output));
        assert_eq!(billing_count(&sandbox), 0);
        assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    }
}

#[test]
fn fifo_metadata_is_quarantined_without_blocking_valid_evidence() {
    use std::os::unix::fs::FileTypeExt;
    let sandbox = archived_replies();
    let directory = pending_billing(&sandbox, 1);
    let fifo = std::ffi::CString::new(directory.join("pipe.json").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let output = refresh_billing(&sandbox);
    assert!(stderr(&output).contains("billing metadata"));
    assert_eq!(billing_count(&sandbox), 1);
    let rejected = fs::read_dir(directory.join("rejected")).unwrap().next().unwrap().unwrap().path();
    assert!(fs::symlink_metadata(rejected).unwrap().file_type().is_fifo());
}

fn digest_sandbox() -> Sandbox {
    let sandbox = archived_replies();
    let reviews = sandbox.home.join("reviews.json");
    fs::write(&reviews, "[]").unwrap();
    let mut sandbox = sandbox;
    sandbox.env.insert("COCKPIT_REVIEWS_FIXTURE".into(), reviews.display().to_string());
    sandbox
}

fn digest_text(sandbox: &Sandbox) -> String {
    let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-sidebar"), &["digest"], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let colours = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    colours.replace_all(&String::from_utf8(output.stdout).unwrap(), "").to_string()
}

fn now_ms() -> i64 { chrono::Utc::now().timestamp_millis() }

fn team_database_with_answer(sandbox: &Sandbox, created_at: i64) {
    let connection = cockpit::teams::store::open(&sandbox.state.join("claude-audit/teams.sqlite")).unwrap();
    connection.execute("INSERT INTO chief_stops(id,session_id,runtime,boundary,project,task_id,bundle,state,created_at) VALUES ('stop-a','ses_abcdefghijkl','runtime-a','b','path:/shop','task-a','{}','answered',?1)", [created_at]).unwrap();
    connection.execute("INSERT INTO chief_answers(id,stop_id,session_id,question,answer,reason,source,delivery,created_at) VALUES ('answer-a','stop-a','ses_abcdefghijkl','Which option?','Use option B.','Because.','chief','received',?1)", [created_at]).unwrap();
}

#[test]
fn a_missing_team_database_leaves_the_digest_intact() {
    let sandbox = digest_sandbox();
    let digest = digest_text(&sandbox);
    assert!(digest.contains("Chief: unavailable ("), "{digest}");
    assert!(digest.contains("Today so far"), "{digest}");
    assert!(!sandbox.state.join("claude-audit/teams.sqlite").exists(), "reading the digest must not create the team database");
    assert!(!sandbox.state.join("cockpit/chief-digest-at").exists(), "answers nobody saw must not move the since-time");
}

#[test]
fn a_locked_team_database_leaves_the_digest_intact() {
    let sandbox = digest_sandbox();
    team_database_with_answer(&sandbox, now_ms() - 60_000);
    let mut blocker = rusqlite::Connection::open(sandbox.state.join("claude-audit/teams.sqlite")).unwrap();
    blocker.execute_batch("PRAGMA locking_mode=EXCLUSIVE;").unwrap();
    let transaction = blocker.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive).unwrap();
    transaction.execute("UPDATE chief_answers SET reason='locked'", []).unwrap();
    let digest = digest_text(&sandbox);
    assert!(digest.contains("Chief: unavailable ("), "{digest}");
    assert!(digest.contains("Today so far"), "{digest}");
    assert!(!sandbox.state.join("cockpit/chief-digest-at").exists());
}

#[test]
fn printing_the_digest_moves_the_since_time() {
    let sandbox = digest_sandbox();
    team_database_with_answer(&sandbox, now_ms() - 3_600_000);
    let first = digest_text(&sandbox);
    assert!(first.contains("Chief answered 1, escalated 0, overruled 0 since"), "{first}");
    assert!(first.contains("ses_…ghijkl · Which option? → Use option B."), "{first}");
    let since_file = sandbox.state.join("cockpit/chief-digest-at");
    let written: i64 = fs::read_to_string(&since_file).unwrap().trim().parse().unwrap();
    assert!((now_ms() - written).abs() < 60_000, "{written}");
    let second = digest_text(&sandbox);
    assert!(second.contains("Chief: nothing answered since"), "{second}");
    assert!(!second.contains("Which option?"), "{second}");
}

fn team_database_with_habit(sandbox: &Sandbox, created_at: i64) {
    let connection = cockpit::teams::store::open(&sandbox.state.join("claude-audit/teams.sqlite")).unwrap();
    connection.execute("INSERT INTO chief_habits(id,text,project,origin_project,source,source_kind,source_id,state,created_at,changed_at)
        VALUES ('habit-a','Always run the tests first.','path:/home/theo/IdeaProjects/shop','path:/home/theo/IdeaProjects/shop','overrule','overrule','overrule-a','active',?1,?1)", [created_at]).unwrap();
}

fn habits_seen_at(sandbox: &Sandbox) -> Option<String> {
    fs::read_to_string(sandbox.state.join("cockpit/chief-habits-seen-at")).ok()
}

#[test]
fn a_printed_digest_clears_new_habits() {
    let sandbox = digest_sandbox();
    let created_at = now_ms() - 3_600_000;
    team_database_with_habit(&sandbox, created_at);
    let started = now_ms();
    let first = digest_text(&sandbox);
    assert!(first.contains("  New habits: 1\n    Always run the tests first. · shop"), "{first}");
    let seen: i64 = habits_seen_at(&sandbox).expect("a printed habits section marks them seen").trim().parse().unwrap();
    assert!(seen >= started && seen <= now_ms(), "the time the digest read them: {seen}");
    let second = digest_text(&sandbox);
    assert!(!second.contains("New habits"), "{second}");
}

#[test]
fn a_digest_that_lists_some_new_habits_keeps_the_rest_new() {
    let sandbox = digest_sandbox();
    let connection = cockpit::teams::store::open(&sandbox.state.join("claude-audit/teams.sqlite")).unwrap();
    let base = now_ms() - 3_600_000;
    for index in 0..5_i64 {
        connection.execute("INSERT INTO chief_habits(id,text,project,origin_project,source,source_kind,source_id,state,created_at,changed_at)
            VALUES (?1,?2,NULL,NULL,'overrule','overrule','overrule-a','active',?3,?3)", rusqlite::params![format!("habit-{index}"), format!("Habit number {index}."), base + index * 1_000]).unwrap();
    }
    let first = digest_text(&sandbox);
    assert!(first.contains("New habits: 5"), "{first}");
    let seen: i64 = habits_seen_at(&sandbox).expect("a printed habits section marks the listed ones seen").trim().parse().unwrap();
    assert_eq!(seen, base + 2 * 1_000 - 1, "just before the oldest listed habit, so the two unlisted ones stay new");
}

#[test]
fn a_digest_listing_fewer_habits_never_moves_the_seen_time_back() {
    let sandbox = digest_sandbox();
    let connection = cockpit::teams::store::open(&sandbox.state.join("claude-audit/teams.sqlite")).unwrap();
    let base = now_ms() - 3_600_000;
    for index in 0..4_i64 {
        connection.execute("INSERT INTO chief_habits(id,text,project,origin_project,source,source_kind,source_id,state,created_at,changed_at)
            VALUES (?1,?2,NULL,NULL,'overrule','overrule','overrule-a','active',?3,?3)", rusqlite::params![format!("habit-{index}"), format!("Habit number {index}."), base + index * 1_000]).unwrap();
    }
    let later = base + 3_500;
    fs::create_dir_all(sandbox.state.join("cockpit")).unwrap();
    fs::write(sandbox.state.join("cockpit/chief-habits-seen-at"), later.to_string()).unwrap();
    digest_text(&sandbox);
    assert_eq!(habits_seen_at(&sandbox).unwrap().trim(), later.to_string());
}

#[test]
fn a_digest_without_new_habits_leaves_them_unseen() {
    let sandbox = digest_sandbox();
    team_database_with_answer(&sandbox, now_ms() - 3_600_000);
    digest_text(&sandbox);
    assert_eq!(habits_seen_at(&sandbox), None);
}

#[test]
fn an_unavailable_database_keeps_habits_new() {
    let sandbox = digest_sandbox();
    team_database_with_habit(&sandbox, now_ms() - 60_000);
    let mut blocker = rusqlite::Connection::open(sandbox.state.join("claude-audit/teams.sqlite")).unwrap();
    blocker.execute_batch("PRAGMA locking_mode=EXCLUSIVE;").unwrap();
    let transaction = blocker.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive).unwrap();
    transaction.execute("UPDATE chief_habits SET text='locked'", []).unwrap();
    let digest = digest_text(&sandbox);
    assert!(digest.contains("Chief: unavailable ("), "{digest}");
    assert!(!digest.contains("New habits"), "{digest}");
    assert_eq!(habits_seen_at(&sandbox), None);
}

#[test]
fn an_unreadable_since_time_falls_back_to_the_last_24_hours() {
    let sandbox = digest_sandbox();
    team_database_with_answer(&sandbox, now_ms() - 3_600_000);
    fs::create_dir_all(sandbox.state.join("cockpit")).unwrap();
    fs::write(sandbox.state.join("cockpit/chief-digest-at"), "yesterday-ish").unwrap();
    let digest = digest_text(&sandbox);
    assert!(digest.contains("Chief answered 1,"), "{digest}");
    let written: i64 = fs::read_to_string(sandbox.state.join("cockpit/chief-digest-at")).unwrap().trim().parse().unwrap();
    assert!((now_ms() - written).abs() < 60_000, "{written}");
}
