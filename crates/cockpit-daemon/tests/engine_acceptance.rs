//! #390's acceptance: a project's own rootless Docker engine, set up as the owner sets it up
//! and reached from a sandboxed session through the real daemon. The owner installs the masks
//! helper, runs the setup script `torlek doctor` writes, records the engine and grants Docker
//! Hub; the session's agent then pulls a pinned image and its containers:
//!
//! - see neither the owner's home nor Torlek's state through a bind of the host's `/`;
//! - cannot write the checkout's Git hooks or read its `.env`;
//! - leave files the session can change and delete;
//! - reach no host directly, by address or by name;
//! - reach a granted host through the engine's proxy, which refuses a provider's host and
//!   an unknown one at once and files the unknown one as the project's containers' request;
//! - hold the proxy in their environment and none of the session's credentials.
//!
//! A second project's session reaches no engine.
//!
//! Ignored by default: it needs root through `sudo -n`, systemd, rootless Docker's programs
//! and the internet, and it leaves the engine's user and units on the host. CI's Linux sandbox
//! job runs it on its disposable runner with `--run-ignored only`. It is one test because the
//! setup takes minutes and engine users are named by project id, so two homes would collide.

mod candidate;
mod real_daemon;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use cockpit_protocol::{Command, NetworkSubjectView, Reply};
use real_daemon::{COMMANDED_PROJECT, GENEROUS, Home, new_session};

const REQUIRED: &str = "[agents]\nsandbox = \"required\"\n";
/// curl's image, by digest: its tag could move.
const IMAGE: &str = "curlimages/curl:8.22.0@sha256:\
                     58adaa4e8dca9c988bae2aba4ab3434a0bb2da16bbe3f92dec39ec7785166777";
const OWNER_KEY: &str = "owner's key";
const CHECKOUT_SECRET: &str = "checkout-env-secret";
const PROVIDER_TOKEN: &str = "sk-ant-oat01-engine_fixture_value";
/// Docker Hub's registry, which `network grant PROJECT docker-hub` grants.
const GRANTED_HOST: &str = "registry-1.docker.io";
/// The claude adapter's API host: a session reaches it, a container does not.
const PROVIDER_HOST: &str = "api.anthropic.com";
const UNKNOWN_HOST: &str = "example.com";
/// A public resolver's address, reached without a name.
const DIRECT_ADDRESS: &str = "1.1.1.1";
/// A pull and a container's start, which a session's agent line waits for.
const CONTAINER_WITHIN: Duration = Duration::from_secs(180);
/// A pull's own limit, inside [`CONTAINER_WITHIN`].
const PULL_SECONDS: u64 = 150;
/// The engine's start after its setup restarted it.
const ENGINE_WITHIN: Duration = Duration::from_secs(90);

/// The host's `PATH`: `torlek setup engine-masks` runs `sudo`.
fn host_path() -> String {
    std::env::var("PATH").expect("the test runs with a PATH")
}

/// What the owner's `torlek` printed for `arguments`, with the host's `PATH`.
async fn owner(home: &Home, arguments: &[&str]) -> Output {
    home.cockpit(arguments, &[("PATH", &host_path())]).await
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// [`owner`], asserting success; its standard output.
async fn owner_succeeds(home: &Home, arguments: &[&str]) -> String {
    let output = owner(home, arguments).await;
    assert!(
        output.status.success(),
        "{arguments:?}: {}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    text(&output.stdout)
}

/// What `line` printed in the agent working in `root`, ending `exit=<code>`, within `within`.
async fn run_in_agent(home: &Home, root: &Path, line: &str, within: Duration) -> String {
    let number = home.command_agent(root, line);
    home.agent_output_within(root, number, within).await
}

fn exit_code(printed: &str) -> &str {
    printed
        .lines()
        .last()
        .and_then(|line| line.strip_prefix("exit="))
        .unwrap_or_else(|| panic!("no exit code in {printed}"))
}

fn printed_value<'a>(printed: &'a str, key: &str) -> &'a str {
    printed
        .split_whitespace()
        .find_map(|word| word.strip_prefix(key))
        .unwrap_or_else(|| panic!("no {key} in {printed}"))
}

/// `docker run --rm` of [`IMAGE`] with `options`, running `script` in its shell.
fn container(options: &str, script: &str) -> String {
    format!("docker run --rm {options} --entrypoint sh {IMAGE} -c '{script}'")
}

/// curl of `https://<host>/` in a container: the proxy's answer to the tunnel, the server's
/// (`000` when there was none) and the seconds it took.
fn fetch_in_container(host: &str) -> String {
    container(
        "",
        &format!(
            "curl -sS -o /dev/null --max-time 20 \
             -w \"tunnel=%{{http_connect}} status=%{{http_code}} seconds=%{{time_total}}\\n\" \
             https://{host}/"
        ),
    )
}

/// The owner's home with a key no container may read, `required` set, and the Claude
/// credential no container may hold.
fn prepare(home: &Home) {
    let ssh = home.path().join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(ssh.join("id_ed25519"), format!("{OWNER_KEY}\n")).unwrap();
    home.write_owner_config(REQUIRED);
    home.write_provider_credential(
        "claude",
        "CLAUDE_CODE_OAUTH_TOKEN",
        &format!("{PROVIDER_TOKEN}\n"),
        0o600,
    );
}

/// The setup script `torlek doctor --project ROOT` wrote and names, and its engine user.
async fn written_setup(home: &Home, root: &Path) -> (PathBuf, String) {
    let report = owner(home, &["doctor", "--project", &root.display().to_string()]).await;
    let printed = text(&report.stdout);
    // The engine is not set up yet, which blocks the report.
    assert_eq!(report.status.code(), Some(1), "{printed}{report:?}");
    let script = printed
        .split("run `sudo sh ")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("doctor names no setup script: {printed}"));
    assert_eq!(
        script.parent(),
        Some(home.state_dir().join("containers").as_path()),
        "{printed}"
    );
    assert!(script.is_file(), "{printed}");
    let name = script.file_name().unwrap().to_str().unwrap();
    let user = name.strip_suffix("-setup.sh").unwrap().to_owned();
    (script, user)
}

/// Runs `script` as root, as the owner runs it after reviewing it, off the test's runtime
/// threads: it takes minutes.
async fn run_as_root(script: &Path) {
    let mut command = std::process::Command::new("sudo");
    command.args(["-n", "sh"]).arg(script);
    let output = tokio::task::spawn_blocking(move || command.output().expect("sudo runs"))
        .await
        .unwrap();
    println!("{}{}", text(&output.stdout), text(&output.stderr));
    assert!(output.status.success(), "{}: {output:?}", script.display());
}

/// Records the project's engine once it answers after its setup restarted it.
async fn record_engine(home: &Home, user: &str) -> String {
    let deadline = Instant::now() + ENGINE_WITHIN;
    loop {
        let output = owner(home, &["containers", "record", COMMANDED_PROJECT]).await;
        if output.status.success() {
            return text(&output.stdout);
        }
        if Instant::now() >= deadline {
            panic!(
                "the engine was not recordable: {}{}\n{}",
                text(&output.stdout),
                text(&output.stderr),
                engine_state(user).await
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Waits, as the owner does when doctor says the proxy is starting, until doctor shows the
/// daemon's proxy serving the engine's pulls and containers.
async fn egress_serves(home: &Home, root: &Path, user: &str) {
    let deadline = Instant::now() + ENGINE_WITHIN;
    loop {
        let report = owner(home, &["doctor", "--project", &root.display().to_string()]).await;
        let printed = text(&report.stdout);
        let egress = printed
            .lines()
            .find(|line| line.contains("  container egress: "))
            .and_then(|line| line.split_whitespace().next());
        if egress == Some("available") {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "the engine's egress never served: {printed}{}\n{}",
                text(&report.stderr),
                engine_state(user).await
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// What root sees of `user`'s engine: its units, their journal and its socket's folder, for
/// a failure's message.
async fn engine_state(user: &str) -> String {
    let script = format!(
        "systemctl status --no-pager --full torlek-engine@{user}.service \
         torlek-engine-relay@{user}.service; \
         journalctl --no-pager --lines 60 --unit torlek-engine@{user}.service \
         --unit torlek-engine-relay@{user}.service; \
         systemctl show --property Upholds torlek-engine@{user}.service; \
         ls -la /run/{user}; getfacl --absolute-names /run/{user} /run/{user}/docker.sock"
    );
    let mut command = std::process::Command::new("sudo");
    command.args(["-n", "sh", "-c", &script]);
    let output = tokio::task::spawn_blocking(move || command.output().expect("sudo runs"))
        .await
        .unwrap();
    format!("{}{}", text(&output.stdout), text(&output.stderr))
}

/// Pulls [`IMAGE`] in the agent; the engine's proxy starts within seconds of the record. A
/// stalled pull is ended and tried again: the agent runs its lines one at a time.
async fn pull(home: &Home, root: &Path) -> String {
    let mut printed = String::new();
    for _ in 0..3 {
        printed = run_in_agent(
            home,
            root,
            &format!("timeout {PULL_SECONDS} docker pull {IMAGE}"),
            CONTAINER_WITHIN,
        )
        .await;
        if exit_code(&printed) == "0" {
            return printed;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    panic!("the pull failed: {printed}; {}", home.stderr());
}

/// The host's `/` bound into a container shows neither the owner's home nor Torlek's state.
/// This test's home is under `/tmp`, which the engine's `PrivateTmp` hides as well; the
/// runner's own home, under `/home` as an owner's is, shows the engine's empty `/home`.
async fn host_root_hides_the_owners_files(home: &Home, root: &Path) {
    let key = home.path().join(".ssh/id_ed25519");
    let state = home.state_dir().join("state.db");
    assert!(key.is_file() && state.is_file(), "the probe's files exist");
    let runner_home = std::env::var("HOME").expect("the runner has a HOME");
    assert!(
        runner_home.starts_with("/home/") && Path::new(&runner_home).is_dir(),
        "the runner's home {runner_home} is not under /home"
    );
    let probes = [
        format!("cat /host{} 2>/dev/null", key.display()),
        format!("test -e /host{} && echo state-db-seen", state.display()),
        format!("test -e /host{runner_home} && echo runner-home-seen"),
    ];
    let printed = run_in_agent(
        home,
        root,
        &container("-v /:/host:ro", &(probes.join("; ") + "; true")),
        CONTAINER_WITHIN,
    )
    .await;
    assert_eq!(exit_code(&printed), "0", "{printed}");
    for seen in [OWNER_KEY, "state-db-seen", "runner-home-seen"] {
        assert!(!printed.contains(seen), "{seen}: {printed}");
    }
}

/// A container working in the checkout cannot write its Git hooks nor read its `.env`.
async fn checkout_masks_hold(home: &Home, root: &Path) {
    let printed = run_in_agent(
        home,
        root,
        &container(
            "--user 0 -v \"$PWD:/work\"",
            "echo hook > /work/.git/hooks/pre-commit; cat /work/.env; true",
        ),
        CONTAINER_WITHIN,
    )
    .await;
    assert_eq!(exit_code(&printed), "0", "{printed}");
    assert!(!root.join(".git/hooks/pre-commit").exists(), "{printed}");
    assert!(!printed.contains(CHECKOUT_SECRET), "{printed}");
}

/// What a container writes in the checkout stays the session's to change and delete.
async fn container_files_stay_the_sessions(home: &Home, root: &Path) {
    let written = run_in_agent(
        home,
        root,
        &container(
            "--user 0 -v \"$PWD:/work\"",
            "echo container > /work/from-container",
        ),
        CONTAINER_WITHIN,
    )
    .await;
    assert_eq!(exit_code(&written), "0", "{written}");
    let changed = run_in_agent(
        home,
        root,
        "echo session >> from-container && cat from-container && rm from-container",
        GENEROUS,
    )
    .await;
    assert_eq!(changed, "container\nsession\nexit=0\n");
    assert!(!root.join("from-container").exists());
}

/// Without the proxy a container reaches no address and resolves no name.
async fn no_direct_route(home: &Home, root: &Path) {
    for target in [DIRECT_ADDRESS, UNKNOWN_HOST] {
        let printed = run_in_agent(
            home,
            root,
            &container(
                "",
                &format!(
                    "curl --noproxy \"*\" -sS -o /dev/null --max-time 10 \
                     -w \"status=%{{http_code}}\\n\" https://{target}/"
                ),
            ),
            CONTAINER_WITHIN,
        )
        .await;
        assert_ne!(exit_code(&printed), "0", "{target}: {printed}");
        assert_eq!(printed_value(&printed, "status="), "000", "{printed}");
    }
}

/// The engine's proxy passes the granted host and refuses the others at once.
async fn proxy_passes_only_grants(home: &Home, root: &Path) {
    let granted = run_in_agent(
        home,
        root,
        &fetch_in_container(GRANTED_HOST),
        CONTAINER_WITHIN,
    )
    .await;
    assert_eq!(printed_value(&granted, "tunnel="), "200", "{granted}");
    assert_ne!(printed_value(&granted, "status="), "000", "{granted}");
    for host in [PROVIDER_HOST, UNKNOWN_HOST] {
        let refused = run_in_agent(home, root, &fetch_in_container(host), CONTAINER_WITHIN).await;
        assert_eq!(
            printed_value(&refused, "tunnel="),
            "403",
            "{host}: {refused}"
        );
        assert_eq!(
            printed_value(&refused, "status="),
            "000",
            "{host}: {refused}"
        );
        let seconds: f64 = printed_value(&refused, "seconds=").parse().unwrap();
        assert!(seconds < 1.0, "{host} was refused after {seconds} s");
    }
}

/// The unknown host is the project's containers' request, for the owner to decide.
async fn unknown_host_is_filed(home: &Home) {
    let target = format!("{UNKNOWN_HOST}:443");
    let containers = NetworkSubjectView::Containers {
        project: COMMANDED_PROJECT.to_owned(),
    };
    let client = home.connect().await;
    let deadline = Instant::now() + GENEROUS;
    loop {
        let reply = client.request(Command::NetworkList, GENEROUS).await;
        let Ok(Reply::NetworkRequests { requests }) = reply else {
            panic!("expected the network requests, got {reply:?}");
        };
        let is_filed = requests
            .iter()
            .any(|request| request.target == target && request.subject == containers);
        if is_filed {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "no containers request for {target}: {requests:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// A container's environment holds the engine's proxy and none of the session's credentials.
async fn container_environment(home: &Home, root: &Path) {
    let printed = run_in_agent(home, root, &container("", "env"), CONTAINER_WITHIN).await;
    assert_eq!(exit_code(&printed), "0", "{printed}");
    assert!(
        printed.contains("HTTPS_PROXY=http://10.0.2.2:3128\n"),
        "{printed}"
    );
    for credential in [
        PROVIDER_TOKEN,
        "CLAUDE_CODE_OAUTH_TOKEN",
        cockpit_sandbox::PLACEHOLDER,
    ] {
        assert!(!printed.contains(credential), "{credential}: {printed}");
    }
}

/// A session of another project reaches no engine: no `DOCKER_HOST`, and neither the
/// session socket nor the first project's engine socket is there to name.
async fn another_project_reaches_no_engine(home: &Home, user: &str) {
    let client = home.connect().await;
    let (root, _) = home
        .project_at(
            &client,
            &format!("second/{COMMANDED_PROJECT}"),
            Some("second"),
        )
        .await;
    new_session(&client, &root).await;
    let printed = run_in_agent(
        home,
        &root,
        &format!(
            "echo \"host=${{DOCKER_HOST-unset}}\"; \
             for socket in {inside} /run/{user}/docker.sock; do \
             test -e \"$socket\" && echo \"seen=$socket\"; done; true",
            inside = cockpit_sandbox::ENGINE_SOCKET_INSIDE
        ),
        GENEROUS,
    )
    .await;
    assert_eq!(printed, "host=unset\nexit=0\n");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live: needs sudo, systemd, rootless Docker and the internet; CI's Linux job runs it (#390)"]
async fn a_projects_engine_runs_its_sessions_containers_inside_the_sandboxs_bounds() {
    let home = Home::new();
    prepare(&home);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    std::fs::write(root.join(".env"), format!("TOKEN={CHECKOUT_SECRET}\n")).unwrap();
    // A project that declares Docker is the one whose engine readiness examines and sets up.
    std::fs::create_dir_all(root.join(".torlek")).unwrap();
    std::fs::write(
        root.join(".torlek/config.toml"),
        "[execution]\nenvironment = \"docker\"\n",
    )
    .unwrap();

    owner_succeeds(&home, &["setup", "engine-masks"]).await;
    let (script, user) = written_setup(&home, &root).await;
    run_as_root(&script).await;
    println!("{}", record_engine(&home, &user).await);
    egress_serves(&home, &root, &user).await;
    owner_succeeds(
        &home,
        &["network", "grant", COMMANDED_PROJECT, "docker-hub"],
    )
    .await;

    new_session(&client, &root).await;
    let reached = run_in_agent(&home, &root, "echo \"$DOCKER_HOST\"", GENEROUS).await;
    assert_eq!(
        reached,
        format!("unix://{}\nexit=0\n", cockpit_sandbox::ENGINE_SOCKET_INSIDE),
        "the launch reached no engine; {}",
        home.stderr()
    );
    println!("{}", pull(&home, &root).await);

    host_root_hides_the_owners_files(&home, &root).await;
    checkout_masks_hold(&home, &root).await;
    container_files_stay_the_sessions(&home, &root).await;
    no_direct_route(&home, &root).await;
    proxy_passes_only_grants(&home, &root).await;
    unknown_host_is_filed(&home).await;
    container_environment(&home, &root).await;
    another_project_reaches_no_engine(&home, &user).await;
}
