//! No adapter holds a closed list of a harness's models or effort levels (#743): a harness
//! update adds them between Torlek releases, so each adapter asks the installed harness or
//! passes the value through for the harness to refuse. A list would make a new level
//! unusable until Torlek ships, and a removed one fail late.

use std::fs;
use std::path::{Path, PathBuf};

const ADAPTERS: [&str; 3] = ["cockpit-claude", "cockpit-codex", "cockpit-opencode"];

/// Quoted words a closed list of effort levels or models is made of.
const CHOICE_WORDS: [&str; 14] = [
    "low", "medium", "high", "xhigh", "max", "minimal", "opus", "sonnet", "haiku", "fable",
    "gpt-5", "o3", "o4-mini", "claude-",
];

fn adapter_sources() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("this crate lives in crates/<name>/");
    let mut files = Vec::new();
    for adapter in ADAPTERS {
        let mut pending = vec![crates.join(adapter).join("src")];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).unwrap_or_else(|error| panic!("{dir:?}: {error}")) {
                let path = entry.expect("entry is readable").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    files
}

/// The `const` and `static` items of `source`, each from its keyword to the `;` after its
/// `=` (an array type holds a `;` of its own). A text check: it also reads items inside
/// functions and tests.
fn constant_items(source: &str) -> Vec<&str> {
    let mut items = Vec::new();
    for (at, _) in source
        .match_indices("const ")
        .chain(source.match_indices("static "))
    {
        // Not the `static` of `&'static str`, nor a `const fn`.
        let starts_a_word = source[..at]
            .chars()
            .next_back()
            .is_none_or(|before| !before.is_alphanumeric() && !"_'".contains(before));
        let rest = &source[at..];
        if rest.split_whitespace().nth(1) == Some("fn") {
            continue;
        }
        let end = rest
            .find('=')
            .and_then(|equals| rest[equals..].find(';').map(|semicolon| equals + semicolon));
        if let (true, Some(end)) = (starts_a_word, end) {
            items.push(&rest[..end]);
        }
    }
    items
}

/// Whether `item` holds a quoted effort level or model name.
fn lists_a_choice(item: &str) -> bool {
    CHOICE_WORDS.iter().any(|word| {
        let exact_or_prefix = if word.ends_with('-') {
            format!("\"{word}")
        } else {
            format!("\"{word}\"")
        };
        item.contains(&exact_or_prefix)
    })
}

#[test]
fn no_adapter_source_holds_a_const_list_of_models_or_effort_levels() {
    let offenders: Vec<String> = adapter_sources()
        .iter()
        .flat_map(|path| {
            let text = fs::read_to_string(path).expect("source is readable");
            constant_items(&text)
                .into_iter()
                .filter(|item| lists_a_choice(item))
                .map(|item| format!("{}: {}", path.display(), item.lines().next().unwrap_or("")))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "an adapter asks the installed harness for its models and effort levels, or passes \
         them through; it keeps no list:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_list_check_catches_a_const_of_levels_or_models() {
    for caught in [
        r#"pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];"#,
        r#"static MODELS: &[&str] = &["opus", "sonnet"];"#,
        r#"const DEFAULT_MODEL: &str = "claude-opus-5-5";"#,
        r#"fn name() -> &'static str { "x" } const LEVELS: [&str; 2] = ["low", "high"];"#,
    ] {
        let items = constant_items(caught);
        assert!(items.iter().any(|item| lists_a_choice(item)), "{caught}");
    }
    for passed in [
        r#"const HOSTS: [&str; 3] = ["api.openai.com", "api.anthropic.com", "models.dev"];"#,
        "const MAX_MODEL_BYTES: usize = 200;",
        r#"const LAUNCH_FLAGS: [&str; 2] = ["--effort", "--model"];"#,
        r#"fn label() -> &'static str { "low" }"#,
    ] {
        let items = constant_items(passed);
        assert!(!items.iter().any(|item| lists_a_choice(item)), "{passed}");
    }
}
