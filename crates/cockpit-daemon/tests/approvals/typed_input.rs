//! #1160: what the owner types into an approved command never reaches the output the agent
//! reads or the daemon stores. A terminal with echo on would copy it there, so input goes in
//! only while echo is off, as at a password prompt. The prompts here are fakes; nothing runs
//! `sudo`.
use super::*;

const TYPED_SECRET: &str = "hunter22";

/// A `sudo` stand-in at `<dir>/sudo`: sudo's prompt, the line read with echo off, then the
/// command it was given.
fn fake_sudo(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("sudo");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '[sudo] password for owner: '\nstty -echo\nread -r typed\n\
         stty echo\nprintf '\\n'\nexec \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The run as the agent reads it and as the owner reads it, both from the stored row.
async fn outputs_after_the_end(asking: &Asking, request: i64) -> [String; 2] {
    let to_agent = asking.ended(request).await.result.expect("a result").output;
    let to_owner = shown_to_owner(asking, request)
        .await
        .result
        .expect("a result")
        .output;
    [to_agent, to_owner]
}

#[tokio::test]
async fn a_password_typed_at_a_sudo_prompt_is_not_in_the_output() {
    let asking = started(true).await;
    fake_sudo(asking.cwd.path());
    let request = asking
        .waiting_at_a_prompt("./sudo printf 'as root\\n'")
        .await;

    asking.type_line(request.id, TYPED_SECRET).await;

    for output in outputs_after_the_end(&asking, request.id).await {
        assert!(output.ends_with("as root"), "{output}");
        assert!(!output.contains(TYPED_SECRET), "{output}");
    }
}

#[tokio::test]
async fn sudos_prompt_line_is_not_counted_as_a_secret() {
    let asking = started(true).await;
    fake_sudo(asking.cwd.path());
    let request = asking.waiting_at_a_prompt("./sudo true").await;

    asking.type_line(request.id, TYPED_SECRET).await;

    let result = asking.ended(request.id).await.result.expect("a result");
    assert_eq!(result.redacted, 0, "{}", result.output);
    assert!(
        result.output.contains("[sudo] password for owner:"),
        "{}",
        result.output
    );
}

/// The leak #1160 reported: a line typed again after the prompt read its line and turned
/// echo back on.
#[tokio::test]
async fn input_typed_after_echo_came_back_on_is_refused_and_not_in_the_output() {
    let asking = started(true).await;
    fake_sudo(asking.cwd.path());
    let echo_ready = asking.cwd.path().join("echo-ready");
    let request = asking
        .waiting_at_a_prompt(
            "./sudo sh -c \"printf 'Installing... '; touch echo-ready; sleep 5; echo done\"",
        )
        .await;
    asking.type_line(request.id, TYPED_SECRET).await;
    let deadline = Instant::now() + GENEROUS;
    while !echo_ready.exists() {
        assert!(Instant::now() < deadline, "echo never came back on");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let refusal = refused(
        asking
            .owner
            .request(
                Command::ApprovalInput {
                    request: request.id,
                    text: TYPED_SECRET.to_owned(),
                    shown: false,
                },
                GENEROUS,
            )
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::Conflict);
    assert!(refusal.message.contains("echo"), "{}", refusal.message);
    assert_eq!(refusal.unblock.unwrap().by, UnblockedBy::Owner);
    for output in outputs_after_the_end(&asking, request.id).await {
        assert!(output.ends_with("Installing... done"), "{output}");
        assert!(!output.contains(TYPED_SECRET), "{output}");
    }
}

/// A program that reads keys one at a time, echo off, may print each key itself (readline).
#[tokio::test]
async fn input_to_a_terminal_reading_keys_one_at_a_time_is_refused() {
    let asking = started(true).await;
    let request = asking
        .waiting_at_a_prompt("printf 'Token: '; stty -echo -icanon; sleep 4; stty echo icanon")
        .await;

    let refusal = refused(
        asking
            .owner
            .request(
                Command::ApprovalInput {
                    request: request.id,
                    text: TYPED_SECRET.to_owned(),
                    shown: false,
                },
                GENEROUS,
            )
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::Conflict);
    assert!(
        refusal.message.contains("one at a time"),
        "{}",
        refusal.message
    );
}

async fn purge_output(asking: &Asking, request: i64) -> Result<Reply, ClientError> {
    asking
        .owner
        .request(Command::ApprovalPurgeOutput { request }, GENEROUS)
        .await
}

#[tokio::test]
async fn the_owner_purges_one_runs_stored_output() {
    let asking = started(true).await;
    let purged = asking.ran("echo first").await;
    let kept = asking.ran("echo second").await;

    let reply = run(purge_output(&asking, purged.id).await);

    let result = reply.result.expect("a result");
    assert!(result.output_retired, "{result:?}");
    assert_eq!(result.output, "");
    let to_agent = asking.status(purged.id).await.result.expect("a result");
    assert_eq!((to_agent.output.as_str(), to_agent.exit_code), ("", 0));
    assert_eq!(
        asking.status(kept.id).await.result.unwrap().output,
        "second"
    );
}

#[tokio::test]
async fn purging_a_run_twice_answers_it_purged() {
    let asking = started(true).await;
    let finished = asking.ran("echo once").await;
    run(purge_output(&asking, finished.id).await);

    let again = run(purge_output(&asking, finished.id).await);

    assert!(again.result.expect("a result").output_retired);
}

#[tokio::test]
async fn purging_a_request_that_stores_no_output_is_refused() {
    let asking = started(true).await;
    let asked = asking.asked("echo later").await;

    let refusal = refused(purge_output(&asking, asked.id).await);

    assert_eq!(refusal.code, ErrorCode::Conflict);
    assert!(refusal.message.contains("asking"), "{}", refusal.message);
}

#[tokio::test]
async fn an_agent_cannot_purge_a_runs_output() {
    let asking = started(true).await;
    let finished = asking.ran("echo mine").await;

    let refusal = refused(
        asking
            .agent
            .request(
                Command::ApprovalPurgeOutput {
                    request: finished.id,
                },
                GENEROUS,
            )
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::NotAuthorized);
    assert_eq!(
        asking.status(finished.id).await.result.unwrap().output,
        "mine"
    );
}
