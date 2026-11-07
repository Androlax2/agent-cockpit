//! A provider with the baseline contract only, for tests (feature `fake`): it launches a new
//! or resumed conversation and reports `resume`, nothing else. Its agent is a shell script
//! that appends one line per launch to a log (session, conversation mode and id, first
//! message, pid, launch token, model, effort and compaction threshold), leaves an `agent-was-here` file in the session's directory
//! ([`crate::LaunchRequest::session_dir`]), then sleeps until stopped (`FAKE_AGENT_SLEEP`
//! seconds, 600 by default). A test may give it more capabilities ([`FakeProvider::reporting`]) and a
//! program its agent runs, in its own process tree, before it sleeps
//! ([`FakeProvider::running`]), or have it read the messages typed to it
//! ([`FakeProvider::reading_messages`]), or exit at once ([`FakeProvider::exiting`]), or
//! answer quota readings ([`FakeProvider::reading_quota`]).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    Action, Activity, Capabilities, Capability, Conversation, ConversationId, Key, LaunchCommand,
    LaunchRequest, Provenance, Provider, ProviderError, ProviderName, QuotaReading, TerminalInput,
    TrustRequest, WorkspaceTrust,
};

/// The efforts the fake takes once it reports [`Capability::Effort`]; any other is refused.
pub const FAKE_EFFORT_LEVELS: [&str; 3] = ["low", "medium", "high"];

const AGENT_SCRIPT: &str = r#"printf '%s %s %s %s pid=%s token=%s model=%s effort=%s compaction-threshold=%s\n' "$TORLEK_SESSION" "$1" "$2" "${3:-}" "$$" "$TORLEK_LAUNCH_TOKEN" "${FAKE_AGENT_MODEL:-}" "${FAKE_AGENT_EFFORT:-}" "${FAKE_AGENT_COMPACTION_THRESHOLD:-}" >> "$FAKE_AGENT_LOG"
: > "$FAKE_AGENT_SESSION_DIR/agent-was-here"
if [ -n "${FAKE_AGENT_RUN:-}" ]; then "$FAKE_AGENT_RUN"; fi
if [ -n "${FAKE_AGENT_EXIT:-}" ]; then printf '%s\n' "$FAKE_AGENT_EXIT_OUTPUT" >&2; exit "$FAKE_AGENT_EXIT"; fi
if [ -n "${FAKE_AGENT_MESSAGES:-}" ]; then
  while IFS= read -r line; do printf '%s %s\n' "$TORLEK_SESSION" "$line" >> "$FAKE_AGENT_MESSAGES"; done
fi
exec sleep "${FAKE_AGENT_SLEEP:-600}""#;

type QuotaAnswers = VecDeque<Result<QuotaReading, ProviderError>>;

pub struct FakeProvider {
    name: ProviderName,
    log: PathBuf,
    can_resume: bool,
    names_conversation: bool,
    reported: Capabilities,
    agent_program: Option<PathBuf>,
    messages: Option<PathBuf>,
    /// The status its agent exits with at once, and the line it prints to stderr first.
    exits_with: Option<(u8, String)>,
    reports_at_launch: bool,
    trusted: Option<PathBuf>,
    /// The answers [`Provider::quota`] gives in turn, the last one again once they run out,
    /// and the log each ask is appended to.
    quota: Option<(Mutex<QuotaAnswers>, PathBuf)>,
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
            exits_with: None,
            reports_at_launch: false,
            trusted: None,
            quota: None,
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

    /// Reports `events`, and says its bridge reports at launch
    /// ([`Provider::reports_at_launch`]); its agent itself reports nothing unless a test
    /// reports for it.
    pub fn reporting_at_launch(mut self) -> FakeProvider {
        self.reports_at_launch = true;
        self.reporting(Capability::Events)
    }

    /// Its agent runs `program` (no arguments) after it logs its launch, then sleeps: a
    /// process of the agent's own tree, with its environment and working directory.
    pub fn running(mut self, program: PathBuf) -> FakeProvider {
        self.agent_program = Some(program);
        self
    }

    /// Its agent prints `line` to stderr and exits with `status` right after it logs its
    /// launch, as an agent or sandbox wrapper that refuses to start (#629).
    pub fn exiting(mut self, status: u8, line: &str) -> FakeProvider {
        self.exits_with = Some((status, line.to_owned()));
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

    /// Keeps a harness trust: each [`Provider::trust_workspace`] is recorded and appends
    /// `<workspace>\t<trusted checkout>` to `log` ([`FakeProvider::trusted`]).
    pub fn keeping_trust(mut self, log: PathBuf) -> FakeProvider {
        self.trusted = Some(log);
        self
    }

    /// Reports `quota`, as discovered: each [`Provider::quota`] gives the next of `answers`,
    /// the last one again once they run out, and appends `<unix milliseconds>\t<home>` to
    /// `log` ([`FakeProvider::quota_asks`]).
    pub fn reading_quota(
        mut self,
        answers: Vec<Result<QuotaReading, ProviderError>>,
        log: PathBuf,
    ) -> FakeProvider {
        assert!(!answers.is_empty(), "a fake reading quota needs an answer");
        self.quota = Some((Mutex::new(answers.into()), log));
        self.reporting(Capability::Quota)
    }

    /// Every quota ask the fake recorded into `log`, as (unix milliseconds, home), oldest
    /// first.
    pub fn quota_asks(log: &Path) -> Vec<(u128, PathBuf)> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(at, home)| (at.parse().expect("milliseconds"), home.into()))
            .collect()
    }

    /// Every trust the fake recorded into `log`, as (workspace, trusted checkout), oldest
    /// first.
    pub fn trusted(log: &Path) -> Vec<(PathBuf, PathBuf)> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(workspace, trusted)| (workspace.into(), trusted.into()))
            .collect()
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

    fn quota(&self, home: &Path) -> Result<QuotaReading, ProviderError> {
        let Some((answers, log)) = &self.quota else {
            return Err(ProviderError::unsupported(Capability::Quota));
        };
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_millis();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .expect("the quota log opens");
        writeln!(file, "{at}\t{}", home.display()).expect("the quota log takes a line");
        let mut answers = answers.lock().expect("the quota answers are not poisoned");
        if answers.len() > 1 {
            return answers.pop_front().expect("more than one answer");
        }
        answers.front().cloned().expect("one answer left")
    }

    fn reports_at_launch(&self) -> bool {
        self.reports_at_launch
    }

    fn trust_workspace(&self, request: &TrustRequest<'_>) -> Result<WorkspaceTrust, ProviderError> {
        let Some(log) = &self.trusted else {
            return Ok(WorkspaceTrust::NotKept);
        };
        let line = format!(
            "{}\t{}\n",
            request.workspace.display(),
            request.trusted_checkout.display()
        );
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()))
            .map_err(|error| ProviderError {
                reason: format!("the fake's trust log {}: {error}", log.display()),
            })?;
        Ok(WorkspaceTrust::Recorded)
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        request.require(&self.capabilities())?;
        if let Some(effort) = request.effort
            && !FAKE_EFFORT_LEVELS.contains(&effort)
        {
            return Err(ProviderError {
                reason: format!(
                    "effort {effort:?} is not one of {}",
                    FAKE_EFFORT_LEVELS.join(", ")
                ),
            });
        }
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
        if let Some(model) = request.model {
            environment.push(("FAKE_AGENT_MODEL".into(), model.into()));
        }
        if let Some(effort) = request.effort {
            environment.push(("FAKE_AGENT_EFFORT".into(), effort.into()));
        }
        if let Some(threshold) = request.compaction_threshold {
            environment.push(("FAKE_AGENT_COMPACTION_THRESHOLD".into(), threshold.into()));
        }
        if let Some(program) = &self.agent_program {
            environment.push(("FAKE_AGENT_RUN".into(), program.clone().into()));
        }
        if let Some(log) = &self.messages {
            environment.push(("FAKE_AGENT_MESSAGES".into(), log.clone().into()));
        }
        if let Some((status, line)) = &self.exits_with {
            environment.push(("FAKE_AGENT_EXIT".into(), status.to_string().into()));
            environment.push(("FAKE_AGENT_EXIT_OUTPUT".into(), line.into()));
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
    /// The model it was launched on, when one was chosen.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The compaction threshold it was launched with, when one was chosen (#783).
    pub compaction_threshold: Option<String>,
}

impl FakeLaunch {
    /// Panics on a line the fake agent did not write: the log is the test's own.
    fn parse(line: &str) -> FakeLaunch {
        let words: Vec<&str> = line.split(' ').collect();
        let value = |prefix: &str| words.iter().find_map(|word| word.strip_prefix(prefix));
        let field = |prefix: &str| {
            value(prefix)
                .unwrap_or_else(|| panic!("no {prefix} in the fake agent's line {line:?}"))
                .to_owned()
        };
        // Last on the line, after a first message that may hold the same words. Lines written
        // before #701 have neither; an empty one was not chosen.
        let chosen = |prefix: &str| {
            words
                .iter()
                .rev()
                .find_map(|word| word.strip_prefix(prefix))
                .filter(|chosen| !chosen.is_empty())
                .map(str::to_owned)
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
            model: chosen("model="),
            effort: chosen("effort="),
            compaction_threshold: chosen("compaction-threshold="),
        }
    }
}
