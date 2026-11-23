//! Whether every commit an agent's push sends carries Torlek's trailer naming a session, and no
//! signature (#694): an agent's commit is attributed by its trailer, never by the owner's key.

use std::path::Path;

use crate::Error;
use crate::git::{Git, arg};

/// The variable a session's launch sets to the trailer its commits carry.
pub const COMMIT_TRAILER_VARIABLE: &str = "TORLEK_COMMIT_TRAILER";

const TRAILER_KEY: &str = "Agent";
const TRAILER_JOIN: &str = " via Torlek ";

/// The commits a refusal names one by one; the rest are counted.
const NAMED_COMMITS: usize = 10;

/// The trailer a commit `provider`'s agent makes in `session` carries, as
/// [`COMMIT_TRAILER_VARIABLE`] holds it: `Agent: <provider> via Torlek <session>`.
pub fn agent_trailer(provider: &str, session: &str) -> String {
    format!("{TRAILER_KEY}: {provider}{TRAILER_JOIN}{session}")
}

/// Whether `value`, an `Agent` trailer's value, is in Torlek's form: a provider and a session,
/// one word each. Which session it names is the agent's word, not checked against any.
fn is_agent_trailer_value(value: &str) -> bool {
    let is_word = |word: &str| !word.is_empty() && !word.contains(char::is_whitespace);
    value
        .split_once(TRAILER_JOIN)
        .is_some_and(|(provider, session)| is_word(provider) && is_word(session))
}

/// Refuses the push when a commit it sends lacks an `Agent` trailer in Torlek's form, or is
/// signed.
///
/// What it sends is what `commit` reaches and none of `remote_tips` (every branch tip the
/// remote listed) does, as `git push` reckons it: a tip the checkout lacks is passed over,
/// replace refs are off as `pack-objects` has them, and grafts and a shallow boundary apply,
/// since a commit they hide from this walk is hidden from the pack too, and the remote then
/// refuses the push for lacking it. A merge of a signed tag keeps the tag's signature in its
/// `mergetag` header: the commit itself is unsigned, so it passes.
pub(crate) fn require_attributed(
    git: &Git,
    checkout: &Path,
    remote_tips: &[&str],
    commit: &str,
) -> Result<(), Error> {
    let unattributed = unattributed_commits(git, checkout, remote_tips, commit)?;
    let sent = sent_commits(git, checkout, remote_tips, commit)?;
    let signed: Vec<String> = sent
        .iter()
        .filter(|sent| sent.is_signed)
        .map(|sent| sent.id.clone())
        .collect();
    if unattributed.is_empty() && signed.is_empty() {
        return Ok(());
    }
    let mut problems = Vec::new();
    if !unattributed.is_empty() {
        problems.push(format!(
            "{} no `{TRAILER_KEY}: <provider>{TRAILER_JOIN}<session>` trailer",
            with_verb(&unattributed, "carries", "carry")
        ));
    }
    if !signed.is_empty() {
        problems.push(format!(
            "{} signed, and an agent's commit carries none",
            with_verb(&signed, "is", "are")
        ));
    }
    Err(Error::Refused {
        reason: format!(
            "an agent's push sends only attributed, unsigned commits: {}. Commit with `git \
             commit --trailer \"${COMMIT_TRAILER_VARIABLE}\"`; to rewrite the commits made, \
             `git rebase --rebase-merges --exec 'git commit --amend --no-edit --no-gpg-sign \
             --trailer \"${COMMIT_TRAILER_VARIABLE}\"' {}`",
            problems.join("; "),
            rebase_upstream(&sent, &unattributed, &signed)
        ),
    })
}

/// `commits` as a refusal names them, with `singular` or `plural`: `commit A is` or
/// `commits A, B and N more are`.
fn with_verb(commits: &[String], singular: &str, plural: &str) -> String {
    let listed = commits[..commits.len().min(NAMED_COMMITS)].join(", ");
    match commits.len() {
        1 => format!("commit {listed} {singular}"),
        count if count > NAMED_COMMITS => {
            format!(
                "commits {listed} and {} more {plural}",
                count - NAMED_COMMITS
            )
        }
        _ => format!("commits {listed} {plural}"),
    }
}

/// Where the rebase that rewrites the refused commits starts: before the oldest of them in
/// `sent` (newest first, every commit before its parents), or `--root` when that one has no
/// parent. Only commits the push sends are rewritten, never one the remote holds.
fn rebase_upstream(sent: &[SentCommit], unattributed: &[String], signed: &[String]) -> String {
    let oldest = sent
        .iter()
        .rev()
        .find(|sent| unattributed.contains(&sent.id) || signed.contains(&sent.id));
    match oldest {
        Some(oldest) if oldest.is_root => "--root".to_owned(),
        Some(oldest) => format!("{}^", oldest.id),
        None => "--root".to_owned(),
    }
}

/// A commit the push sends, as its raw header reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SentCommit {
    id: String,
    is_root: bool,
    /// A `gpgsig` header, or `gpgsig-sha256` in a SHA-256 repository.
    is_signed: bool,
}

/// `git rev-list` of the range: `commit` and every tip the remote holds, negated, newest first
/// and every commit before its parents. Each is a full object id, so none reads as an option or
/// a range; a tip the checkout lacks is passed over.
fn rev_list(
    git: &Git,
    checkout: &Path,
    format: &str,
    remote_tips: &[&str],
    commit: &str,
) -> Result<String, Error> {
    let range: Vec<String> = std::iter::once(commit.to_owned())
        .chain(remote_tips.iter().map(|tip| format!("^{tip}")))
        .collect();
    let mut arguments = vec![
        arg("--no-replace-objects"),
        arg("rev-list"),
        arg("--topo-order"),
        arg("--ignore-missing"),
        arg("--no-commit-header"),
        arg(format),
        arg("--end-of-options"),
    ];
    arguments.extend(range.iter().map(arg));
    git.run(checkout, &arguments)
}

/// The range's commits whose `Agent` trailers (as Git parses a message's trailers) include
/// none in Torlek's form.
fn unattributed_commits(
    git: &Git,
    checkout: &Path,
    remote_tips: &[&str],
    commit: &str,
) -> Result<Vec<String>, Error> {
    let listed = rev_list(
        git,
        checkout,
        "--format=%H%x01%(trailers:key=Agent,valueonly,unfold,separator=%x02)%x00",
        remote_tips,
        commit,
    )?;
    Ok(unattributed_in(&listed))
}

/// The ids of `listed`'s entries, `<id>\x01<values>\0` each with values joined by `\x02`, whose
/// values include none that names a provider and a session.
fn unattributed_in(listed: &str) -> Vec<String> {
    listed
        .split('\0')
        .filter_map(|entry| {
            let (id, values) = entry.trim_start_matches('\n').split_once('\x01')?;
            let is_attributed = values.split('\x02').any(is_agent_trailer_value);
            (!is_attributed).then(|| id.to_owned())
        })
        .collect()
}

/// The range's commits, read from their raw headers, so no signature program runs.
fn sent_commits(
    git: &Git,
    checkout: &Path,
    remote_tips: &[&str],
    commit: &str,
) -> Result<Vec<SentCommit>, Error> {
    let raw = rev_list(git, checkout, "--pretty=raw", remote_tips, commit)?;
    Ok(sent_in_raw(&raw))
}

/// The commits `rev-list --pretty=raw` lists. Each starts at a `commit <id>` line, its headers
/// follow up to a blank line, and its message lines are indented, so no message line reads as
/// either.
fn sent_in_raw(raw: &str) -> Vec<SentCommit> {
    let mut commits: Vec<SentCommit> = Vec::new();
    let mut in_headers = false;
    for line in raw.lines() {
        if let Some(id) = line.strip_prefix("commit ") {
            commits.push(SentCommit {
                id: id.trim().to_owned(),
                is_root: true,
                is_signed: false,
            });
            in_headers = true;
            continue;
        }
        if line.is_empty() {
            in_headers = false;
            continue;
        }
        let Some(current) = commits.last_mut().filter(|_| in_headers) else {
            continue;
        };
        if line.starts_with("parent ") {
            current.is_root = false;
        } else if line.starts_with("gpgsig ") || line.starts_with("gpgsig-sha256 ") {
            current.is_signed = true;
        }
    }
    commits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailer_value_names_a_provider_and_a_session() {
        assert!(is_agent_trailer_value("claude via Torlek s5477cc2fd0d1"));
    }

    #[test]
    fn a_trailer_value_missing_its_provider_or_session_names_no_session() {
        for value in [
            "",
            "claude",
            " via Torlek s1",
            "claude via Torlek ",
            "two words via Torlek s1",
            "claude via Torlek s1 extra",
            "claude via Cockpit s1",
        ] {
            assert!(!is_agent_trailer_value(value), "{value:?}");
        }
    }

    #[test]
    fn the_trailer_a_session_carries_is_the_one_its_check_accepts() {
        let trailer = agent_trailer("codex", "s1");

        assert_eq!(trailer, "Agent: codex via Torlek s1");
        assert!(is_agent_trailer_value(
            trailer.strip_prefix("Agent: ").unwrap()
        ));
    }

    fn sent(id: &str, is_root: bool, is_signed: bool) -> SentCommit {
        SentCommit {
            id: id.to_owned(),
            is_root,
            is_signed,
        }
    }

    #[test]
    fn a_signature_header_marks_its_commit_signed_and_a_message_line_does_not() {
        let raw = "commit aaaa\ntree t\nparent p\ngpgsig -----BEGIN SSH SIGNATURE-----\n x\n \
                   -----END SSH SIGNATURE-----\n\n    signed\n\ncommit bbbb\ntree t\nparent p\n\n    \
                   gpgsig in the message\n    parent in the message\n\ncommit cccc\ntree t\n\
                   gpgsig-sha256 x\n\n    m\n";

        assert_eq!(
            sent_in_raw(raw),
            [
                sent("aaaa", false, true),
                sent("bbbb", false, false),
                sent("cccc", true, true)
            ]
        );
    }

    #[test]
    fn the_rebase_starts_before_the_oldest_refused_commit() {
        let sent = [
            sent("c3", false, false),
            sent("c2", false, true),
            sent("c1", false, false),
        ];

        assert_eq!(
            rebase_upstream(&sent, &["c3".to_owned()], &["c2".to_owned()]),
            "c2^"
        );
    }

    #[test]
    fn the_rebase_of_a_refused_root_commit_starts_at_the_root() {
        let sent = [sent("c2", false, false), sent("c1", true, false)];

        assert_eq!(rebase_upstream(&sent, &["c1".to_owned()], &[]), "--root");
    }

    #[test]
    fn an_entry_with_one_well_formed_agent_value_among_others_is_attributed() {
        let listed = "aaaa\x01claude via Torlek s1\x02junk\0\nbbbb\x01\0\ncccc\x01junk\0\n";

        assert_eq!(unattributed_in(listed), ["bbbb", "cccc"]);
    }

    #[test]
    fn a_refusal_names_ten_commits_and_counts_the_rest() {
        let commits: Vec<String> = (0..12).map(|index| format!("c{index}")).collect();

        assert_eq!(
            with_verb(&commits, "is", "are"),
            "commits c0, c1, c2, c3, c4, c5, c6, c7, c8, c9 and 2 more are"
        );
    }
}
