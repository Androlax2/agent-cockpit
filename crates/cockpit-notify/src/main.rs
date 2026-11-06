//! `torlek-notify --socket PATH --state-dir DIR [--desktop EVENTS] [--sound EVENTS]`: tells
//! the owner, with a desktop notification and a sound, when a session of the daemon at `PATH`
//! needs them. `EVENTS` is `none` or a comma-separated list of `failure`, `hold`, `question`,
//! `quota` and `finished`; left out, every event is on. It logs under `DIR`.
//!
//! A host without a session bus, `notify-send` or a sound player runs it all the same: that
//! outlet is off and the log says why.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use tokio::time::{Instant, MissedTickBehavior};

use cockpit_client::{ClientError, Connection};
use cockpit_notify::{
    Desktop, Event, FeedEvent, Host, Notifier, Outlets, Sound, follow_feed, names,
};
use cockpit_protocol::{Command, Reply};
use tokio::sync::mpsc;

/// How often the notifier asks again for the agents' reports, the approval queue and the
/// network requests, whatever the feed said. The feed's view stamps (#706) bring a change at
/// once; this catches what no commit records, as the TUI's refresh does.
const REFRESH: Duration = Duration::from_secs(5);
const CONNECT_DEADLINE: Duration = Duration::from_secs(2);
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
/// How long the notifier stops asking after it could not reach the daemon, doubling while it
/// stays out of reach, as the feed's own retry does.
const LONGEST_PAUSE: Duration = Duration::from_secs(30);

const USAGE: &str = "usage: torlek-notify --socket PATH --state-dir DIR [--desktop EVENTS] [--sound EVENTS] | --version
EVENTS: none, or a comma-separated list of failure, hold, question, quota, finished (default: all)";

struct Options {
    socket: PathBuf,
    state_dir: PathBuf,
    desktop_events: BTreeSet<Event>,
    sound_events: BTreeSet<Event>,
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments == ["--version"] {
        println!("torlek-notify {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let options = match parse(&arguments) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("torlek-notify: {problem}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match notify(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("torlek-notify: {problem}");
            ExitCode::FAILURE
        }
    }
}

fn parse(arguments: &[String]) -> Result<Options, String> {
    let mut socket = None;
    let mut state_dir = None;
    let mut desktop_events = BTreeSet::from(Event::ALL);
    let mut sound_events = BTreeSet::from(Event::ALL);
    let mut words = arguments.iter();
    while let Some(flag) = words.next() {
        let value = words
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--socket" => socket = Some(PathBuf::from(value)),
            "--state-dir" => state_dir = Some(PathBuf::from(value)),
            "--desktop" => desktop_events = Event::parse_list(value)?,
            "--sound" => sound_events = Event::parse_list(value)?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Options {
        socket: socket.ok_or("--socket is required")?,
        state_dir: state_dir.ok_or("--state-dir is required")?,
        desktop_events,
        sound_events,
    })
}

fn notify(options: Options) -> Result<(), String> {
    let log_dir = cockpit_diagnostics::log_dir_in(&options.state_dir, "notify")
        .map_err(|error| error.to_string())?;
    let directives =
        cockpit_diagnostics::Directives::from_env().map_err(|error| error.to_string())?;
    let diagnostics =
        cockpit_diagnostics::start(cockpit_diagnostics::Options::new(log_dir, directives))
            .map_err(|error| error.to_string())?;
    tracing::dispatcher::set_global_default(diagnostics.dispatch().clone())
        .map_err(|error| error.to_string())?;
    let outlets = find_outlets(&Host::from_env(), &options);
    tracing::info!(
        socket = %options.socket.display(),
        desktop = outlets.desktop.is_some(),
        desktop_events = %names(&outlets.desktop_events),
        sound = outlets.sound.is_some(),
        sound_events = %names(&outlets.sound_events),
        "the notifier started"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the runtime: {error}"))?;
    let outcome = runtime.block_on(run(options.socket, outlets));
    if let Err(problem) = &outcome {
        tracing::error!(problem, "the notifier stopped");
    }
    outcome
}

/// The outlets this host has; one it lacks is off, with the reason logged once.
fn find_outlets(host: &Host, options: &Options) -> Outlets {
    let desktop = Desktop::find(host)
        .inspect_err(|reason| tracing::warn!(reason, "desktop notifications are off"))
        .ok();
    let sound = Sound::find(host)
        .inspect_err(|reason| tracing::warn!(reason, "sounds are off"))
        .ok();
    Outlets {
        desktop,
        desktop_events: options.desktop_events.clone(),
        sound,
        sound_events: options.sound_events.clone(),
    }
}

async fn run(socket: PathBuf, outlets: Outlets) -> Result<(), String> {
    let (feed_events, mut feed) = mpsc::channel::<FeedEvent>(1);
    tokio::spawn(follow_feed(socket.clone(), feed_events));
    let mut daemon = Daemon {
        socket,
        connection: None,
        pause: None,
    };
    let mut notifier = Notifier::default();
    let mut refresh = tokio::time::interval(REFRESH);
    // A slow daemon delays the next refresh rather than earning a burst of them.
    refresh.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let commands = tokio::select! {
            _ = refresh.tick() => Notifier::refresh_commands(),
            event = feed.recv() => match event {
                None => return Err("the change feed task stopped".to_owned()),
                Some(FeedEvent::Batch { batch, applied }) => {
                    let outcome = notifier.apply_changes(&batch);
                    let notices = outcome.as_ref().map(Vec::as_slice).unwrap_or_default();
                    outlets.tell(notices);
                    // The feed task waits on this answer; it is gone only when we stop.
                    let _ = applied.send(outcome.map(drop));
                    notifier.changed_view_commands()
                }
                Some(FeedEvent::Lost(problem)) => {
                    tracing::warn!(problem, "the change feed was lost");
                    Vec::new()
                }
            },
        };
        for command in commands {
            match daemon.request(command.clone()).await {
                Ok(reply) => outlets.tell(&notifier.answer(&command, Ok(reply))),
                Err(Failure::Refused(problem)) => {
                    outlets.tell(&notifier.answer(&command, Err(problem)));
                }
                // Not an answer: what the notifier starts on must come from the daemon, or
                // everything already waiting would be news once it answers.
                Err(Failure::Unreachable) => break,
            }
        }
    }
}

/// Why a request has no reply.
enum Failure {
    /// The daemon answered with an error.
    Refused(String),
    /// The daemon could not be reached, or did not answer in time; logged when it starts.
    Unreachable,
}

/// One connection to the daemon, opened again after it was lost. Requests are sent one at a
/// time: the notifier has nothing else to do while it waits.
struct Daemon {
    socket: PathBuf,
    connection: Option<Connection>,
    /// Out of reach since the last request: no request before `until`.
    pause: Option<Pause>,
}

struct Pause {
    until: Instant,
    length: Duration,
}

impl Daemon {
    async fn request(&mut self, command: Command) -> Result<Reply, Failure> {
        if self
            .pause
            .as_ref()
            .is_some_and(|pause| Instant::now() < pause.until)
        {
            return Err(Failure::Unreachable);
        }
        let connection = match &mut self.connection {
            Some(connection) => connection,
            None => match Connection::open(&self.socket, CONNECT_DEADLINE).await {
                Ok(opened) => self.connection.insert(opened),
                Err(error) => return Err(self.out_of_reach(&error)),
            },
        };
        match connection.request(command, REQUEST_DEADLINE).await {
            Ok(reply) => {
                if self.pause.take().is_some() {
                    tracing::info!("the daemon answers again");
                }
                Ok(reply)
            }
            Err(ClientError::Daemon(error)) => Err(Failure::Refused(format!(
                "{}: {}",
                error.code.as_str(),
                error.message
            ))),
            // A connection that timed out may never answer again: the next request opens
            // a new one.
            Err(error) => {
                self.connection = None;
                Err(self.out_of_reach(&error))
            }
        }
    }

    fn out_of_reach(&mut self, error: &ClientError) -> Failure {
        let length = match &self.pause {
            None => {
                tracing::warn!(%error, "cannot reach the daemon; asking again later");
                REFRESH
            }
            Some(pause) => (pause.length * 2).min(LONGEST_PAUSE),
        };
        self.pause = Some(Pause {
            until: Instant::now() + length,
            length,
        });
        Failure::Unreachable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn every_event_is_on_when_none_is_named() {
        let options = parse(&words("--socket /s --state-dir /d")).unwrap();
        assert_eq!(options.desktop_events, BTreeSet::from(Event::ALL));
        assert_eq!(options.sound_events, BTreeSet::from(Event::ALL));
    }

    #[test]
    fn named_events_are_the_only_ones_on() {
        let options = parse(&words(
            "--socket /s --state-dir /d --desktop failure,hold --sound none",
        ))
        .unwrap();
        assert_eq!(
            options.desktop_events,
            BTreeSet::from([Event::Failure, Event::Hold])
        );
        assert!(options.sound_events.is_empty());
    }

    #[test]
    fn an_unknown_event_is_refused() {
        let refused = parse(&words("--socket /s --state-dir /d --sound loud")).err();
        assert!(refused.is_some_and(|problem| problem.contains("loud")));
    }

    #[test]
    fn a_missing_socket_is_refused() {
        assert!(parse(&words("--state-dir /d")).is_err());
    }
}
