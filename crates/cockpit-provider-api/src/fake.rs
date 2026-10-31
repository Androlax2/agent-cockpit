//! A provider with the baseline contract only, for tests (feature `fake`): it launches a new
//! or resumed conversation and reports `resume`, nothing else. Its agent is a shell script
//! that appends one line per launch to a log (session, conversation mode and id, first
//! message, pid, launch token), then sleeps until stopped (`FAKE_AGENT_SLEEP` seconds,
//! 600 by default).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    Capabilities, Capability, Conversation, ConversationId, LaunchCommand, LaunchRequest,
    Provenance, Provider, ProviderError, ProviderName,
};

const AGENT_SCRIPT: &str = r#"printf '%s %s %s %s pid=%s token=%s\n' "$COCKPIT_SESSION" "$1" "$2" "${3:-}" "$$" "$COCKPIT_LAUNCH_TOKEN" >> "$FAKE_AGENT_LOG"
exec sleep "${FAKE_AGENT_SLEEP:-600}""#;

pub struct FakeProvider {
    name: ProviderName,
    log: PathBuf,
    can_resume: bool,
    names_conversation: bool,
    conversations: AtomicUsize,
}

impl FakeProvider {
    /// A fake that resumes and names its conversations. New conversations are numbered
    /// after the launches already in `log`, so a restarted test daemon never reuses an id.
    pub fn new(name: ProviderName, log: PathBuf) -> FakeProvider {
        let logged = FakeProvider::launches(&log).len();
        FakeProvider {
            name,
            log,
            can_resume: true,
            names_conversation: true,
            conversations: AtomicUsize::new(logged),
        }
    }

    /// Without the `resume` capability.
    pub fn without_resume(mut self) -> FakeProvider {
        self.can_resume = false;
        self
    }

    /// Never names the conversation it launches, so it cannot be resumed.
    pub fn unnamed_conversations(mut self) -> FakeProvider {
        self.names_conversation = false;
        self
    }

    /// Every launch the fake agent logged in `log`, oldest first; none when it has none.
    pub fn launches(log: &Path) -> Vec<FakeLaunch> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(FakeLaunch::parse)
            .collect()
    }
}

impl Provider for FakeProvider {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn capabilities(&self) -> Capabilities {
        if self.can_resume {
            Capabilities::none().with(
                Capability::Resume,
                Provenance::Tested {
                    version: "fake".to_owned(),
                },
            )
        } else {
            Capabilities::none()
        }
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        request.require(&self.capabilities())?;
        let (mode, conversation) = match request.conversation {
            Conversation::New => {
                let number = self.conversations.fetch_add(1, Ordering::SeqCst) + 1;
                let id = ConversationId::new(format!("conversation-{number}"))
                    .expect("a conversation number is a valid id");
                ("new", id)
            }
            Conversation::Resume(id) => ("resume", id.clone()),
        };
        let mut arguments: Vec<OsString> = vec![
            "-c".into(),
            AGENT_SCRIPT.into(),
            "fake-agent".into(),
            mode.into(),
            conversation.as_str().into(),
        ];
        arguments.extend(request.first_message.map(OsString::from));
        Ok(LaunchCommand {
            program: "/bin/sh".into(),
            arguments,
            environment: vec![
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("FAKE_AGENT_LOG".into(), self.log.clone().into()),
            ],
            conversation: self.names_conversation.then_some(conversation),
        })
    }
}

/// One launch line of the fake agent's log. The first message is its first word only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeLaunch {
    pub session: String,
    /// `new` or `resume`.
    pub mode: String,
    pub conversation: String,
    pub message: String,
    pub pid: i32,
    pub token: String,
}

impl FakeLaunch {
    /// Panics on a line the fake agent did not write: the log is the test's own.
    fn parse(line: &str) -> FakeLaunch {
        let words: Vec<&str> = line.split(' ').collect();
        let field = |prefix: &str| {
            words
                .iter()
                .find_map(|word| word.strip_prefix(prefix))
                .unwrap_or_else(|| panic!("no {prefix} in the fake agent's line {line:?}"))
                .to_owned()
        };
        FakeLaunch {
            session: words[0].to_owned(),
            mode: words[1].to_owned(),
            conversation: words[2].to_owned(),
            message: words[3].to_owned(),
            pid: field("pid=")
                .parse()
                .unwrap_or_else(|_| panic!("a pid in {line:?}")),
            token: field("token="),
        }
    }
}
