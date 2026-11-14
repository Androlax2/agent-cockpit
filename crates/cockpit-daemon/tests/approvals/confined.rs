//! #1387: an approved run is confined in a systemd user service of its own. A test home
//! reaches no systemd user manager, so there a confined run is refused, never run
//! unconfined; the tests marked ignored give the daemon this user's real manager, which
//! enforces the limits on a fixture home.
use std::path::PathBuf;

use super::*;

/// The daemon of `home` with confined runs required, as the product default has them.
async fn confining(home: TestHome) -> Asking {
    std::fs::create_dir_all(home.config_file().parent().unwrap()).unwrap();
    std::fs::write(
        home.config_file(),
        "[agents]\nconfined_runs = \"required\"\n",
    )
    .unwrap();
    started_in(home, true).await
}

/// A home whose daemon reaches this user's systemd manager: its runtime folder links the
/// manager's sockets, which `systemctl --user` finds there.
fn with_the_real_manager() -> TestHome {
    let home = TestHome::new();
    let runtime = home.runtime_base.as_ref().unwrap();
    let user_runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
    for socket in ["bus", "systemd"] {
        std::os::unix::fs::symlink(user_runtime.join(socket), runtime.join(socket)).unwrap();
    }
    home
}

#[tokio::test]
async fn a_confined_run_without_a_systemd_manager_is_refused_never_run_unconfined() {
    let asking = confining(TestHome::new()).await;
    let marker = asking.cwd.path().join("ran");

    let ended = asking.ran(&format!("touch '{}'", marker.display())).await;

    assert_eq!(ended.state, RunStateView::NotRun, "{ended:?}");
    let note = ended.note.unwrap_or_default();
    assert!(note.contains("confined"), "{note}");
    assert!(!marker.exists(), "the command ran unconfined");
}

/// `command` asked with `writes` and `hosts` on its card.
async fn ask_reaching(
    asking: &Asking,
    writes: Vec<String>,
    hosts: Vec<String>,
) -> Result<Reply, ClientError> {
    asking
        .agent
        .request(
            Command::AskRun {
                launch_token: Some(asking.token.as_str().to_owned()),
                command: "make fetch".to_owned(),
                cwd: asking.cwd_text(),
                reason: "Proves a confined run's card.".to_owned(),
                writes,
                hosts,
            },
            GENEROUS,
        )
        .await
}

#[tokio::test]
async fn a_card_shows_the_owner_its_folders_resolved_its_hosts_and_the_runs_limits() {
    let asking = confining(TestHome::new()).await;
    let named = asking.cwd.path().join("out");
    std::fs::create_dir(&named).unwrap();
    std::os::unix::fs::symlink(&named, asking.cwd.path().join("link")).unwrap();
    let link = asking.cwd.path().join("link").to_str().unwrap().to_owned();

    let request = run(ask_reaching(&asking, vec![link], vec!["example.com".to_owned()]).await);

    let resolved = named.canonicalize().unwrap().to_str().unwrap().to_owned();
    assert_eq!(request.writes, [resolved.clone()]);
    assert_eq!(request.hosts, ["example.com"]);
    let limits: Vec<&str> = request
        .limits
        .iter()
        .map(|limit| limit.text.as_str())
        .collect();
    assert_eq!(
        limits[0],
        format!("writes only {resolved} and a /tmp of its own")
    );
    assert_eq!(limits[2], "reaches only example.com");
}

#[tokio::test]
async fn a_card_naming_a_relative_folder_is_refused_before_the_owner_is_asked() {
    let asking = confining(TestHome::new()).await;

    let refusal = refused(ask_reaching(&asking, vec!["out".to_owned()], Vec::new()).await);

    assert_eq!(refusal.code, ErrorCode::InvalidArgument);
    assert!(refusal.message.contains("writes"), "{refusal:?}");
}

#[tokio::test]
async fn a_card_naming_no_usable_host_is_refused_before_the_owner_is_asked() {
    let asking = confining(TestHome::new()).await;

    let refusal =
        refused(ask_reaching(&asking, Vec::new(), vec!["*.example.com".to_owned()]).await);

    assert_eq!(refusal.code, ErrorCode::InvalidArgument);
    assert!(refusal.message.contains("hosts"), "{refusal:?}");
}

#[tokio::test]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_run_cannot_read_the_owners_keys() {
    let asking = confining(with_the_real_manager()).await;
    let keys = asking.home.home().join(".ssh");
    std::fs::create_dir(&keys).unwrap();
    std::fs::write(keys.join("id_test"), "the owner's key").unwrap();

    let ended = asking
        .ran(&format!("cat '{}'", keys.join("id_test").display()))
        .await;

    assert_eq!(ended.state, RunStateView::Finished, "{ended:?}");
    let result = ended.result.unwrap();
    assert_ne!(result.exit_code, 0, "{result:?}");
    assert!(result.output.contains("Permission denied"), "{result:?}");
    assert!(!result.output.contains("the owner's key"), "{result:?}");
    assert_eq!(
        result.limit_hits,
        [format!(
            "tried to read {}, which confined runs cannot see",
            keys.join("id_test").display()
        )]
    );
}

#[tokio::test]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_run_writes_its_checkout_only() {
    let asking = confining(with_the_real_manager()).await;
    let root = in_a_registered_project(&asking).await;
    let outside = asking.home.home().join("outside");

    let ended = asking
        .ran(&format!(
            "echo in > '{}' && echo out > '{}'",
            root.join("inside").display(),
            outside.display()
        ))
        .await;

    let result = ended.result.unwrap();
    assert_ne!(result.exit_code, 0, "{result:?}");
    assert!(
        result.output.contains("Read-only file system"),
        "{result:?}"
    );
    assert!(root.join("inside").exists(), "{result:?}");
    assert!(!outside.exists(), "{result:?}");
    assert_eq!(
        result.limit_hits,
        [format!(
            "tried to write {}, outside the folders this run may write",
            outside.display()
        )]
    );
}

#[tokio::test]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_run_writes_a_folder_its_card_names() {
    let asking = confining(with_the_real_manager()).await;
    let named = asking.home.home().join("build-cache");
    std::fs::create_dir(&named).unwrap();

    let ended = ran_reaching(
        &asking,
        &format!("echo cached > '{}'", named.join("entry").display()),
        vec![named.to_str().unwrap().to_owned()],
        Vec::new(),
    )
    .await;

    let result = ended.result.unwrap();
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert!(named.join("entry").exists(), "{result:?}");
}

/// The run's network namespace has a loopback of its own: a listener on the host's is out
/// of its reach, as every other host is.
#[tokio::test]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_run_has_no_network() {
    let asking = confining(with_the_real_manager()).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let ended = asking
        .ran(&format!(
            "curl -sS --max-time 5 http://127.0.0.1:{port}/ && echo connected"
        ))
        .await;

    let result = ended.result.unwrap();
    assert_ne!(result.exit_code, 0, "{result:?}");
    assert!(!result.output.contains("connected"), "{result:?}");
    assert_eq!(
        result.limit_hits,
        ["tried to reach 127.0.0.1, but this run has no network"],
        "{result:?}"
    );
}

/// A host on the test's network that answers each HTTP request with `fixture`. Not on
/// `127.0.0.1`, which the run's proxy variables keep direct.
fn fixture_host(address: &str) -> String {
    let listener = std::net::TcpListener::bind(format!("{address}:0")).unwrap();
    let host = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let mut client = client.unwrap();
            let mut head = [0_u8; 1024];
            let _ = std::io::Read::read(&mut client, &mut head);
            let _ = std::io::Write::write_all(
                &mut client,
                b"HTTP/1.0 200 OK\r\nContent-Length: 7\r\n\r\nfixture",
            );
        }
    });
    host
}

/// `command` asked with `writes` and `hosts` on its card, approved and run to its end.
async fn ran_reaching(
    asking: &Asking,
    command: &str,
    writes: Vec<String>,
    hosts: Vec<String>,
) -> RunRequestView {
    let request = run(asking
        .agent
        .request(
            Command::AskRun {
                launch_token: Some(asking.token.as_str().to_owned()),
                command: command.to_owned(),
                cwd: asking.cwd_text(),
                reason: "Proves a confined run's card.".to_owned(),
                writes,
                hosts,
            },
            GENEROUS,
        )
        .await);
    run(asking.approve(&request).await);
    asking.ended(request.id).await
}

fn curl(host: &str) -> String {
    format!("curl -sS --max-time 10 --proxytunnel http://{host}/")
}

#[tokio::test]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_run_without_hosts_names_the_host_it_could_not_reach() {
    let asking = confining(with_the_real_manager()).await;
    let host = fixture_host("127.0.0.2");

    let ended = asking.ran(&curl(&host)).await;

    let result = ended.result.unwrap();
    assert_ne!(result.exit_code, 0, "{result:?}");
    assert!(!result.output.contains("fixture"), "{result:?}");
    assert_eq!(
        result.limit_hits,
        ["tried to reach 127.0.0.2, but this run has no network"],
        "{result:?}"
    );
}

#[tokio::test]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_run_reaches_a_host_its_card_names() {
    let asking = confining(with_the_real_manager()).await;
    let host = fixture_host("127.0.0.2");

    let ended = ran_reaching(&asking, &curl(&host), Vec::new(), vec![host.clone()]).await;

    assert_eq!(ended.state, RunStateView::Finished, "{ended:?}");
    let result = ended.result.unwrap();
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert!(result.output.contains("fixture"), "{result:?}");
    assert!(result.limit_hits.is_empty(), "{result:?}");
}

#[tokio::test]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_run_names_the_host_its_card_does_not() {
    let asking = confining(with_the_real_manager()).await;
    let named = fixture_host("127.0.0.2");
    let unnamed = fixture_host("127.0.0.3");

    let ended = ran_reaching(&asking, &curl(&unnamed), Vec::new(), vec![named]).await;

    let result = ended.result.unwrap();
    assert_ne!(result.exit_code, 0, "{result:?}");
    assert!(
        result.limit_hits.contains(&format!(
            "tried to reach {unnamed}, which the run's card does not name"
        )),
        "{result:?}"
    );
}
