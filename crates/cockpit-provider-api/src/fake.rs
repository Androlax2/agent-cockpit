//! A provider with the baseline contract only, for tests (feature `fake`): it launches a new
//! or resumed conversation and reports `resume`, nothing else. Its agent is a shell script
//! that appends one line per launch to a log (session, conversation mode and id, first
//! message, pid, launch token), then sleeps until stopped (`FAKE_AGENT_SLEEP` seconds,
//! 600 by default). A test may give it more capabilities ([`FakeProvider::reporting`]) and a
//! program its agent runs, in its own process tree, before it sleeps
//! ([`FakeProvider::running`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    Capabilities, Capability, Conversation, ConversationId, LaunchCommand, LaunchRequest,
    Provenance, Provider, ProviderError, ProviderName,
};

const AGENT_SCRIPT: &str = r#"printf '%s %s %s %s pid=%s token=%s\n' "$COCKPIT_SESSION" "$1" "$2" "${3:-}" "$$" "$COCKPIT_LAUNCH_TOKEN" >> "$FAKE_AGENT_LOG"
if [ -n "${FAKE_AGENT_RUN:-}" ]; then "$FAKE_AGENT_RUN"; fi
exec sleep "${FAKE_AGENT_SLEEP:-600}""#;

pub struct FakeProvider {
    name: ProviderName,
    log: PathBuf,
    can_resume: bool,
    names_conversation: bool,
    reported: Capabilities,
    agent_program: Option<PathBuf>,
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
            reported: Capabilities::none(),
            agent_program: None,
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

    /// Also reports `capability`, as discovered.
    pub fn reporting(mut self, capability: Capability) -> FakeProvider {
        self.reported = self.reported.with(capability, Provenance::Discovered);
        self
    }

    /// Its agent runs `program` (no arguments) after it logs its launch, then sleeps: a
    /// process of the agent's own tree, with its environment and working directory.
    pub fn running(mut self, program: PathBuf) -> FakeProvider {
        self.agent_program = Some(program);
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
            self.reported.clone().with(
                Capability::Resume,
                Provenance::Tested {
                    version: "fake".to_owned(),
                },
            )
        } else {
            self.reported.clone()
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
        let mut environment: Vec<(OsString, OsString)> = vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("FAKE_AGENT_LOG".into(), self.log.clone().into()),
        ];
        if let Some(program) = &self.agent_program {
            environment.push(("FAKE_AGENT_RUN".into(), program.clone().into()));
        }
        Ok(LaunchCommand {
            program: "/bin/sh".into(),
            arguments,
            environment,
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
