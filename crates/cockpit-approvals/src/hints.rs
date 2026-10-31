//! What the owner is told about a command beyond its text (#156): a `timeout` that takes
//! the terminal from it, and the build files or scripts the session changed that it runs.
//! Both read the command's words roughly, the way the owner would: a hint too many is
//! read and dismissed, a hint missing hides what runs.

use std::path::Path;

/// Whether the command runs `timeout` without `--foreground`. Without it `timeout` puts the
/// command in a background process group: a prompt (`sudo`, `read`) stops it instead of
/// reading the terminal, and the run hangs until the timeout.
pub fn timeout_without_foreground(command: &str) -> bool {
    let words = words(command);
    words.iter().enumerate().any(|(index, word)| {
        program_name(word) == "timeout"
            && is_command_position(&words, index)
            && !options(&words[index + 1..]).any(is_foreground_option)
    })
}

/// The options before `timeout`'s duration, skipping the values `-s`/`--signal` and
/// `-k`/`--kill-after` take as the next word.
fn options<'a>(after: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    let mut takes_value = false;
    after
        .iter()
        .copied()
        .take_while(move |word| {
            let is_value = std::mem::take(&mut takes_value);
            takes_value = matches!(*word, "-s" | "-k" | "--signal" | "--kill-after");
            is_value || word.starts_with('-')
        })
        .filter(|word| word.starts_with('-'))
}

/// `--foreground`, or an unambiguous abbreviation of it (`--f` is one for GNU `timeout`).
fn is_foreground_option(option: &str) -> bool {
    option.len() >= 3 && "--foreground".starts_with(option)
}

/// `timeout` as a program, not as an argument (`echo timeout`, `make timeout-test`): first
/// word, or right after an operator or a word that runs the next one.
fn is_command_position(words: &[&str], index: usize) -> bool {
    index == 0
        || matches!(
            words[index - 1],
            "sudo" | "exec" | "nice" | "env" | "time" | ";"
        )
}

/// Which of `changed_files` (paths from the checkout's root) `command` runs: build files of
/// the tools it calls (`make` → Makefiles, `cargo` → `build.rs`, a package manager →
/// `package.json`, `gh` → workflows, `git` → hook managers' files) and files it names.
/// Sorted, each once.
pub fn build_files_run(command: &str, changed_files: &[String]) -> Vec<String> {
    let words = words(command);
    let mut run: Vec<String> = changed_files
        .iter()
        .filter(|changed| {
            words
                .iter()
                .any(|word| tool_runs(program_name(word), changed) || names_file(word, changed))
        })
        .cloned()
        .collect();
    run.sort();
    run.dedup();
    run
}

fn tool_runs(program: &str, changed: &str) -> bool {
    let file_name = Path::new(changed)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(changed);
    match program {
        "make" | "gmake" => {
            matches!(file_name, "Makefile" | "makefile" | "GNUmakefile")
                || file_name.ends_with(".mk")
        }
        "cargo" => file_name == "build.rs" || changed.contains(".cargo/config"),
        "npm" | "npx" | "pnpm" | "yarn" | "bun" => file_name == "package.json",
        "gh" | "act" => changed.starts_with(".github/workflows/"),
        "git" | "pre-commit" | "lefthook" => {
            changed.starts_with(".husky/")
                || changed.starts_with(".githooks/")
                || matches!(
                    file_name,
                    ".pre-commit-config.yaml" | "lefthook.yml" | "lefthook.yaml"
                )
        }
        _ => false,
    }
}

/// Whether `word` names `changed`: the same path, or one ending in the other at a `/` (the
/// command may run from a subdirectory, or name the file absolutely).
fn names_file(word: &str, changed: &str) -> bool {
    let word = word.trim_start_matches("./");
    !word.is_empty()
        && (word == changed
            || changed.ends_with(&format!("/{word}"))
            || word.ends_with(&format!("/{changed}")))
}

/// The program a word names: its last path component (`/usr/bin/make` is `make`).
fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The command's words, split at blanks, quotes and shell operators; `;` stands for every
/// operator that starts another command.
fn words(command: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, character) in command.char_indices() {
        let is_separator = character.is_whitespace() || "'\"`$(){}<>".contains(character);
        let is_operator = ";|&\n".contains(character);
        if is_separator || is_operator {
            if let Some(word_start) = start.take() {
                words.push(&command[word_start..index]);
            }
            if is_operator && words.last() != Some(&";") {
                words.push(";");
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(word_start) = start {
        words.push(&command[word_start..]);
    }
    words
}
