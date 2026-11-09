//! A provider suspended with Ctrl+Z (#1271). Like Claude Code, the fake agent prints a notice,
//! releases its terminal and sends itself `SIGTSTP`, which the kernel discards (its process
//! group is orphaned under tmux), then waits for a `SIGCONT` that nothing else sends. The
//! daemon types nothing into it, tells the sender, and resumes it.
#![cfg(target_os = "linux")]
mod candidate;
mod common;
mod messaging_fixture;
use cockpit_protocol::{Command, ErrorCode};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{
    Action, Activity, Capabilities, Capability, LaunchCommand, LaunchRequest, Provider,
    ProviderError, ProviderName, TerminalInput,
};
use common::{GENEROUS, refused};
use messaging_fixture::Home;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SUSPENDED_NOTICE: &str = "Fake agent has been suspended. Run fg to bring it back.";

struct Suspending {
    fake: FakeProvider,
}
impl Suspending {
    fn new(home: &Home) -> Self {
        let program = home.path().join("suspend");
        std::fs::write(&program, suspend_script(&home.path().join("resumed"))).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            fake: FakeProvider::new(
                ProviderName::new("fake").unwrap(),
                home.path().join("launches"),
            )
            .running(program)
            .reading_messages(home.path().join("messages"))
            .reporting(Capability::ScreenReading),
        }
    }
}
impl Provider for Suspending {
    fn name(&self) -> &ProviderName {
        self.fake.name()
    }
    fn capabilities(&self) -> Capabilities {
        self.fake.capabilities()
    }
    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        self.fake.launch(request)
    }
    fn input(
        &self,
        action: &Action<'_>,
        activity: Option<Activity>,
    ) -> Result<Vec<TerminalInput>, ProviderError> {
        self.fake.input(action, activity)
    }
    fn is_suspended(&self, screen: &str) -> bool {
        screen.lines().any(|line| line == SUSPENDED_NOTICE)
    }
}

/// Run by the fake agent before it reads its messages: suspends as Claude Code does, and
/// once resumed appends `resumed` to `log`, then clears its screen as Claude Code redraws.
fn suspend_script(log: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         trap 'resumed=yes' CONT\n\
         printf '%s\\n' '{SUSPENDED_NOTICE}'\n\
         kill -TSTP 0\n\
         until [ -n \"${{resumed:-}}\" ]; do sleep 0.05; done\n\
         printf 'resumed\\n' >> '{log}'\n\
         printf '\\033[H\\033[2J'\n",
        log = log.display()
    )
}

fn resumes(home: &Home) -> usize {
    std::fs::read_to_string(home.path().join("resumed"))
        .map(|log| log.lines().count())
        .unwrap_or(0)
}

async fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + GENEROUS;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The agent's visible screen, read from the test's own tmux server.
fn screen(home: &Home, session: &str) -> String {
    let output = std::process::Command::new("tmux")
        .arg("-S")
        .arg(home.path().join("tmux.sock"))
        .args(["capture-pane", "-p", "-t", &format!("={session}:")])
        .env_remove("TMUX")
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn send(session: &str, message: &str) -> Command {
    Command::SessionSend {
        session: session.into(),
        message: message.into(),
    }
}

#[tokio::test]
async fn a_message_to_a_suspended_agent_is_refused_untyped_and_the_agent_resumed() {
    let home = Home::new();
    let _daemon = home.start(vec![Arc::new(Suspending::new(&home))]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    home.waiting(&session.id).await;
    until("the agent shows itself suspended", || {
        screen(&home, &session.id).contains(SUSPENDED_NOTICE)
    })
    .await;

    let refusal = refused(client.request(send(&session.id, "lost"), GENEROUS).await);

    assert_eq!(refusal.code, ErrorCode::Conflict);
    assert!(
        refusal.message.contains("suspended") && refusal.message.contains("send again"),
        "{}",
        refusal.message
    );
    until("the agent was resumed", || resumes(&home) == 1).await;
    client
        .request(send(&session.id, "after resume"), GENEROUS)
        .await
        .unwrap();
    assert_eq!(
        home.messages(1).await,
        vec![(session.id, "after resume".into())]
    );
}

#[tokio::test]
async fn the_periodic_pass_resumes_a_suspended_agent() {
    let home = Home::new();
    let provider: Arc<dyn Provider> = Arc::new(Suspending::new(&home));
    let mut wiring = common::wiring_in(home.path(), vec![provider]);
    wiring.session_tick = Duration::from_millis(200);
    let _daemon = home.start_wired(wiring).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    home.waiting(&session.id).await;

    until("the agent was resumed", || resumes(&home) == 1).await;
}
