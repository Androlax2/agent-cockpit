//! claude-audit: the audit log (cockpit::audit), for the scripts outside the sandbox and for Theo.
//!
//!     claude-audit record <component> <event> <outcome> [key=value | fp:key=value ...]
//!                                     one event; values are text, an fp: one is kept only as its
//!                                     hash and length (claude-sessions' audit events)
//!     claude-audit verify             walks the hash chain; exit 1 at the first break, named
//!
//! The log is ~/.local/state/claude-audit (greywall hides it from every sandbox).

use cockpit::audit;
use serde_json::{Map, Value};

const USAGE: &str = "usage: claude-audit record <component> <event> <outcome> [key=value | fp:key=value ...] | claude-audit verify";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let status = match words.as_slice() {
        ["verify"] => match audit::verify() {
            Ok(count) => {
                println!("The chain holds: {count} line(s) in {}.", audit::dir().display());
                0
            }
            Err(reason) => {
                println!("The chain breaks: {reason}.");
                1
            }
        },
        ["record", component, event, outcome, pairs @ ..] => {
            let mut fields = Map::new();
            for pair in pairs {
                let Some((key, value)) = pair.split_once('=') else {
                    eprintln!("claude-audit: {pair:?} is not key=value\n{USAGE}");
                    std::process::exit(2);
                };
                match key.strip_prefix("fp:") {
                    Some(key) => fields.insert(key.to_string(), audit::fingerprint(value)),
                    None => fields.insert(key.to_string(), Value::from(value)),
                };
            }
            audit::record(component, event, outcome, Value::Object(fields));
            0
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(status);
}
