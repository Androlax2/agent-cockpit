//! A provider with the baseline contract only, for tests (feature `fake`): it launches a new
//! or resumed conversation and reports `resume`, nothing else. Its agent is a shell script
//! that appends one line per launch to a log (session, conversation mode and id, first
//! message, pid, launch token), leaves an `agent-was-here` file in the session's directory
//! ([`crate::LaunchRequest::session_dir`]), then sleeps until stopped (`FAKE_AGENT_SLEEP`
//! seconds, 600 by default). A test may give it more capabilities ([`FakeProvider::reporting`]) and a
//! program its agent runs, in its own process tree, before it sleeps
//! ([`FakeProvider::running`]), or have it read the messages typed to it
//! ([`FakeProvider::reading_messages`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    Action, Activity, Capabilities, Capability, Conversation, ConversationId, Key, LaunchCommand,
    LaunchRequest, Provenance, Provider, ProviderError, ProviderName, TerminalInput,
};

const AGENT_SCRIPT: &str = r#"printf '%s %s %s %s pid=%s token=%s\n' "$COCKPIT_SESSION" "$1" "$2" "${3:-}" "$$" "$COCKPIT_LAUNCH_TOKEN" >> "$FAKE_AGENT_LOG"
: > "$FAKE_AGENT_SESSION_DIR/agent-was-here"
if [ -n "${FAKE_AGENT_RUN:-}" ]; then "$FAKE_AGENT_RUN"; fi
if [ -n "${FAKE_AGENT_MESSAGES:-}" ]; then
  while IFS= read -r line; do printf '%s %s\n' "$COCKPIT_SESSION" "$line" >> "$FAKE_AGENT_MESSAGES"; done
fi
exec sleep "${FAKE_AGENT_SLEEP:-600}""#;

pub struct FakeProvider {
    name: ProviderName,
    log: PathBuf,
    can_resume: bool,
    names_conversation: bool,
    reported: Capabilities,
    agent_program: Option<PathBuf>,
    messages: Option<PathBuf>,
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
            messages: None,
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

    /// Reports `submit`, as discovered: a message is typed as its text then Enter, while the
    /// agent waits or works, and refused while it needs the owner or its activity is unknown.
    /// Its agent appends each line it reads to `log` as `<session> <line>`
    /// ([`FakeProvider::messages`]).
    pub fn reading_messages(mut self, log: PathBuf) -> FakeProvider {
        self.messages = Some(log);
        self.reporting(Capability::Submit)
    }

    /// Every message the fake agents read into `log`, as (session, line), oldest first.
    pub fn messages(log: &Path) -> Vec<(String, String)> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(session, message)| (session.to_owned(), message.to_owned()))
            .collect()
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
            ("FAKE_AGENT_SESSION_DIR".into(), request.session_dir.into()),
        ];
        if let Some(program) = &self.agent_program {
            environment.push(("FAKE_AGENT_RUN".into(), program.clone().into()));
        }
        if let Some(log) = &self.messages {
            environment.push(("FAKE_AGENT_MESSAGES".into(), log.clone().into()));
        }
        Ok(LaunchCommand {
            program: "/bin/sh".into(),
            arguments,
            environment,
            conversation: self.names_conversation.then_some(conversation),
        })
    }

    fn input(
        &self,
        action: &Action<'_>,
        activity: Option<Activity>,
    ) -> Result<Vec<TerminalInput>, ProviderError> {
        self.capabilities().require(action.capability())?;
        let Action::Submit(message) = action else {
            return Err(ProviderError::unsupported(action.capability()));
        };
        match activity {
            Some(Activity::Waiting | Activity::Working) => Ok(vec![
                TerminalInput::Text((*message).to_owned()),
                TerminalInput::Key(Key::Enter),
            ]),
            Some(Activity::NeedsYou) => Err(ProviderError {
                reason: "a dialog is open in its terminal; answer it first".to_owned(),
            }),
            None => Err(ProviderError {
                reason: "its activity is unknown, so a message could answer a dialog".to_owned(),
            }),
        }
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
