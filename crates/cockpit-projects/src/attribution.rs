//! Whether every commit an agent's push sends says which session made it, and carries no
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

/// Whether `value`, an `Agent` trailer's value, names a provider and a session.
fn is_agent_trailer_value(value: &str) -> bool {
    let is_word = |word: &str| !word.is_empty() && !word.contains(char::is_whitespace);
    value
        .split_once(TRAILER_JOIN)
        .is_some_and(|(provider, session)| is_word(provider) && is_word(session))
}

/// Refuses the push when a commit it sends lacks an `Agent` trailer naming its session, or is
/// signed.
///
/// What it sends is what `commit` reaches and neither of `on_remote` does, read as the push
/// reads the checkout: replace refs off, as `pack-objects` has them, and grafts and a shallow
/// boundary as they are, since a commit they hide from this walk is hidden from the pack too,
/// and the remote then refuses the push for lacking it.
pub(crate) fn require_attributed(
    git: &Git,
    checkout: &Path,
    on_remote: &[&str],
    commit: &str,
) -> Result<(), Error> {
    let unattributed = unattributed_commits(git, checkout, on_remote, commit)?;
    let signed = signed_commits(git, checkout, on_remote, commit)?;
    if unattributed.is_empty() && signed.is_empty() {
        return Ok(());
    }
    let base = on_remote.first().copied().unwrap_or("the root");
    let mut problems = Vec::new();
    if !unattributed.is_empty() {
        problems.push(format!(
            "{} {} no `{TRAILER_KEY}: <provider>{TRAILER_JOIN}<session>` trailer naming \
             {}session; commit with `git commit --trailer \"${COMMIT_TRAILER_VARIABLE}\"`, and \
             add it to {} with `git rebase --exec 'git commit --amend --no-edit --trailer \
             \"${COMMIT_TRAILER_VARIABLE}\"' {base}`",
            named(&unattributed),
            if unattributed.len() == 1 {
                "carries"
            } else {
                "carry"
            },
            if unattributed.len() == 1 {
                "its "
            } else {
                "their "
            },
            if unattributed.len() == 1 {
                "that commit"
            } else {
                "those commits"
            },
        ));
    }
    if !signed.is_empty() {
        problems.push(format!(
            "{} {} signed, and an agent's commit carries no signature; rewrite {} with \
             `git commit --amend --no-edit --no-gpg-sign`",
            named(&signed),
            if signed.len() == 1 { "is" } else { "are" },
            if signed.len() == 1 { "it" } else { "them" },
        ));
    }
    Err(Error::Refused {
        reason: format!(
            "an agent's push sends only commits it attributes: {}",
            problems.join("; ")
        ),
    })
}

/// `commits` as a refusal names them: `commit A` or `commits A, B and N more`.
fn named(commits: &[String]) -> String {
    let listed = commits[..commits.len().min(NAMED_COMMITS)].join(", ");
    match commits.len() {
        1 => format!("commit {listed}"),
        count if count > NAMED_COMMITS => {
            format!("commits {listed} and {} more", count - NAMED_COMMITS)
        }
        _ => format!("commits {listed}"),
    }
}

/// The range's arguments: `commit` and every commit the remote holds, negated. Each is a full
/// object id, so none reads as an option or a range.
fn range(on_remote: &[&str], commit: &str) -> Vec<String> {
    std::iter::once(commit.to_owned())
        .chain(on_remote.iter().map(|tip| format!("^{tip}")))
        .collect()
}

fn rev_list(
    git: &Git,
    checkout: &Path,
    format: &str,
    on_remote: &[&str],
    commit: &str,
) -> Result<String, Error> {
    let range = range(on_remote, commit);
    let mut arguments = vec![
        arg("--no-replace-objects"),
        arg("rev-list"),
        arg("--no-commit-header"),
        arg(format),
        arg("--end-of-options"),
    ];
    arguments.extend(range.iter().map(arg));
    git.run(checkout, &arguments)
}

/// The range's commits whose `Agent` trailers (as Git parses a message's trailers) include
/// none that names a provider and a session.
fn unattributed_commits(
    git: &Git,
    checkout: &Path,
    on_remote: &[&str],
    commit: &str,
) -> Result<Vec<String>, Error> {
    let listed = rev_list(
        git,
        checkout,
        "--format=%H%x01%(trailers:key=Agent,valueonly,unfold,separator=%x02)%x00",
        on_remote,
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

/// The range's commits whose header carries a signature (`gpgsig`, or `gpgsig-sha256` in a
/// SHA-256 repository), read from the raw commit, so no signature program runs.
fn signed_commits(
    git: &Git,
    checkout: &Path,
    on_remote: &[&str],
    commit: &str,
) -> Result<Vec<String>, Error> {
    let raw = rev_list(git, checkout, "--pretty=raw", on_remote, commit)?;
    Ok(signed_in_raw(&raw))
}

/// The ids `rev-list --pretty=raw` lists with a signature header. Each commit starts at a
/// `commit <id>` line, its headers follow up to a blank line, and its message lines are
/// indented, so no message line reads as either.
fn signed_in_raw(raw: &str) -> Vec<String> {
    let mut signed = Vec::new();
    let mut current: Option<&str> = None;
    let mut in_headers = false;
    for line in raw.lines() {
        if let Some(id) = line.strip_prefix("commit ") {
            current = Some(id.trim());
            in_headers = true;
            continue;
        }
        if line.is_empty() {
            in_headers = false;
            continue;
        }
        let is_signature = line.starts_with("gpgsig ") || line.starts_with("gpgsig-sha256 ");
        if in_headers
            && is_signature
            && let Some(id) = current.take()
        {
            signed.push(id.to_owned());
        }
    }
    signed
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

    #[test]
    fn a_signature_header_marks_its_commit_signed_and_a_message_line_does_not() {
        let raw = "commit aaaa\ntree t\ngpgsig -----BEGIN SSH SIGNATURE-----\n x\n \
                   -----END SSH SIGNATURE-----\n\n    signed\n\ncommit bbbb\ntree t\n\n    \
                   gpgsig in the message\n\ncommit cccc\ntree t\ngpgsig-sha256 x\n\n    m\n";

        assert_eq!(signed_in_raw(raw), ["aaaa", "cccc"]);
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
            named(&commits),
            "commits c0, c1, c2, c3, c4, c5, c6, c7, c8, c9 and 2 more"
        );
    }
}
