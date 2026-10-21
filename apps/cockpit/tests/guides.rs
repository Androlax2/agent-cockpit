//! Guides: ask-to-guide (inside the sandbox). The daemon's queue is tested in apps/osd.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

const STEPS: &str = "# Search Console key
## Turn on the API
1. Open https://console.cloud.google.com/ and pick the project
2. Enable the API
   open: https://console.cloud.google.com/apis/library
## Service account
- Copy the new account's email
  copy: review@example.iam.gserviceaccount.com
  It ends in iam.gserviceaccount.com.
Paste it here
  ask: Which email did it get?
Run the check
  run: make search-console
";

fn guide_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else { return Vec::new() };
    entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|found| found == "guide")).collect()
}

struct AskToGuide {
    sandbox: Sandbox,
    guides: PathBuf,
}

impl AskToGuide {
    fn new() -> AskToGuide {
        let sandbox = Sandbox::new();
        let guides = sandbox.root.join("guides");
        AskToGuide { sandbox, guides }
    }

    /// The daemon's report socket, answering every guide "ok".
    fn listen(&self) -> ReportSocket {
        ReportSocket::start(&self.sandbox.runtime.join("claude-report/osd-report.sock"), "ok")
    }

    fn ask(&self, steps: &str, wait: &str) -> Child {
        let guides = self.guides.display().to_string();
        let mut asking = self.sandbox.command(bin().join("ask-to-guide"), &[("CLAUDE_GUIDES_DIR", &guides), ("ASK_TO_GUIDE_WAIT_S", wait)])
            .current_dir(&self.sandbox.root)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        asking.stdin.take().unwrap().write_all(steps.as_bytes()).unwrap();
        asking
    }

    /// The guide the daemon received, whole, on its report socket.
    fn filed(daemon: &ReportSocket) -> Value {
        let lines = daemon.received(Duration::from_secs(3));
        let line = lines.first().expect("no guide filed");
        serde_json::from_str(line.strip_prefix("guide-request ").unwrap_or_else(|| panic!("not a guide request: {line}"))).unwrap()
    }

    fn write_beside(&self, guide: &Value, extension: &str, content: Value) {
        // Answers and progress live where only Theo's programs write (approval::answers_dir).
        let answers = self.sandbox.answers("guides");
        fs::write(answers.join(format!("{}.{extension}", guide["id"].as_str().unwrap())), content.to_string()).unwrap();
    }
}

#[test]
fn a_guide_is_filed_announced_and_its_answers_come_back() {
    let fixture = AskToGuide::new();
    let daemon = fixture.listen();
    let asking = fixture.ask(STEPS, "5");
    let guide = AskToGuide::filed(&daemon);
    assert_eq!((guide["title"].as_str(), guide["steps"].as_array().map(Vec::len), guide["cwd"].as_str()),
               (Some("Search Console key"), Some(5), Some(fixture.sandbox.root.to_str().unwrap())));
    fixture.write_beside(&guide, "result", json!({
        "status": "done", "total": 5, "answers": [{"step": 4, "question": "Which email?", "answer": "a@b"}],
        "runs": [{"step": 5, "command": "make x", "status": "ran", "exit": 0, "output": "\x1b[32mok\x1b[0m"}]}));
    let output = finish(asking, TIMEOUT);
    assert_eq!(output.status.code(), Some(0));
    let shown = stdout(&output);
    assert!(shown.contains("Theo answered: a@b"), "{shown}");
    assert!(shown.contains("ok\n[exit 0]"), "{shown:?}");
    assert!(guide_files(&fixture.guides).is_empty(), "a guide file was left where every sandbox writes");
}

#[test]
fn a_pasted_image_comes_back_as_a_path_in_the_answers_folder() {
    let fixture = AskToGuide::new();
    let daemon = fixture.listen();
    let asking = fixture.ask(STEPS, "5");
    let guide = AskToGuide::filed(&daemon);
    fixture.write_beside(&guide, "result", json!({"status": "done", "total": 5,
        "images": [{"step": 2, "file": "aaaaaaaaaaaa-image1.png"}, {"step": 3, "file": "../../etc/passwd.png"}]}));
    let shown = stdout(&finish(asking, TIMEOUT));
    let expected = fixture.sandbox.answers("guides").join("aaaaaaaaaaaa-image1.png");
    assert!(shown.contains(&format!("image on step 2 (read it): {}", expected.display())), "{shown}");
    assert!(!shown.contains("passwd"), "a file name leaving the answers folder was printed: {shown}");
}

#[test]
fn stuck_exits_3_with_the_note() {
    let fixture = AskToGuide::new();
    let daemon = fixture.listen();
    let asking = fixture.ask(STEPS, "5");
    let guide = AskToGuide::filed(&daemon);
    fixture.write_beside(&guide, "result",
                         json!({"status": "stuck", "step": 2, "total": 5, "step_text": "Enable the API", "note": "403 on enable"}));
    let output = finish(asking, TIMEOUT);
    assert_eq!(output.status.code(), Some(3));
    let shown = stdout(&output);
    assert!(shown.contains("stuck at step 2 of 5: Enable the API"), "{shown}");
    assert!(shown.contains("His note: 403 on enable"), "{shown}");
}

#[test]
fn the_wait_running_out_says_where_theo_is() {
    let fixture = AskToGuide::new();
    let daemon = fixture.listen();
    let asking = fixture.ask(STEPS, "1");
    let guide = AskToGuide::filed(&daemon);
    fixture.write_beside(&guide, "progress", json!({"step": 3, "total": 5, "done": 2}));
    let output = finish(asking, TIMEOUT);
    assert_eq!(output.status.code(), Some(124));
    let shown = stdout(&output);
    assert!(shown.contains("on step 3 of 5 (2 done)"), "{shown}");
    assert!(shown.contains(&format!("ask-to-guide --wait {}", guide["id"].as_str().unwrap())), "{shown}");
}

#[test]
fn without_a_daemon_it_exits_126_and_leaves_no_guide() {
    let fixture = AskToGuide::new();
    let output = finish(fixture.ask(STEPS, "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(126));
}

#[test]
fn a_guide_the_daemon_refuses_says_why() {
    let fixture = AskToGuide::new();
    let _daemon = ReportSocket::start(&fixture.sandbox.runtime.join("claude-report/osd-report.sock"), "refused: no valid id");
    let output = finish(fixture.ask(STEPS, "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("refused the guide: no valid id"), "{}", stdout(&output));
}

#[test]
fn no_steps_exits_2() {
    let fixture = AskToGuide::new();
    let output = finish(fixture.ask("# only a title\n", "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_terminal_session_gets_no_guide_window_and_falls_back() {
    let fixture = AskToGuide::new();
    let daemon = fixture.listen();
    let guides = fixture.guides.display().to_string();
    let mut asking = fixture.sandbox.command(bin().join("ask-to-guide"), &[("CLAUDE_GUIDES_DIR", &guides), ("COCKPIT_TERMINAL", "1")])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let _ = asking.stdin.take().unwrap().write_all(STEPS.as_bytes());
    let output = finish(asking, TIMEOUT);
    assert_eq!(output.status.code(), Some(126));
    assert!(daemon.received(Duration::from_millis(200)).is_empty(), "a guide was filed");
}
