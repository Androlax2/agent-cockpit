//! A made-up Claude Code cockpit for the README screenshots (tools/capture-claude.sh).
//!
//!     claude-demo build <root>   everything the column reads, invented, under <root>
//!     claude-demo session        a still picture of a session for the demo's session pane
//!
//! build writes home/ (statusline facts, history, plan name, parallel clones), run/ (the session
//! snapshot, plan usage, the viewed session), state/ (todo list), repos/ (git checkouts with their
//! branch and a github.com origin), bin/ (a gh that answers pull requests and issues from here, a
//! no-op claude-sessions), and status.json for both provider status feeds. Pointing HOME, XDG_RUNTIME_DIR,
//! XDG_STATE_HOME, PATH, CLAUDE_STATUS_URL and CODEX_STATUS_URL at it shows none of the real sessions, tasks or costs.

use chrono::{Duration as Span, Utc};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

struct DemoSession {
    id: &'static str,
    project: &'static str,
    checkout: &'static str,
    branch: &'static str,
    repository: &'static str,
    state: &'static str,
    task: &'static str,
    idle_minutes: i64,
    started_minutes: i64,
    model: &'static str,
    effort: &'static str,
    context: u32,
}

const SESSIONS: [DemoSession; 4] = [
    DemoSession { id: "11111111-1111-4111-8111-111111111111", project: "acme-shop", checkout: "repos/acme-shop", branch: "feat/checkout-vat",
                  repository: "acme/shop", state: "working", task: "Checkout totals with VAT per country", idle_minutes: 2, started_minutes: 95,
                  model: "Opus 5.5", effort: "high", context: 41 },
    DemoSession { id: "22222222-2222-4222-8222-222222222222", project: "acme-shop", checkout: "home/IdeaProjects/.parallel/acme-shop/parallel-0412",
                  branch: "fix/cart-rounding", repository: "acme/shop", state: "needs-you", task: "Cart rounding on discounted items", idle_minutes: 0,
                  started_minutes: 40, model: "Opus 5.5", effort: "high", context: 18 },
    DemoSession { id: "33333333-3333-4333-8333-333333333333", project: "notes-api", checkout: "repos/notes-api", branch: "main",
                  repository: "acme/notes-api", state: "waiting", task: "Paginate the search endpoint", idle_minutes: 6, started_minutes: 70,
                  model: "Sonnet 5.5", effort: "medium", context: 63 },
    DemoSession { id: "44444444-4444-4444-8444-444444444444", project: "homelab", checkout: "repos/homelab", branch: "monitoring",
                  repository: "acme/homelab", state: "waiting", task: "Grafana dashboard for the NAS disks", idle_minutes: 25, started_minutes: 180,
                  model: "Opus 5.5", effort: "xhigh", context: 92 },
];

fn iso(seconds_from_now: i64) -> String {
    (Utc::now() + Span::seconds(seconds_from_now)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn write(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(path, content).map_err(|error| format!("{}: {error}", path.display()))
}

fn write_executable(path: &Path, content: &str) -> Result<(), String> {
    write(path, content)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|error| format!("{}: {error}", path.display()))
}

fn git(arguments: &[&str]) -> Result<(), String> {
    let status = Command::new("git").args(arguments).status().map_err(|error| format!("git: {error}"))?;
    if status.success() { Ok(()) } else { Err(format!("git {} failed", arguments.join(" "))) }
}

fn checkout(path: &Path, branch: &str, repository: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let path = path.to_string_lossy();
    git(&["init", "-q", "-b", branch, &path])?;
    git(&["-C", &path, "remote", "add", "origin", &format!("https://github.com/{repository}.git")])
}

fn session_rows(root: &Path, now: i64) -> Result<Vec<Value>, String> {
    let home = root.join("home");
    let mut rows = Vec::new();
    for (index, session) in SESSIONS.iter().enumerate() {
        let cwd = root.join(session.checkout);
        checkout(&cwd, session.branch, session.repository)?;
        let mut row = json!({"session": session.id, "state": session.state, "project": session.project, "cwd": cwd, "task": session.task,
                             "since": now - session.idle_minutes * 60, "started": now - session.started_minutes * 60, "tmux_pane": format!("%9{index}")});
        if session.state == "needs-you" {
            row["request"] = json!("Claude needs your permission to use Bash");
            row["detail"] = json!("npm test -- cart/rounding");
        }
        if session.state == "waiting" && session.project == "notes-api" {
            row["outcome"] = json!("Finished · search pages by 50 with a cursor; 14 tests pass");
        }
        rows.push(row);
        write(&home.join(format!(".cache/claude/statusline-{}.json", session.id)),
              &json!({"model": session.model, "effort": session.effort, "context": session.context}).to_string())?;
    }
    Ok(rows)
}

fn build(root: &Path) -> Result<(), String> {
    let (home, run, state) = (root.join("home"), root.join("run"), root.join("state"));
    let now = Utc::now().timestamp();
    let viewed = SESSIONS[0].id;
    write(&run.join("claude-sessions.json"), &json!({"sessions": session_rows(root, now)?}).to_string())?;
    write(&run.join("claude-sessions.current"), &format!("{viewed}\n"))?;
    write(&run.join("claude-usage.json"), &json!({
        "five_hour": {"utilization": 34, "resets_at": iso(2 * 3600 + 40 * 60)},
        "seven_day": {"utilization": 58, "resets_at": iso(3 * 86400)},
        "seven_day_opus": {"utilization": 41, "resets_at": iso(3 * 86400)}}).to_string())?;
    // The plan's name only: no token of any kind.
    write(&home.join(".claude/.credentials.json"), &json!({"claudeAiOauth": {"subscriptionType": "max"}}).to_string())?;
    let history: String = [("repos/acme-shop", 20, "Move the invoice PDF to the new template"),
                           ("repos/notes-api", 30, "Rate limit the public API per token"),
                           ("repos/homelab", 50, "Back up the Postgres volumes to the NAS nightly")]
        .iter().enumerate().map(|(index, (project, hours, prompt))| {
            json!({"sessionId": format!("5555555{index}-5555-4555-8555-555555555555"), "project": root.join(project),
                   "timestamp": (now - hours * 3600) * 1000, "display": prompt}).to_string() + "\n"
        }).collect();
    write(&home.join(".claude/history.jsonl"), &history)?;
    let shop = root.join("repos/acme-shop");
    write(&state.join("claude-sessions/todo.json"), &json!({"items": [
        {"id": "a1b2c3d4", "project": "acme-shop", "cwd": shop, "text": "Show prices with and without VAT on the product page",
         "status": "doing", "priority": 4, "created": now - 3600},
        {"id": "b2c3d4e5", "project": "acme-shop", "cwd": shop, "text": "Remove the old coupon endpoint",
         "status": "todo", "priority": 2, "created": now - 7200},
        {"id": "c3d4e5f6", "project": "acme-shop", "cwd": shop, "text": "Translate the checkout emails",
         "status": "done", "priority": 2, "created": now - 86400}],
        "issues": {"acme/shop#57": {"status": "doing", "updated": now - 600, "session": viewed}}}).to_string())?;
    let pulls = json!([
        {"number": 61, "title": "Checkout totals with VAT per country", "url": "https://github.com/acme/shop/pull/61",
         "updatedAt": iso(-1200), "author": {"login": "claude"}, "isDraft": true, "reviewDecision": "",
         "statusCheckRollup": [{"status": "IN_PROGRESS", "conclusion": ""}], "headRefName": "feat/checkout-vat"},
        {"number": 58, "title": "Cache product images at the edge", "url": "https://github.com/acme/shop/pull/58",
         "updatedAt": iso(-7200), "author": {"login": "sam"}, "isDraft": false, "reviewDecision": "REVIEW_REQUIRED",
         "statusCheckRollup": [{"status": "COMPLETED", "conclusion": "SUCCESS"}], "headRefName": "perf/image-cache"},
        {"number": 55, "title": "Upgrade the payment SDK", "url": "https://github.com/acme/shop/pull/55",
         "updatedAt": iso(-86400), "author": {"login": "alex"}, "isDraft": false, "reviewDecision": "CHANGES_REQUESTED",
         "statusCheckRollup": [{"status": "COMPLETED", "conclusion": "FAILURE"}], "headRefName": "deps/payment-sdk"}]);
    let issues = json!([
        {"number": 57, "title": "VAT missing on shipping costs", "url": "https://github.com/acme/shop/issues/57",
         "updatedAt": iso(-3600), "labels": [{"name": "bug"}, {"name": "P1"}]},
        {"number": 49, "title": "Wishlist export to CSV", "url": "https://github.com/acme/shop/issues/49",
         "updatedAt": iso(-5 * 86400), "labels": [{"name": "enhancement"}]}]);
    write(&root.join("gh-pulls.json"), &pulls.to_string())?;
    write(&root.join("gh-issues.json"), &issues.to_string())?;
    let root_text = root.display();
    write_executable(&root.join("bin/gh"), &format!(r#"#!/bin/sh
# The demo's GitHub: only acme/shop has pull requests and issues.
case "$*" in
  "pr list --repo acme/shop"*) cat {root_text}/gh-pulls.json ;;
  "issue list --repo acme/shop"*) cat {root_text}/gh-issues.json ;;
  *) echo "[]" ;;
esac
"#))?;
    write_executable(&root.join("bin/claude-sessions"), "#!/bin/sh\n# The demo has no real sessions to act on.\nexit 0\n")?;
    write(&root.join("status.json"), &json!({"status": {"indicator": "none", "description": "All Systems Operational"},
                                             "components": [], "incidents": []}).to_string())
}

const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";
const WHITE: &str = "\x1b[97m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const ORANGE: &str = "\x1b[38;5;173m";
const GREY: &str = "\x1b[90m";

fn conversation() -> String {
    format!(r#"
{GREY}>{RESET} Add VAT to the checkout total, per the customer's country. Rates are in config/vat.json.

{WHITE}●{RESET} I'll read how the total is computed, then add the VAT step and its tests.

{GREEN}●{RESET} {BOLD}Read{RESET}(src/checkout/total.ts)
  {GREY}⎿  Read 48 lines{RESET}

{GREEN}●{RESET} {BOLD}Update{RESET}(src/checkout/total.ts)
  {GREY}⎿  Updated src/checkout/total.ts with 7 additions and 2 removals{RESET}
       {GREY}21{RESET} {RED}-  return subtotal + shipping{RESET}
       {GREY}21{RESET} {GREEN}+  const rate = vatRate(customer.country){RESET}
       {GREY}22{RESET} {GREEN}+  const vat = roundCents((subtotal + shipping) * rate){RESET}
       {GREY}23{RESET} {GREEN}+  return {{ net: subtotal + shipping, vat, gross: subtotal + shipping + vat }}{RESET}

{GREEN}●{RESET} {BOLD}Bash{RESET}(npm test -- checkout)
  {GREY}⎿  PASS  tests/checkout/total.test.ts (9 tests){RESET}
     {GREY}PASS  tests/checkout/vat.test.ts (6 tests){RESET}

{WHITE}●{RESET} Checkout totals now include VAT for the customer's country:
  - {BOLD}total.ts{RESET} returns net, VAT and gross, VAT rounded to the cent
  - {BOLD}vat.test.ts{RESET} covers France, Germany, a zero-rate country and a missing rate
  - The product page still shows net prices: that is the next todo item.

{ORANGE}✻{RESET} {ORANGE}Writing the pull request description…{RESET} {GREY}(38s · ↓ 1.2k tokens){RESET}
"#)
}

/// Draws the conversation once, then waits for the screenshot.
fn session() -> ! {
    use std::io::Write;
    let columns = cockpit::approval::terminal_size().0;
    let rule = format!("{GREY}{}{RESET}", "─".repeat(columns));
    let mut screen = std::io::stdout();
    let _ = write!(screen, "\x1b[2J\x1b[H{}\n{rule}\n{GREY}>{RESET} \n{rule}\n  {GREY}⏵⏵ auto mode on (shift+tab to cycle){RESET}", conversation());
    let _ = screen.flush();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["build", root] => {
            let root = std::path::absolute(root).unwrap_or_else(|_| root.into());
            if let Err(error) = build(&root) {
                eprintln!("claude-demo: {error}");
                std::process::exit(1);
            }
        }
        ["session"] => session(),
        _ => {
            eprintln!("usage: claude-demo build <root> | claude-demo session");
            std::process::exit(2);
        }
    }
}
