use std::fmt;

use crate::Error;

/// A tmux session name Cockpit chose: 1 to 64 of `A-Z a-z 0-9 _ -`, not starting with `-`.
///
/// tmux reads `:` and `.` in a target as window and pane separators and a leading `=` as
/// "exact match", so names outside this set could address another session. Every command
/// targets a session as `=name:`. Without the `=`, tmux matches a prefix and `agent-1`
/// would reach `agent-12`; without the `:`, a command taking a window target (`list-panes
/// -s`) falls back to matching the name as a window and reaches `agent-12` too.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionName(String);

const MAX_SESSION_NAME_BYTES: usize = 64;

impl SessionName {
    pub fn new(name: &str) -> Result<SessionName, Error> {
        let is_valid = !name.is_empty()
            && name.len() <= MAX_SESSION_NAME_BYTES
            && !name.starts_with('-')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if !is_valid {
            return Err(Error::InvalidArgument {
                what: "tmux session name (1-64 of A-Z a-z 0-9 _ -, not starting with -)",
                value: name.to_string(),
            });
        }
        Ok(SessionName(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The exact-match target `=name:`, valid wherever tmux takes a session or a window.
    pub(crate) fn target(&self) -> String {
        format!("={}:", self.0)
    }
}

impl fmt::Display for SessionName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A pane's server-wide identity, `%` followed by digits. tmux never reuses one while its
/// server runs, so it stays valid across a daemon restart.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(String);

impl PaneId {
    pub fn parse(printed: &str) -> Result<PaneId, Error> {
        let is_valid = printed
            .strip_prefix('%')
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
        if !is_valid {
            return Err(Error::InvalidArgument {
                what: "tmux pane id (% followed by digits)",
                value: printed.to_string(),
            });
        }
        Ok(PaneId(printed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PaneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
