//! The branches beyond its launch branch a project's agents may push (#155, decision of
//! 2026-11-09, option B): patterns the owner adds per project, none by default.

use cockpit_turso::sql::{Connection, params};
use cockpit_turso::{Database, Migration};

use crate::Error;
use crate::store::{now, project_named};

/// #155: the owner's push-branch patterns, per project.
pub(crate) const MIGRATION: Migration = Migration {
    version: 6,
    sql: "CREATE TABLE project_push_branches (
        project_id INTEGER NOT NULL,
        pattern TEXT NOT NULL CHECK (pattern <> ''),
        added_at INTEGER NOT NULL,
        PRIMARY KEY (project_id, pattern)
    );",
};

/// A branch name pattern: `/`-separated segments where `*` matches any run of characters
/// within one segment and a `**` segment matches any number of segments, none included.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BranchPattern(String);

impl BranchPattern {
    /// Refuses a pattern no branch name could match or that reaches outside branches: an
    /// empty segment, `..`, a segment starting with `.` or `-`, a `.lock` ending, or a
    /// character Git or a shell treats specially.
    pub fn new(pattern: &str) -> Result<BranchPattern, Error> {
        let refuse = |reason: &str| {
            Err(Error::InvalidPushBranch {
                pattern: pattern.to_owned(),
                reason: reason.to_owned(),
            })
        };
        if pattern.starts_with("refs/") {
            return refuse("name the branch alone, without `refs/`");
        }
        if !pattern
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./*".contains(&byte))
        {
            return refuse("use letters, digits, `-`, `_`, `.`, `/` and `*` only");
        }
        for segment in pattern.split('/') {
            if segment.is_empty() {
                return refuse("a `/` starts, ends or doubles the pattern");
            }
            if segment.starts_with('.') || segment.starts_with('-') {
                return refuse("a segment starts with `.` or `-`");
            }
            if segment.ends_with(".lock") || segment.contains("..") {
                return refuse("a segment holds `..` or ends with `.lock`");
            }
            if segment.contains("**") && segment != "**" {
                return refuse("`**` stands alone between `/`");
            }
        }
        Ok(BranchPattern(pattern.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `branch` is one this pattern names.
    pub fn matches(&self, branch: &str) -> bool {
        let pattern: Vec<&str> = self.0.split('/').collect();
        let branch: Vec<&str> = branch.split('/').collect();
        segments_match(&pattern, &branch)
    }
}

impl std::fmt::Display for BranchPattern {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn segments_match(pattern: &[&str], branch: &[&str]) -> bool {
    match pattern.split_first() {
        None => branch.is_empty(),
        Some((&"**", rest)) => {
            (0..=branch.len()).any(|taken| segments_match(rest, &branch[taken..]))
        }
        Some((segment, rest)) => match branch.split_first() {
            Some((name, branch_rest)) => {
                segment_matches(segment.as_bytes(), name.as_bytes())
                    && segments_match(rest, branch_rest)
            }
            None => false,
        },
    }
}

fn segment_matches(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|taken| segment_matches(rest, &name[taken..])),
        Some((byte, rest)) => name.first() == Some(byte) && segment_matches(rest, &name[1..]),
    }
}

/// Adds `pattern` to project `project`'s push branches; `false` when it was there already.
pub async fn add_push_branch(
    database: &Database,
    project: &str,
    pattern: &BranchPattern,
) -> Result<bool, Error> {
    let write = database.write().await?;
    let project = project_named(&write, project).await?;
    let added = write
        .execute(
            "INSERT INTO project_push_branches (project_id, pattern, added_at)
             VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            params![project.id.0, pattern.as_str(), now()],
        )
        .await?;
    write.commit().await?;
    Ok(added == 1)
}

/// Removes `pattern` from project `project`'s push branches; `false` when it was not there.
pub async fn remove_push_branch(
    database: &Database,
    project: &str,
    pattern: &BranchPattern,
) -> Result<bool, Error> {
    let write = database.write().await?;
    let project = project_named(&write, project).await?;
    let removed = write
        .execute(
            "DELETE FROM project_push_branches WHERE project_id = ?1 AND pattern = ?2",
            params![project.id.0, pattern.as_str()],
        )
        .await?;
    write.commit().await?;
    Ok(removed == 1)
}

/// Project `project`'s push-branch patterns, in name order.
pub async fn push_branches(
    connection: &Connection,
    project: &str,
) -> Result<Vec<BranchPattern>, Error> {
    let project = project_named(connection, project).await?;
    let mut rows = connection
        .query(
            "SELECT pattern FROM project_push_branches WHERE project_id = ?1 ORDER BY pattern",
            [project.id.0],
        )
        .await?;
    let mut patterns = Vec::new();
    while let Some(row) = rows.next().await? {
        let stored: String = row.get(0)?;
        patterns.push(
            BranchPattern::new(&stored).map_err(|error| Error::InvalidStoredRow {
                reason: error.to_string(),
            })?,
        );
    }
    Ok(patterns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_names_the_branches_its_segments_match() {
        for (pattern, branch, is_match) in [
            ("main", "main", true),
            ("main", "mainline", false),
            ("agent/*", "agent/fix", true),
            ("agent/*", "agent/fix/deep", false),
            ("agent/*", "agent", false),
            ("agent/**", "agent/fix/deep", true),
            ("agent/**", "agent", true),
            ("**/wip", "a/b/wip", true),
            ("**/wip", "wip", true),
            ("greenfield/155-*", "greenfield/155-push", true),
            ("greenfield/155-*", "greenfield/156-push", false),
            ("*-draft", "x-draft", true),
            ("*", "two/segments", false),
        ] {
            let matched = BranchPattern::new(pattern).unwrap().matches(branch);
            assert_eq!(matched, is_match, "{pattern} against {branch}");
        }
    }

    #[test]
    fn a_pattern_no_branch_could_take_or_reaching_outside_branches_is_refused() {
        for pattern in [
            "",
            "refs/heads/main",
            "agent//x",
            "/agent",
            "agent/",
            "../main",
            "agent/.hidden",
            "-flag",
            "main.lock",
            "a**",
            "agent/[ab]",
            "agent x",
            "agent?",
        ] {
            assert!(
                matches!(
                    BranchPattern::new(pattern),
                    Err(Error::InvalidPushBranch { .. })
                ),
                "{pattern:?}"
            );
        }
    }
}
