//! Exact gate semantics. Provenance is established by host/QA intake, never by result text.

use super::{Evidence, GateKind, GateRequirement};
use crate::teams::{ErrorCode, TeamError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    commit: String,
    #[serde(default)]
    command_fingerprint: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

fn invalid() -> TeamError { TeamError::new(ErrorCode::Invalid, "evidence does not match the exact approved gate") }

pub fn validate_requirement(requirement: &GateRequirement) -> Result<(), TeamError> {
    let expected: Expected = serde_json::from_value(requirement.expected.clone())?;
    let full_commit = matches!(expected.commit.len(), 40 | 64) && expected.commit.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    let supported = match (requirement.kind, requirement.source.as_str()) {
        (_, "host_attestation") | (GateKind::Qa, "qa") | (GateKind::Apply, "reviewed_apply") => expected.command_fingerprint.is_none() && expected.cwd.is_none(),
        (GateKind::Tests, "host_run") => expected.command_fingerprint.as_ref().is_some_and(|fingerprint| fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
            && expected.cwd.as_ref().is_some_and(|cwd| cwd.starts_with('/') && !cwd.chars().any(char::is_control)),
        _ => false,
    };
    if !full_commit || !supported { return Err(invalid()); }
    Ok(())
}

pub fn validate(requirement: &GateRequirement, evidence: &Evidence) -> Result<(), TeamError> {
    validate_requirement(requirement)?;
    evidence.validate()?;
    let expected: Expected = serde_json::from_value(requirement.expected.clone())?;
    if evidence.kind != requirement.kind || evidence.source != requirement.source || evidence.commit != expected.commit {
        return Err(invalid());
    }
    let result = &evidence.result;
    let passed = match evidence.source.as_str() {
        "host_attestation" => result["passed"] == true && result["note"].as_str().is_some_and(|note| !note.trim().is_empty()),
        "host_run" => result["exit_code"] == 0 && result["command_fingerprint"].as_str() == expected.command_fingerprint.as_deref()
            && result["cwd"].as_str() == expected.cwd.as_deref() && result["protected_reviewed"] == true
            && result["before"].is_string() && result["before"] == result["after"],
        "qa" => result["verdict"] == "pass" && result["handoff_id"].is_string() && result["reviewer_epoch"].is_string(),
        "reviewed_apply" => result["installed"] == true && result["live_commit"].is_string()
            && result["patches"].as_array().is_some_and(|patches| !patches.is_empty()),
        _ => false,
    };
    if !passed { return Err(invalid()); }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunBinding { pub assignment: String, pub gate: GateKind }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunReview {
    pub binding: RunBinding,
    pub work: super::WorkRef,
    pub commit: String,
    pub cwd: String,
    pub before: String,
    pub command_fingerprint: String,
    pub approval_hash: String,
}

fn protected_checkout(cwd: &str) -> Result<PathBuf, TeamError> {
    let path = Path::new(cwd);
    // These running copies are excluded from agent launches/mounts. Never inspect git in a source clone.
    if !["dotfiles","agent-cockpit","vitals"].iter().any(|name| path == crate::paths::home().join(name)) { return Err(invalid()); }
    for ancestor in path.ancestors() {
        if std::fs::symlink_metadata(ancestor)?.file_type().is_symlink() { return Err(invalid()); }
    }
    let canonical = std::fs::canonicalize(path)?;
    if canonical != path { return Err(invalid()); }
    let metadata = std::fs::symlink_metadata(path.join(".git"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || path.join(".git/objects/info/alternates").exists() { return Err(invalid()); }
    Ok(canonical)
}

fn git(cwd: &Path, arguments: &[&str]) -> Result<String, TeamError> {
    let output = std::process::Command::new("git").arg("-C").arg(cwd).args(arguments)
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY").env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES").output()?;
    if !output.status.success() { return Err(TeamError::new(ErrorCode::Unavailable,"cannot observe protected reviewed git identity")); }
    String::from_utf8(output.stdout).map(|text|text.trim().into()).map_err(|_|invalid())
}

fn identity(cwd: &Path) -> Result<String, TeamError> {
    if !git(cwd,&["status","--porcelain","--untracked-files=normal"])?.is_empty() { return Err(invalid()); }
    Ok(format!("{}:{}",git(cwd,&["rev-parse","HEAD"])?,git(cwd,&["rev-parse","HEAD^{tree}"])?))
}

fn patch_identity(cwd: &Path, contents: &[u8]) -> Result<String, TeamError> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new("git").arg("-C").arg(cwd).args(["patch-id","--stable"])
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    child.stdin.take().ok_or_else(invalid)?.write_all(contents)?;
    let output = child.wait_with_output()?;
    let text = String::from_utf8(output.stdout).map_err(|_|invalid())?;
    let mut lines = text.lines();
    let hash = lines.next().and_then(|line|line.split_whitespace().next()).ok_or_else(invalid)?;
    if !output.status.success() || lines.next().is_some() { return Err(invalid()); }
    Ok(hash.into())
}

fn assignment_gate(connection: &Connection, assignment: &str, kind: GateKind, source: &str) -> Result<(super::WorkBrief,String), TeamError> {
    let row: Option<(String,String)> = connection.query_row("SELECT w.brief,w.approval_hash FROM chief_assignments a JOIN chief_work_revisions w
        ON w.project=a.project AND w.task_id=a.task_id AND w.revision=a.work_revision
        WHERE a.id=?1 AND a.state NOT IN ('cancelled','uncertain','blocked') AND w.state IN ('approved','completed')", [assignment], |row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    let (encoded,hash) = row.ok_or_else(invalid)?;
    let brief: super::WorkBrief = serde_json::from_str(&encoded)?;
    if !brief.gates.iter().any(|gate|gate.kind == kind && gate.source == source) { return Err(invalid()); }
    Ok((brief,hash))
}

pub fn prepare_run(connection: &Connection, binding: &RunBinding, command: &str, cwd: &str) -> Result<RunReview, TeamError> {
    let checkout = protected_checkout(cwd)?;
    if binding.gate != GateKind::Tests { return Err(invalid()); }
    let (brief,hash) = assignment_gate(connection,&binding.assignment,binding.gate,"host_run")?;
    let gate = brief.gates.iter().find(|gate|gate.kind == GateKind::Tests).ok_or_else(invalid)?;
    let expected: Expected = serde_json::from_value(gate.expected.clone())?;
    let fingerprint = crate::audit::sha256_hex(command.as_bytes());
    if expected.cwd.as_deref() != Some(cwd) || expected.command_fingerprint.as_deref() != Some(fingerprint.as_str()) { return Err(invalid()); }
    let before = identity(&checkout)?;
    let live_commit = before.split(':').next().ok_or_else(invalid)?;
    // Source/live hashes differ under git am: only the host's reviewed patch correspondence maps them.
    let mapped: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_sources WHERE json_extract(evidence,'$.source')='reviewed_apply'
        AND json_extract(evidence,'$.work')=json(?1) AND json_extract(evidence,'$.commit')=?2
        AND json_extract(evidence,'$.result.cwd')=?3 AND json_extract(evidence,'$.result.live_commit')=?4
        AND json_extract(evidence,'$.result.installed')=1 AND approval_hash=?5)",
        params![serde_json::to_string(&brief.work)?,expected.commit,cwd,live_commit,hash], |row|row.get(0))?;
    if !mapped { return Err(TeamError::new(ErrorCode::Unavailable,"protected reviewed patch correspondence is missing; a different evidence source requires revised exact work approval")); }
    Ok(RunReview { binding:binding.clone(),work:brief.work,commit:expected.commit,cwd:cwd.into(),before,command_fingerprint:fingerprint,approval_hash:hash })
}

pub fn finish_run(connection: &mut Connection, id: &str, review: &RunReview, command: &str, exit_code: i32, now_ms: i64) -> Result<(), TeamError> {
    let checkout = protected_checkout(&review.cwd)?;
    let (_,hash) = assignment_gate(connection,&review.binding.assignment,GateKind::Tests,"host_run")?;
    if hash != review.approval_hash { return Err(invalid()); }
    let after = identity(&checkout).unwrap_or_else(|_|"changed-or-unavailable".into());
    let evidence = Evidence { id:format!("run-{id}"),work:review.work.clone(),kind:GateKind::Tests,commit:review.commit.clone(),source:"host_run".into(),
        result:json!({"assignment":review.binding.assignment,"command_fingerprint":crate::audit::sha256_hex(command.as_bytes()),"cwd":review.cwd,
            "exit_code":exit_code,"before":review.before,"after":after,"protected_reviewed":true}) };
    super::store::publish_source(connection,&evidence,now_ms)
}

/// Called only by the host's reviewed-apply helper, before its existing displayed yes/no approval.
pub fn prepare_apply(connection: &mut Connection, assignment: &str, cwd: &str, copies: &Path, now_ms: i64) -> Result<String, TeamError> {
    let checkout = protected_checkout(cwd)?;
    let (brief,hash) = assignment_gate(connection,assignment,GateKind::Apply,"reviewed_apply")?;
    let expected = brief.gates.iter().find(|gate|gate.kind == GateKind::Apply).ok_or_else(invalid)?;
    let mut files = std::fs::read_dir(copies)?.map(|entry|entry.map(|entry|entry.path())).collect::<Result<Vec<_>,_>>()?;
    files.retain(|path|path.extension().is_some_and(|extension|extension == "patch"));
    files.sort();
    let mut patches = Vec::new();
    for file in files {
        if std::fs::symlink_metadata(&file)?.file_type().is_symlink() { return Err(invalid()); }
        let contents = std::fs::read(&file)?;
        let text = std::str::from_utf8(&contents).map_err(|_|invalid())?;
        let commit = text.lines().next().and_then(|line|line.strip_prefix("From ")).and_then(|line|line.split_whitespace().next()).ok_or_else(invalid)?;
        patches.push(json!({"source_commit":commit,"sha256":crate::audit::sha256_hex(&contents),"patch_id":patch_identity(&checkout,&contents)?}));
    }
    if patches.last().map(|patch|&patch["source_commit"]) != Some(&expected.expected["commit"]) { return Err(invalid()); }
    let token = crate::teams::store::identifier()?;
    let review = json!({"assignment":assignment,"work":brief.work,"approval_hash":hash,"commit":expected.expected["commit"],
        "cwd":cwd,"before":identity(&checkout)?,"patches":patches});
    connection.execute("INSERT INTO chief_apply_reviews VALUES (?1,?2,?3)", params![token,review.to_string(),now_ms])?;
    eprintln!("Chief apply binding (included in the Apply approval): {}",crate::approval::escape_hiding(&review.to_string(),"",""));
    Ok(token)
}

pub fn finish_apply(connection: &mut Connection, token: &str, installed: bool, now_ms: i64) -> Result<(), TeamError> {
    let encoded: String = connection.query_row("SELECT review FROM chief_apply_reviews WHERE id=?1", [token], |row|row.get(0))?;
    let review: Value = serde_json::from_str(&encoded)?;
    let (_,hash) = assignment_gate(connection,review["assignment"].as_str().ok_or_else(invalid)?,GateKind::Apply,"reviewed_apply")?;
    if review["approval_hash"] != hash { return Err(invalid()); }
    let cwd = review["cwd"].as_str().ok_or_else(invalid)?;
    let checkout = protected_checkout(cwd)?;
    let before = review["before"].as_str().and_then(|before|before.split(':').next()).ok_or_else(invalid)?;
    let commits = git(&checkout,&["rev-list","--reverse",&format!("{before}..HEAD")])?;
    let live_commits = commits.lines().collect::<Vec<_>>();
    let mut patches = review["patches"].as_array().ok_or_else(invalid)?.clone();
    if live_commits.len() != patches.len() { return Err(invalid()); }
    for (patch,commit) in patches.iter_mut().zip(live_commits.iter()) {
        let applied = git(&checkout,&["show","--pretty=email","--binary",commit])?;
        if patch["patch_id"] != patch_identity(&checkout,applied.as_bytes())? { return Err(invalid()); }
        patch["live_commit"] = json!(commit);
    }
    let live_commit = git(&checkout,&["rev-parse","HEAD"])?;
    let evidence = Evidence { id:format!("apply-{token}"),work:serde_json::from_value(review["work"].clone())?,kind:GateKind::Apply,
        commit:review["commit"].as_str().ok_or_else(invalid)?.into(),source:"reviewed_apply".into(),
        result:json!({"assignment":review["assignment"],"cwd":cwd,"live_commit":live_commit,"patches":patches,"installed":installed}) };
    super::store::publish_source(connection,&evidence,now_ms)
}
