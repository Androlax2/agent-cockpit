//! `cockpit init`, and `cockpit` alone until the setup is done (#100): the first-run steps,
//! each with its state and the next action, computed again at every run from the readiness
//! report, the owner's configuration, the registered projects and the sessions.
//!
//! It writes nothing: each step's action is a verb the owner runs or a line the owner writes,
//! so the checklist is the same whether it is followed once or interrupted and resumed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cockpit_client::ClientError;
use cockpit_config::{AgentSandbox, EffectiveConfig, Source};
use cockpit_protocol::{
    Command, ProjectView, ReadinessStatusView, Reply, RequirementView, SessionView,
};

use crate::daemon;

/// The daemon probes Docker only for a project, which `init` does not name; the rest is reads.
const INIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// `cockpit init`: the checklist, whether or not the setup is done.
pub(crate) fn init(arguments: &[OsString]) -> Result<ExitCode, String> {
    if let Some(argument) = arguments.first() {
        return Err(format!(
            "init takes no arguments, not {}",
            argument.to_string_lossy()
        ));
    }
    let steps = match gather() {
        Ok(facts) => steps(&facts),
        Err(code) => return Ok(code),
    };
    render(&steps);
    Ok(ExitCode::SUCCESS)
}

/// `cockpit` alone: the checklist while a step is open, the usage once the setup is done.
pub(crate) fn first_run() -> ExitCode {
    let steps = match gather() {
        Ok(facts) => steps(&facts),
        Err(code) => return code,
    };
    if is_complete(&steps) {
        return crate::usage_error(None);
    }
    render(&steps);
    ExitCode::SUCCESS
}

struct Facts {
    config: Result<EffectiveConfig, String>,
    /// `Err` holds why no daemon answered.
    daemon: Result<DaemonFacts, String>,
}

struct DaemonFacts {
    requirements: Vec<RequirementView>,
    projects: Vec<ProjectView>,
    /// Running and ended ones: one launched session is enough to have started.
    sessions: Vec<SessionView>,
}

fn gather() -> Result<Facts, ExitCode> {
    let environment = cockpit_config::Environment::from_process();
    let config = cockpit_config::load(&environment, None, &[]).map_err(|error| error.to_string());
    let daemon = match daemon_socket(&environment) {
        Ok(socket) => ask_daemon(&socket)?,
        Err(reason) => Err(reason),
    };
    Ok(Facts { config, daemon })
}

/// Unlike the other verbs', a socket that cannot be located is a fact to report in the
/// checklist, next to the configuration error that explains it.
fn daemon_socket(environment: &cockpit_config::Environment) -> Result<PathBuf, String> {
    environment
        .home_dir()
        .and_then(|home| cockpit_config::Layout::resolve(environment, &home).socket_dir())
        .map(|dir| dir.join(cockpit_config::DAEMON_SOCKET))
        .map_err(|error| format!("no socket to reach it: {error}"))
}

/// The daemon's facts, `Err` with why no daemon answers, or exit 1 when one answered with
/// an error.
fn ask_daemon(socket: &Path) -> Result<Result<DaemonFacts, String>, ExitCode> {
    let commands = vec![
        Command::Readiness { project: None },
        Command::ProjectList,
        Command::SessionList { ended: true },
    ];
    match daemon::exchange_all(socket, commands, INIT_DEADLINE)? {
        Ok(replies) => Ok(Ok(daemon_facts(replies)?)),
        Err(error @ ClientError::Connect { .. }) => Ok(Err(error.to_string())),
        Err(error) => Err(daemon::failed(&error)),
    }
}

fn daemon_facts(replies: Vec<Reply>) -> Result<DaemonFacts, ExitCode> {
    let mut replies = replies.into_iter();
    let requirements = match replies.next() {
        Some(Reply::Readiness { requirements }) => requirements,
        other => return Err(unexpected(other)),
    };
    let projects = match replies.next() {
        Some(Reply::Projects { projects }) => projects,
        other => return Err(unexpected(other)),
    };
    let sessions = match replies.next() {
        Some(Reply::Sessions { sessions }) => sessions,
        other => return Err(unexpected(other)),
    };
    Ok(DaemonFacts {
        requirements,
        projects,
        sessions,
    })
}

fn unexpected(reply: Option<Reply>) -> ExitCode {
    match reply {
        Some(reply) => daemon::unexpected(&reply),
        None => daemon::fail("the daemon answered fewer requests than it was asked"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Done,
    /// The owner's action comes next, or after an earlier open step.
    Open,
    /// An earlier step must be done before this one can be assessed.
    Waiting,
    /// Cockpit cannot tell; the step names how to check by hand and does not hold the setup.
    Unknown,
}

impl State {
    fn label(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Open => "todo",
            Self::Waiting => "waits",
            Self::Unknown => "unknown",
        }
    }
}

struct Step {
    title: &'static str,
    state: State,
    lines: Vec<String>,
    next: Option<String>,
}

impl Step {
    fn waiting(title: &'static str, on: &str) -> Step {
        Step {
            title,
            state: State::Waiting,
            lines: vec![format!("after {on}")],
            next: None,
        }
    }
}

fn is_complete(steps: &[Step]) -> bool {
    steps
        .iter()
        .all(|step| matches!(step.state, State::Done | State::Unknown))
}

fn steps(facts: &Facts) -> Vec<Step> {
    let readiness = readiness_step(facts);
    let sandbox = sandbox_step(&facts.config);
    let Ok(daemon) = &facts.daemon else {
        return vec![
            readiness,
            sandbox,
            Step::waiting(PROVIDER, "the daemon runs"),
            Step::waiting(SIGN_IN, "the daemon runs"),
            Step::waiting(PROJECT, "the daemon runs"),
            Step::waiting(SESSION, "the daemon runs"),
        ];
    };
    let available = available_providers(&daemon.requirements);
    let provider = provider_step(&daemon.requirements, &available);
    let sign_in = if available.is_empty() {
        Step::waiting(SIGN_IN, "a provider is available")
    } else {
        sign_in_step(&daemon.requirements, &available)
    };
    let project = project_step(&daemon.projects);
    let session = session_step(daemon, &available, readiness.state == State::Done);
    vec![readiness, sandbox, provider, sign_in, project, session]
}

const READINESS: &str = "Host readiness";
const SANDBOX: &str = "Sandbox and approvals";
const PROVIDER: &str = "Pick a provider";
const SIGN_IN: &str = "Sign in to the provider";
const PROJECT: &str = "Add a project";
const SESSION: &str = "Start a session";

/// The host's requirements outside the provider steps; done when none blocks. Those that
/// are not available are listed either way, since `needs-authorization` and `unknown` are
/// the owner's to know about.
fn readiness_step(facts: &Facts) -> Step {
    let mut lines = Vec::new();
    let mut next = None;
    let mut blocks = false;
    if let Err(error) = &facts.config {
        blocks = true;
        lines.push(format!("config: {error}"));
        next = Some(
            "fix what is named here; `cockpit config show` reads the configuration again"
                .to_owned(),
        );
    }
    match &facts.daemon {
        Err(unreachable) => {
            blocks = true;
            lines.push(format!("daemon: not running ({unreachable})"));
            next.get_or_insert_with(|| {
                "start `cockpit-daemon` in another terminal (it stays in the foreground)".to_owned()
            });
        }
        Ok(daemon) => {
            for requirement in daemon
                .requirements
                .iter()
                .filter(|requirement| !is_provider(requirement))
                .filter(|requirement| requirement.status != ReadinessStatusView::Available)
            {
                lines.push(requirement_line(requirement));
                if requirement.status.blocks() {
                    blocks = true;
                    if next.is_none() {
                        next.clone_from(&requirement.next);
                    }
                }
            }
        }
    }
    if !blocks && lines.is_empty() {
        lines.push("tmux, the sandbox engine and resource controls are ready".to_owned());
    }
    lines.push("`cockpit doctor` shows every requirement".to_owned());
    Step {
        title: READINESS,
        state: if blocks { State::Open } else { State::Done },
        lines,
        next,
    }
}

/// What the sandbox restricts, what needs the owner and how approvals work; done once the
/// owner wrote `agents.sandbox` in their own file, which is the recorded consent.
fn sandbox_step(config: &Result<EffectiveConfig, String>) -> Step {
    let Ok(config) = config else {
        return Step::waiting(SANDBOX, "the configuration reads");
    };
    let setting = &config.agents_sandbox;
    let mut lines = vec![
        "restricted: each agent runs in the greywall sandbox; it writes only inside its \
         own checkout, cannot read Cockpit's own files, and reaches the network through \
         greyproxy"
            .to_owned(),
        "needs you: a command outside the sandbox (`cockpit ask run`) or a step only a \
         person can do (`cockpit ask guide`); an agent cannot approve its own request"
            .to_owned(),
        "approvals: `cockpit approval list` (or the approval pane of `cockpit tui`) shows the \
         exact command, its directory and the content it runs; an approval runs that command \
         once, a decline tells the agent"
            .to_owned(),
        format!(
            "desktop: agents get your display and session bus while agents.desktop_session = \
             \"shared\" (now \"{}\")",
            config.agents_desktop_session.value.as_str()
        ),
    ];
    if let Source::UserFile { path, line } = &setting.source {
        lines.push(match setting.value {
            AgentSandbox::Required => format!("you chose the sandbox ({}:{line})", path.display()),
            AgentSandbox::Off => format!(
                "you chose to run agents UNSANDBOXED, with your rights ({}:{line})",
                path.display()
            ),
        });
        return Step {
            title: SANDBOX,
            state: State::Done,
            lines,
            next: None,
        };
    }
    Step {
        title: SANDBOX,
        state: State::Open,
        lines,
        next: Some(format!(
            "to accept, write `[agents]` and `sandbox = \"required\"` in {} \
             (`sandbox = \"off\"` runs agents unsandboxed, with your rights)",
            config.user_file.path.display()
        )),
    }
}

/// `provider <name>` and the `providers` requirement; sign-ins are the next step's.
fn is_provider(requirement: &RequirementView) -> bool {
    requirement.name == "providers" || requirement.name.starts_with("provider ")
}

fn provider_name(requirement: &RequirementView) -> Option<&str> {
    requirement
        .name
        .strip_prefix("provider ")
        .filter(|name| !name.ends_with(" sign-in"))
}

fn available_providers(requirements: &[RequirementView]) -> Vec<&str> {
    requirements
        .iter()
        .filter(|requirement| requirement.status == ReadinessStatusView::Available)
        .filter_map(provider_name)
        .collect()
}

fn provider_step(requirements: &[RequirementView], available: &[&str]) -> Step {
    let missing: Vec<&RequirementView> = requirements
        .iter()
        .filter(|requirement| {
            requirement.name == "providers" || provider_name(requirement).is_some()
        })
        .filter(|requirement| requirement.status != ReadinessStatusView::Available)
        .collect();
    let mut lines: Vec<String> = missing
        .iter()
        .map(|requirement| requirement_line(requirement))
        .collect();
    match available {
        [] => Step {
            title: PROVIDER,
            state: State::Open,
            next: missing
                .iter()
                .find_map(|requirement| requirement.next.clone())
                .or_else(|| {
                    Some("install a supported agent (claude), then restart the daemon".to_owned())
                }),
            lines,
        },
        [only] => {
            lines.insert(0, format!("sessions start with {only}"));
            done(PROVIDER, lines)
        }
        several => {
            lines.insert(
                0,
                format!(
                    "available: {}; each session picks one with `--agent NAME`",
                    several.join(", ")
                ),
            );
            done(PROVIDER, lines)
        }
    }
}

/// Open while a provider needs a sign-in; `unknown` (#443: no adapter reports it yet)
/// names how to check by hand without holding the setup.
fn sign_in_step(requirements: &[RequirementView], available: &[&str]) -> Step {
    let sign_ins: Vec<&RequirementView> = available
        .iter()
        .filter_map(|provider| {
            let name = format!("provider {provider} sign-in");
            requirements
                .iter()
                .find(|requirement| requirement.name == name)
        })
        .collect();
    let lines = sign_ins
        .iter()
        .map(|requirement| requirement_line(requirement))
        .collect();
    let first_with = |status: fn(ReadinessStatusView) -> bool| {
        sign_ins
            .iter()
            .find(|requirement| status(requirement.status))
            .and_then(|requirement| requirement.next.clone())
    };
    if let Some(next) = first_with(|status| {
        !matches!(
            status,
            ReadinessStatusView::Available | ReadinessStatusView::Unknown
        )
    }) {
        return Step {
            title: SIGN_IN,
            state: State::Open,
            lines,
            next: Some(next),
        };
    }
    if let Some(next) = first_with(|status| status == ReadinessStatusView::Unknown) {
        return Step {
            title: SIGN_IN,
            state: State::Unknown,
            lines,
            next: Some(next),
        };
    }
    done(SIGN_IN, lines)
}

fn project_step(projects: &[ProjectView]) -> Step {
    if projects.is_empty() {
        return Step {
            title: PROJECT,
            state: State::Open,
            lines: vec!["no project is registered".to_owned()],
            next: Some(
                "`cockpit project add DIR` registers the Git repository agents work in".to_owned(),
            ),
        };
    }
    done(
        PROJECT,
        projects
            .iter()
            .map(|project| format!("{} ({})", project.name, project.root))
            .collect(),
    )
}

fn session_step(daemon: &DaemonFacts, available: &[&str], is_host_ready: bool) -> Step {
    if !daemon.sessions.is_empty() {
        return done(
            SESSION,
            vec![format!(
                "{} session(s) started; `cockpit tui` shows them",
                daemon.sessions.len()
            )],
        );
    }
    let Some(project) = daemon.projects.first() else {
        return Step::waiting(SESSION, "a project is added");
    };
    if available.is_empty() {
        return Step::waiting(SESSION, "a provider is available");
    }
    if !is_host_ready {
        return Step::waiting(SESSION, "nothing in the host readiness blocks");
    }
    let agent = match available {
        [_] => String::new(),
        several => format!(" --agent {}", several[0]),
    };
    Step {
        title: SESSION,
        state: State::Open,
        lines: vec!["no session has started yet".to_owned()],
        next: Some(format!(
            "`cockpit session new {}{agent}` starts an agent there; \
             `cockpit tui` shows it",
            shell_word(&project.root)
        )),
    }
}

fn done(title: &'static str, lines: Vec<String>) -> Step {
    Step {
        title,
        state: State::Done,
        lines,
        next: None,
    }
}

fn requirement_line(requirement: &RequirementView) -> String {
    format!(
        "{}: {} ({})",
        requirement.name,
        requirement.detail,
        requirement.status.as_str()
    )
}

/// `word` as one shell word: quoted only when it holds something a shell would split or
/// expand.
fn shell_word(word: &str) -> String {
    let is_plain = !word.is_empty()
        && word
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/._-+:,@%=".contains(character));
    if is_plain {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

fn render(steps: &[Step]) {
    let finished = steps
        .iter()
        .filter(|step| step.state == State::Done)
        .count();
    outln!("Cockpit setup: {finished} of {} steps done", steps.len());
    for (number, step) in steps.iter().enumerate() {
        outln!("");
        outln!("[{:<7}] {}. {}", step.state.label(), number + 1, step.title);
        for line in &step.lines {
            outln!("          {line}");
        }
        if let Some(next) = &step.next {
            outln!("          next: {next}");
        }
    }
    outln!("");
    match steps.iter().find(|step| step.state == State::Open) {
        Some(step) => outln!(
            "next: {} (then run `cockpit init` again)",
            step.next.as_deref().unwrap_or(step.title)
        ),
        None => outln!(
            "Cockpit is set up: `cockpit tui` opens the manager, `cockpit --help` lists every verb"
        ),
    }
}
