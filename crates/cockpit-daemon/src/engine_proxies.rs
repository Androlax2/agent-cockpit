//! The daemon runs one egress proxy per recorded engine (#390 part 6c, option A): the owner's
//! `torlek-sandbox engine-proxy`, which takes the listener the engine's relay opens in the
//! engine's network namespace and serves it with the project's decisions, filing what it
//! refuses as the project's containers' requests (`network.rs` moves both files).
//!
//! The proxy's standard input is a pipe only this module holds: dropping it, as the daemon's
//! end does, killed or not, ends the proxy, so the engine's containers have no egress while
//! the daemon is stopped. A proxy that ends is started again after [`RESTART_DELAY`], so an
//! engine that is stopped, or set up before its relay opened a port, is retried without
//! spinning; readiness shows why it is down.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cockpit_readiness::EngineEgress;
use cockpit_sandbox::Sandbox;
use cockpit_turso::Database;
use tokio::sync::Notify;

/// How often the recorded engines are compared with the proxies running.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(2);

/// How long an ended proxy waits before it is started again.
const RESTART_DELAY: Duration = Duration::from_secs(10);

/// The longest line of a proxy's standard error kept as the reason it ended.
const MAX_REASON: usize = 512;

/// One proxy the daemon is to run: a recorded engine whose user has an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wanted {
    pub(crate) project: String,
    pub(crate) relay_socket: PathBuf,
    pub(crate) engine_uid: u32,
}

/// A recorded engine the daemon runs no proxy for, and why: readiness names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unrunnable {
    pub(crate) project: String,
    pub(crate) reason: String,
}

/// Each project's engine proxy state, shared with readiness. Only [`Proxies::reconcile`]
/// writes it.
#[derive(Debug, Clone, Default)]
pub(crate) struct EngineProxies(Arc<Mutex<HashMap<String, EngineEgress>>>);

impl EngineProxies {
    /// `project`'s proxy state; None when the daemon runs none for it: no engine recorded.
    pub(crate) fn state(&self, project: &str) -> Option<EngineEgress> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(project)
            .cloned()
    }

    fn set(&self, project: &str, state: EngineEgress) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(project.to_owned(), state);
    }

    fn retain(&self, is_recorded: impl Fn(&str) -> bool) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|project, _| is_recorded(project));
    }
}

/// A proxy started and not yet seen ending.
struct Running {
    wanted: Wanted,
    /// Dropping it ends the proxy.
    _pipe: ChildStdin,
    is_serving: Arc<AtomicBool>,
    ended: Receiver<String>,
}

/// The proxies the daemon runs.
pub(crate) struct Proxies {
    program: PathBuf,
    egress_port: u16,
    sandbox: Sandbox,
    states: EngineProxies,
    running: HashMap<String, Running>,
    /// Proxies whose pipe was dropped and that may still hold their engine's listener: a
    /// project's next proxy starts once its last one ended.
    stopping: HashMap<String, Receiver<String>>,
    /// When each project's ended proxy may start again.
    retry_at: HashMap<String, Instant>,
    /// The reason last logged for each project that has no serving proxy, so a proxy that
    /// keeps failing the same way is logged once.
    reported: HashMap<String, String>,
}

impl Proxies {
    pub(crate) fn new(
        program: &Path,
        egress_port: u16,
        sandbox: Sandbox,
        states: EngineProxies,
    ) -> Proxies {
        Proxies {
            program: program.to_path_buf(),
            egress_port,
            sandbox,
            states,
            running: HashMap::new(),
            stopping: HashMap::new(),
            retry_at: HashMap::new(),
            reported: HashMap::new(),
        }
    }

    /// Runs exactly the proxies `wanted` names: ends the others, notes the ones that ended
    /// and starts the missing ones whose restart delay passed, each once its last proxy
    /// ended. `unrunnable` are down with their reason. True when it started one. Blocks on
    /// the file system and on starting processes.
    pub(crate) fn reconcile(
        &mut self,
        wanted: &[Wanted],
        unrunnable: &[Unrunnable],
        now: Instant,
    ) -> bool {
        let is_recorded = |project: &str| {
            wanted.iter().any(|engine| engine.project == project)
                || unrunnable.iter().any(|engine| engine.project == project)
        };
        let unwanted: Vec<String> = self
            .running
            .iter()
            .filter(|(_, running)| !wanted.contains(&running.wanted))
            .map(|(project, _)| project.clone())
            .collect();
        for project in unwanted {
            if let Some(Running { ended, .. }) = self.running.remove(&project) {
                self.stopping.insert(project, ended);
            }
        }
        self.stopping
            .retain(|_, ended| matches!(ended.try_recv(), Err(TryRecvError::Empty)));
        self.retry_at.retain(|project, _| is_recorded(project));
        self.reported.retain(|project, _| is_recorded(project));
        self.states.retain(is_recorded);
        for engine in unrunnable {
            self.down(&engine.project, engine.reason.clone());
        }
        let mut ended = Vec::new();
        for (project, running) in &self.running {
            match running.ended.try_recv() {
                Ok(reason) => ended.push((project.clone(), reason)),
                Err(TryRecvError::Disconnected) => ended.push((
                    project.clone(),
                    "the proxy's watcher ended without a reason".to_owned(),
                )),
                Err(TryRecvError::Empty) if running.is_serving.load(Ordering::Acquire) => {
                    self.states.set(project, EngineEgress::Serving);
                    self.reported.remove(project);
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        for (project, reason) in ended {
            self.running.remove(&project);
            self.down(&project, reason);
            self.retry_at.insert(project, now + RESTART_DELAY);
        }
        let mut is_started = false;
        for engine in wanted {
            let is_waiting = self
                .retry_at
                .get(&engine.project)
                .is_some_and(|retry_at| now < *retry_at);
            if self.running.contains_key(&engine.project)
                || self.stopping.contains_key(&engine.project)
                || is_waiting
            {
                continue;
            }
            match self.start(engine) {
                Ok(running) => {
                    self.retry_at.remove(&engine.project);
                    self.states.set(&engine.project, EngineEgress::Starting);
                    self.running.insert(engine.project.clone(), running);
                    is_started = true;
                }
                Err(reason) => {
                    self.down(&engine.project, reason);
                    self.retry_at
                        .insert(engine.project.clone(), now + RESTART_DELAY);
                }
            }
        }
        is_started
    }

    /// `project` has no serving proxy, for `reason`; logged when the reason is new.
    fn down(&mut self, project: &str, reason: String) {
        if self.reported.get(project) != Some(&reason) {
            tracing::warn!(project, reason, "an engine's proxy does not serve");
            self.reported.insert(project.to_owned(), reason.clone());
        }
        self.states.set(project, EngineEgress::Down { reason });
    }

    fn start(&self, engine: &Wanted) -> Result<Running, String> {
        let folder = self
            .sandbox
            .prepare_engine_proxy_folder(&engine.project)?
            .ok_or("the daemon read no sandbox folders, so no engine proxy can share one")?;
        let mut child = Command::new(&self.program)
            .arg("engine-proxy")
            .arg(&engine.relay_socket)
            .arg(engine.engine_uid.to_string())
            .arg(&engine.project)
            .arg(&folder)
            .arg(self.egress_port.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "cannot start {} engine-proxy: {error}",
                    self.program.display()
                )
            })?;
        let pipe = child.stdin.take().ok_or("the proxy was given no pipe")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("the proxy has no standard output")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("the proxy has no standard error")?;
        let is_serving = Arc::new(AtomicBool::new(false));
        let said_serving = Arc::clone(&is_serving);
        let project = engine.project.clone();
        std::thread::Builder::new()
            .name("engine-proxy-out".to_owned())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    match line {
                        Ok(line) if line == "serving" => {
                            said_serving.store(true, Ordering::Release)
                        }
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(project, %error, "an engine proxy's output was not read");
                            return;
                        }
                    }
                }
            })
            .map_err(|error| format!("cannot watch the proxy's output: {error}"))?;
        let (ending, ended) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("engine-proxy".to_owned())
            .spawn(move || {
                let reason = last_line(stderr);
                let status = child.wait();
                let _ = ending.send(match (reason, status) {
                    (Some(reason), _) => reason,
                    (None, Ok(status)) => format!("the proxy ended: {status}"),
                    (None, Err(error)) => format!("the proxy could not be waited for: {error}"),
                });
            })
            .map_err(|error| format!("cannot watch the proxy: {error}"))?;
        Ok(Running {
            wanted: engine.clone(),
            _pipe: pipe,
            is_serving,
            ended,
        })
    }
}

/// The last line `stderr` printed, without the program's prefix, cut to [`MAX_REASON`].
fn last_line(stderr: std::process::ChildStderr) -> Option<String> {
    let mut last = None;
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if !line.trim().is_empty() {
            last = Some(line);
        }
    }
    last.map(|line| {
        let line = line.strip_prefix("torlek-sandbox: ").unwrap_or(&line);
        let mut end = line.len().min(MAX_REASON);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line[..end].to_owned()
    })
}

/// Runs the proxies of the recorded engines until the daemon stops; `decided` is woken after
/// a proxy starts so its decisions file is written before it serves for long.
pub(crate) async fn keep_forever(
    database: Arc<Database>,
    host: crate::containers::Host,
    sandbox: Sandbox,
    decided: Arc<Notify>,
) {
    let proxies = Arc::new(Mutex::new(Proxies::new(
        &host.relay_program,
        cockpit_containers::ENGINE_EGRESS_PORT,
        sandbox,
        host.proxies.clone(),
    )));
    loop {
        match wanted(&database, &host).await {
            Ok((wanted, unrunnable)) => {
                let reconciling = Arc::clone(&proxies);
                let started = tokio::task::spawn_blocking(move || {
                    reconciling
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .reconcile(&wanted, &unrunnable, Instant::now())
                })
                .await;
                match started {
                    Ok(true) => decided.notify_one(),
                    Ok(false) => {}
                    Err(error) => tracing::warn!(%error, "the engine proxies were not reconciled"),
                }
            }
            // The proxies running keep running: a failed read decides nothing.
            Err(error) => tracing::warn!(%error, "the recorded engines were not read"),
        }
        tokio::time::sleep(RECONCILE_INTERVAL).await;
    }
}

/// A proxy for each recorded engine whose user has an account, as the launchers' decisions
/// name its relay; the other recorded engines, with why none runs.
async fn wanted(
    database: &Database,
    host: &crate::containers::Host,
) -> Result<(Vec<Wanted>, Vec<Unrunnable>), String> {
    let read = database.read().await.map_err(|error| error.to_string())?;
    let engines = cockpit_containers::recorded_engines(&read)
        .await
        .map_err(|error| format!("the recorded engines could not be read: {error}"))?;
    if engines.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let passwd = crate::containers::read_passwd().await?;
    let mut wanted = Vec::new();
    let mut unrunnable = Vec::new();
    for engine in engines {
        match crate::containers::engine_relay(&read, host, &passwd, &engine.project).await {
            Ok(Some(relay)) => wanted.push(Wanted {
                project: engine.project,
                relay_socket: relay.socket,
                engine_uid: relay.uid,
            }),
            Ok(None) => unrunnable.push(Unrunnable {
                project: engine.project,
                reason: "the engine recorded is not the project's engine user's, or that user \
                         has no account"
                    .to_owned(),
            }),
            Err(error) => unrunnable.push(Unrunnable {
                project: engine.project,
                reason: format!("the engine's relay was not found: {error}"),
            }),
        }
    }
    Ok((wanted, unrunnable))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    struct Fixture {
        dir: tempfile::TempDir,
        proxies: Proxies,
        states: EngineProxies,
    }

    impl Fixture {
        fn log(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.path().join("log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// Reconciles until `is_done` holds, at most ten seconds.
        fn until(&mut self, wanted: &[Wanted], is_done: impl Fn(&Fixture) -> bool) {
            let started = Instant::now();
            loop {
                self.proxies.reconcile(wanted, &[], Instant::now());
                if is_done(self) {
                    return;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "{:?}",
                    self.log()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// A stand-in for `torlek-sandbox` running `body` after logging its arguments.
    fn fixture(body: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let program = dir.path().join("torlek-sandbox");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n{body}\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mounts = dir.path().join("mounts");
        let sandbox = Sandbox::new(
            cockpit_sandbox::Platform::Linux,
            cockpit_sandbox::HostPaths {
                read_only: Vec::new(),
                sockets: Vec::new(),
                denied: Vec::new(),
                denied_write: Vec::new(),
                owner_uid: 1000,
                mount_sources: mounts,
                homes: dir.path().join("homes"),
                logins: dir.path().join("logins"),
            },
            cockpit_sandbox::Backend::Missing {
                reason: "not needed".to_owned(),
            },
        );
        let states = EngineProxies::default();
        let proxies = Proxies::new(&program, 3128, sandbox, states.clone());
        Fixture {
            dir,
            proxies,
            states,
        }
    }

    /// Says it serves, then holds until its pipe closes, as `engine-proxy` does.
    const SERVES: &str =
        "echo serving\ncat >/dev/null\necho ended >> \"$(dirname \"$0\")/log\"\nexit 125";

    /// As [`SERVES`], and takes a while to let go of the listener once its pipe closes.
    const SLOW_TO_END: &str = "echo serving\ncat >/dev/null\nsleep 0.3\necho ended >> \"$(dirname \"$0\")/log\"\nexit 125";

    const REFUSED: &str = "echo 'torlek-sandbox: the engine'\\''s relay answered: refused: this relay serves no egress' >&2\nexit 125";

    fn app() -> Wanted {
        Wanted {
            project: "app".to_owned(),
            relay_socket: PathBuf::from("/run/torlek-e3/relay.sock"),
            engine_uid: 991,
        }
    }

    #[test]
    fn a_recorded_engine_gets_a_proxy_on_its_relay_and_a_folder_of_its_own() {
        let mut fixture = fixture(SERVES);

        fixture.until(&[app()], |fixture| {
            fixture.states.state("app") == Some(EngineEgress::Serving)
        });

        let folder = fixture.dir.path().join("mounts/engine-app");
        assert!(folder.is_dir());
        assert_eq!(
            fixture.log(),
            vec![format!(
                "engine-proxy /run/torlek-e3/relay.sock 991 app {} 3128",
                folder.display()
            )]
        );
    }

    #[test]
    fn an_engine_no_longer_recorded_has_its_proxy_ended() {
        let mut fixture = fixture(SERVES);
        fixture.until(&[app()], |fixture| {
            fixture.states.state("app") == Some(EngineEgress::Serving)
        });

        fixture.until(&[], |fixture| fixture.log().contains(&"ended".to_owned()));

        assert_eq!(fixture.states.state("app"), None);
    }

    #[test]
    fn a_proxy_that_ends_is_down_with_its_reason() {
        let mut fixture = fixture(REFUSED);

        fixture.until(&[app()], |fixture| {
            matches!(fixture.states.state("app"), Some(EngineEgress::Down { .. }))
        });

        assert_eq!(
            fixture.states.state("app"),
            Some(EngineEgress::Down {
                reason: "the engine's relay answered: refused: this relay serves no egress"
                    .to_owned()
            })
        );
    }

    #[test]
    fn a_recorded_engine_no_proxy_can_run_for_is_down_with_why() {
        let mut fixture = fixture(SERVES);
        let unrunnable = Unrunnable {
            project: "app".to_owned(),
            reason: "no account is named torlek-e3".to_owned(),
        };

        let is_started = fixture
            .proxies
            .reconcile(&[], &[unrunnable], Instant::now());

        assert!(!is_started);
        assert_eq!(
            fixture.states.state("app"),
            Some(EngineEgress::Down {
                reason: "no account is named torlek-e3".to_owned()
            })
        );
        assert!(fixture.log().is_empty(), "{:?}", fixture.log());
    }

    /// The old proxy holds the engine's listener until it exits: the new one would be refused.
    #[test]
    fn a_changed_engines_new_proxy_starts_only_once_the_old_one_ended() {
        let mut fixture = fixture(SLOW_TO_END);
        fixture.until(&[app()], |fixture| {
            fixture.states.state("app") == Some(EngineEgress::Serving)
        });
        let moved = Wanted {
            engine_uid: 992,
            ..app()
        };

        fixture.until(std::slice::from_ref(&moved), |fixture| {
            fixture.log().len() == 3
        });

        let log = fixture.log();
        assert_eq!(log[1], "ended", "{log:?}");
        assert!(log[2].contains(" 992 app "), "{log:?}");
    }

    #[test]
    fn an_ended_proxy_starts_again_only_after_the_restart_delay() {
        let mut fixture = fixture(REFUSED);
        fixture.until(&[app()], |fixture| {
            matches!(fixture.states.state("app"), Some(EngineEgress::Down { .. }))
        });
        let ended_at = Instant::now();

        let is_started_early = fixture.proxies.reconcile(&[app()], &[], ended_at);
        let is_started_late = fixture.proxies.reconcile(
            &[app()],
            &[],
            ended_at + RESTART_DELAY + Duration::from_secs(1),
        );

        assert!(!is_started_early);
        assert!(is_started_late);
    }
}
