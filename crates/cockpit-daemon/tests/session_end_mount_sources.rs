//! #1003: a session's end removes its folder of the daemon's mount sources while the daemon's
//! network loop rewrites the decisions file inside it. Through the real daemon binary: a named
//! pipe in the decisions file's place holds the loop's write open, so the end meets it at
//! once rather than by chance.

mod candidate;
mod real_daemon;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use cockpit_protocol::Command;
use real_daemon::{BUILD_PROFILE_DIR, GENEROUS, Home, SETTLE, new_session, wait_for_running};

/// Session `id`'s folder of the daemon's mount sources.
fn mount_sources(home: &Home, id: &str) -> PathBuf {
    home.path()
        .join("run")
        .join(BUILD_PROFILE_DIR)
        .join("mounts")
        .join(id)
}

/// Puts a named pipe at `path`: the next reader waits in it until a writer opens and closes it.
fn make_pipe(path: &Path) {
    let made = Process::new("mkfifo")
        .arg("-m")
        .arg("600")
        .arg(path)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo {}: {made}", path.display());
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_session_waits_for_its_decisions_being_written_and_removes_the_folder() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    wait_for_running(&client, &session.id).await;
    let folder = mount_sources(&home, &session.id);
    std::fs::create_dir_all(&folder).unwrap();
    let decisions = folder.join(cockpit_sandbox::DECISIONS_FILE);
    make_pipe(&decisions);

    // Opening the pipe returns once the loop opened it to compare: its write is under way.
    let opening = tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new().write(true).open(decisions)
    });
    let mut writing = tokio::time::timeout(GENEROUS, opening)
        .await
        .expect("the network loop reads the decisions file")
        .unwrap()
        .unwrap();
    let ending = tokio::spawn({
        let client = home.connect().await;
        let session = session.id.clone();
        async move {
            client
                .request(Command::SessionEnd { session }, GENEROUS)
                .await
        }
    });
    let mut ending = std::pin::pin!(ending);

    let ended_meanwhile = tokio::time::timeout(SETTLE, &mut ending).await;
    assert!(
        ended_meanwhile.is_err(),
        "the end returned while the decisions were being written into {}: {ended_meanwhile:?}",
        folder.display()
    );
    writing.write_all(b"[]").unwrap();
    drop(writing);

    ending.await.unwrap().expect("the owner ends the session");
    assert!(!folder.exists(), "{} is left", folder.display());
}
