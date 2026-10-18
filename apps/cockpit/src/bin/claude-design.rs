//! claude-design: which Claude Design canvas this session works on, for the cockpit's card.
//!
//!     claude-design link <canvas url> '<canvas title>'
//!     claude-design unlink
//!     claude-design show
//!
//! Run by Claude inside the sandbox. The link goes to ~/.cache/claude/designs/<session id>.json
//! (the session id from CLAUDE_CODE_SESSION_ID); the cockpit's column shows the title on the
//! session's card and opens the canvas on o. A canvas address is https://claude.ai/artifact/<id>;
//! a chat address with ?artifact=<id> is turned into it. CLAUDE_DESIGNS_DIR overrides the folder.

use cockpit::sessions::canvas_address;
use cockpit::util::{collapse, now_secs, write_json};
use cockpit::paths;
use serde_json::{Value, json};

const USAGE: &str = "    claude-design link <canvas url> '<canvas title>'
    claude-design unlink
    claude-design show";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let session = std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default();
    let valid_session = session.len() == 36 && session.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if !valid_session {
        eprintln!("claude-design: no session id (CLAUDE_CODE_SESSION_ID); run it from a Claude session");
        std::process::exit(2);
    }
    let path = paths::designs().join(format!("{session}.json"));
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let status = match words.as_slice() {
        ["link", url, title] => {
            let title: String = collapse(title).chars().take(80).collect();
            match canvas_address(url).filter(|_| !title.is_empty()) {
                None => { eprintln!("claude-design: give the canvas address (claude.ai/artifact/<id>) and its title"); 2 }
                Some(url) => match write_json(&path, &json!({"url": url, "title": title, "at": now_secs()})) {
                    Ok(()) => { println!("This session's card now shows the canvas {title:?}; o in the cockpit opens it."); 0 }
                    Err(error) => { eprintln!("claude-design: {error}"); 1 }
                },
            }
        }
        ["unlink"] => {
            let _ = std::fs::remove_file(&path);
            println!("No canvas linked to this session.");
            0
        }
        ["show"] => {
            let url = cockpit::util::read_json(&path).and_then(|link| link.get("url").and_then(Value::as_str).map(str::to_string));
            println!("{}", url.unwrap_or_else(|| "No canvas linked to this session.".into()));
            0
        }
        _ => { eprintln!("{USAGE}"); 2 }
    };
    std::process::exit(status);
}
