//! `cockpit init`, and `cockpit` alone until the setup is done (#100): the first-run steps,
//! each with its state and the next action, computed again at every run from the readiness
//! report, the owner's configuration, the registered projects and the sessions.
//!
//! It writes nothing: each step's action is a verb the owner runs or a line the owner writes,
//! so the checklist is the same whether it is followed once or interrupted and resumed.

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

use cockpit_client::ClientError;
use cockpit_config::{AgentSandbox, EffectiveConfig, Source};
use cockpit_protocol::{
    Command, ErrorCode, ProjectView, ReadinessStatusView, Reply, RequirementView, SessionView,
};

use crate::daemon;

/// For the three reads together; the daemon probes Docker only for a project, which `init`
/// does not name.
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
    daemon: Result<DaemonFacts, DaemonProblem>,
}

struct DaemonFacts {
    requirements: Vec<RequirementView>,
    projects: Vec<ProjectView>,
    /// Running and ended ones: one launched session is enough to have started.
    sessions: Vec<SessionView>,
}

/// Why the daemon's facts are missing, and what fixes that.
struct DaemonProblem {
    detail: String,
    next: &'static str,
}

/// Exit 1 only when the client cannot run at all; every other failure is a step to show.
fn gather() -> Result<Facts, ExitCode> {
    let config = cockpit_config::load(&crate::profile::environment(), None, &[])
        .map_err(|error| error.to_string());
    let daemon = match crate::agent::locate_socket_dir() {
        Ok(_) => ask_daemon(&crate::agent::caller_socket()?)?,
        Err(error) => Err(DaemonProblem {
            detail: format!("cannot locate its socket: {error}"),
            next: "fix what is named here so `torlek` finds the daemon's socket",
        }),
    };
    Ok(Facts { config, daemon })
}

fn ask_daemon(socket: &Path) -> Result<Result<DaemonFacts, DaemonProblem>, ExitCode> {
    let commands = vec![
        Command::Readiness { project: None },
        Command::ProjectList,
        Command::SessionList { ended: true },
    ];
    Ok(
        match daemon::exchange_all(socket, commands, INIT_DEADLINE)? {
            Ok(replies) => daemon_facts(replies),
            Err(error @ ClientError::Connect { .. }) => Err(DaemonProblem {
                detail: format!("not running ({error})"),
                next: "`torlek daemon install` runs it as a user service (or run `torlek-daemon` \
                   in another terminal)",
            }),
            Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized => {
                Err(DaemonProblem {
                    detail: format!("refused: {}", error.message),
                    next: "run `torlek init` from your own terminal, outside any Torlek session",
                })
            }
            Err(error) => Err(failing_daemon(&error.to_string())),
        },
    )
}

fn failing_daemon(detail: &str) -> DaemonProblem {
    DaemonProblem {
        detail: format!("running, but it failed to answer: {detail}"),
        next: "read the daemon's log, then restart `torlek-daemon`; a `torlek` and a daemon \
               of different releases refuse each other",
    }
}

fn daemon_facts(replies: Vec<Reply>) -> Result<DaemonFacts, DaemonProblem> {
    let mut replies = replies.into_iter();
    let unexpected = |reply: Option<Reply>| failing_daemon(&format!("unexpected reply {reply:?}"));
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
            "fix what is named here; `torlek config show` reads the configuration again".to_owned(),
        );
    }
    match &facts.daemon {
        Err(problem) => {
            blocks = true;
            lines.push(format!("daemon: {}", problem.detail));
            next.get_or_insert_with(|| problem.next.to_owned());
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
        lines.push("tmux, the sandbox engine, proxy and resource controls are ready".to_owned());
    }
    lines.push("`torlek doctor` shows every requirement".to_owned());
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
         own checkout, cannot read Torlek's own files, and reaches the network through \
         greyproxy"
            .to_owned(),
        "needs you: a command outside the sandbox (`torlek ask run`) or a step only a \
         person can do (`torlek ask guide`); an agent cannot approve its own request"
            .to_owned(),
        "approvals: `torlek approval list` (or the approval pane of `torlek tui`) shows the \
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
                "you chose to run agents UNSANDBOXED, with your rights ({}:{line}): {}",
                path.display(),
                cockpit_config::UNSANDBOXED_RISK
            ),
        });
        return Step {
            title: SANDBOX,
            state: State::Done,
            lines,
            next: None,
        };
    }
    let file = &config.user_file.path;
    // Init writes nothing, so the owner creates the folder a fresh profile lacks (#625).
    // Anything else at that path (a file, a dangling link) is left for the owner to see.
    let create_folder = match file.parent() {
        Some(folder)
            if std::fs::symlink_metadata(folder)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            format!(
                "create the config folder with `mkdir -p -m 700 {}`, then ",
                shell_word(&folder.to_string_lossy())
            )
        }
        _ => String::new(),
    };
    Step {
        title: SANDBOX,
        state: State::Open,
        lines,
        next: Some(format!(
            "to accept, {create_folder}write `[agents]` and `sandbox = \"required\"` in {} \
             (`sandbox = \"off\"` runs agents unsandboxed, with your rights: {})",
            shell_word(&file.to_string_lossy()),
            cockpit_config::UNSANDBOXED_RISK
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

/// The providers sessions can start with. Readiness lists one the daemon could not wire as
/// `available`, so that it does not block (#411), and adds a `provider NAME sign-in` row
/// only for a usable one; `torlek doctor` prints the same report (#624).
fn available_providers(requirements: &[RequirementView]) -> Vec<&str> {
    requirements
        .iter()
        .filter(|requirement| requirement.status == ReadinessStatusView::Available)
        .filter_map(provider_name)
        .filter(|name| {
            let sign_in = format!("provider {name} sign-in");
            requirements
                .iter()
                .any(|requirement| requirement.name == sign_in)
        })
        .collect()
}

fn provider_step(requirements: &[RequirementView], available: &[&str]) -> Step {
    let missing: Vec<&RequirementView> = requirements
        .iter()
        .filter(|requirement| {
            requirement.name == "providers"
                || provider_name(requirement).is_some_and(|name| !available.contains(&name))
        })
        .collect();
    let mut lines: Vec<String> = missing
        .iter()
        .map(
            |requirement| match (requirement.status, &requirement.next) {
                (ReadinessStatusView::Available, Some(next)) => {
                    format!("{}: {}; {next}", requirement.name, requirement.detail)
                }
                (ReadinessStatusView::Available, None) => {
                    format!("{}: {}", requirement.name, requirement.detail)
                }
                _ => requirement_line(requirement),
            },
        )
        .collect();
    match available {
        [] => Step {
            title: PROVIDER,
            state: State::Open,
            // The blocking `providers` row names every supported agent, not one of them.
            next: missing
                .iter()
                .find(|requirement| requirement.name == "providers")
                .and_then(|requirement| requirement.next.clone())
                .or_else(|| {
                    missing
                        .iter()
                        .find_map(|requirement| requirement.next.clone())
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

/// Done once one available provider is signed in, since a session needs only one; else open
/// while one needs a sign-in, and `unknown` names how to
/// check by hand without holding the setup.
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
    let has = |status: ReadinessStatusView| {
        sign_ins
            .iter()
            .find(|requirement| requirement.status == status)
    };
    if has(ReadinessStatusView::Available).is_some() {
        return done(SIGN_IN, lines);
    }
    let needed = sign_ins.iter().find(|requirement| {
        !matches!(
            requirement.status,
            ReadinessStatusView::Available | ReadinessStatusView::Unknown
        )
    });
    let (state, requirement) = match (needed, has(ReadinessStatusView::Unknown)) {
        (Some(needed), _) => (State::Open, *needed),
        (None, Some(unknown)) => (State::Unknown, *unknown),
        (None, None) => return done(SIGN_IN, lines),
    };
    Step {
        title: SIGN_IN,
        state,
        lines,
        next: requirement.next.clone(),
    }
}

fn project_step(projects: &[ProjectView]) -> Step {
    if projects.is_empty() {
        return Step {
            title: PROJECT,
            state: State::Open,
            lines: vec!["no project is registered".to_owned()],
            next: Some(
                "`torlek project add DIR` registers the Git repository agents work in".to_owned(),
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
                "{} session(s) started; `torlek tui` shows them",
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
            "`torlek session new {}{agent}` starts an agent there; \
             `torlek tui` shows it",
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
    outln!("Torlek setup: {finished} of {} steps done", steps.len());
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
            "next: {} (then run `torlek init` again)",
            step.next.as_deref().unwrap_or(step.title)
        ),
        None => outln!(
            "Torlek is set up: `torlek tui` opens the manager, `torlek --help` lists every verb"
        ),
    }
}
