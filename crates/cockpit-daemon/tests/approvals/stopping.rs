//! #1301: the owner stops a running approved command, such as `git push` waiting at a
//! username prompt it will never get an answer to. The stop ends every process of the run,
//! not only its leader, and the agent reads that the owner stopped it. The prompts here are
//! fakes; nothing reaches a network.
use super::*;

/// A shell waits on a child at an echoing prompt, as `git push` waits on `git-remote-https`,
/// while a helper it left in the background records its pid in `helper.pid`.
const WAITS_AT_AN_ECHOING_PROMPT: &str = "sleep 300 & echo $! > helper.pid; \
     sh -c 'printf \"Username for https://example.test: \"; read -r name; echo \"got $name\"'";

/// SIGTERM's exit status, as a shell reports it.
const TERMINATED_EXIT: i32 = 128 + 15;

async fn stop(asking: &Asking, request: i64) -> Result<Reply, ClientError> {
    asking
        .owner
        .request(Command::ApprovalStop { request }, GENEROUS)
        .await
}

/// The helper's pid, once the command wrote it.
fn helper_pid(asking: &Asking) -> u32 {
    let path = asking.cwd.path().join("helper.pid");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .trim()
        .parse()
        .unwrap()
}

/// Whether `pid` still runs: present and not a zombie waiting to be reaped.
fn is_alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, fields)| !fields.starts_with('Z'))
    })
}

async fn gone_in_time(pid: u32) -> bool {
    let deadline = Instant::now() + GENEROUS;
    while is_alive(pid) {
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    true
}

#[tokio::test]
async fn the_owner_stops_a_command_waiting_at_an_echoing_prompt() {
    let asking = started(true).await;
    let request = asking.waiting_at_a_prompt(WAITS_AT_AN_ECHOING_PROMPT).await;

    let stopped = run(stop(&asking, request.id).await);

    assert_eq!(stopped.state, RunStateView::Finished, "{stopped:?}");
    let result = stopped.result.expect("a result");
    assert_eq!(result.exit_code, TERMINATED_EXIT, "{}", result.output);
    assert!(result.stopped_by_owner, "{result:?}");
}

#[tokio::test]
async fn stopping_a_command_ends_every_process_it_started() {
    let asking = started(true).await;
    let request = asking.waiting_at_a_prompt(WAITS_AT_AN_ECHOING_PROMPT).await;
    let helper = helper_pid(&asking);
    assert!(is_alive(helper), "the helper {helper} never ran");

    run(stop(&asking, request.id).await);

    assert!(
        gone_in_time(helper).await,
        "the helper {helper} outlived the stop"
    );
}

#[tokio::test]
async fn the_agent_reads_that_the_owner_stopped_its_command() {
    let asking = started(true).await;
    let request = asking.waiting_at_a_prompt(WAITS_AT_AN_ECHOING_PROMPT).await;

    run(stop(&asking, request.id).await);

    let to_agent = asking.ended(request.id).await;
    assert_eq!(to_agent.state, RunStateView::Finished);
    let result = to_agent.result.expect("a result");
    assert!(result.stopped_by_owner, "{result:?}");
    assert_eq!(result.exit_code, TERMINATED_EXIT);
    assert!(
        result.output.contains("Username for https://example.test:"),
        "{}",
        result.output
    );
}

#[tokio::test]
async fn a_stopped_command_leaves_the_owners_queue() {
    let asking = started(true).await;
    let request = asking.waiting_at_a_prompt(WAITS_AT_AN_ECHOING_PROMPT).await;

    run(stop(&asking, request.id).await);

    assert!(
        asking.queue().await.iter().all(|run| run.id != request.id),
        "{:?}",
        asking.queue().await
    );
}

/// The refusal the owner met in #1301 names the way out.
#[tokio::test]
async fn refused_input_at_an_echoing_prompt_names_the_stop() {
    let asking = started(true).await;
    let request = asking.waiting_at_a_prompt(WAITS_AT_AN_ECHOING_PROMPT).await;

    let refusal = refused(
        asking
            .owner
            .request(
                Command::ApprovalInput {
                    request: request.id,
                    text: "owner".to_owned(),
                    shown: false,
                },
                GENEROUS,
            )
            .await,
    );

    let stop_verb = format!("torlek approval stop {}", request.id);
    assert!(refusal.message.contains(&stop_verb), "{}", refusal.message);
    assert!(
        refusal.unblock.unwrap().action.contains(&stop_verb),
        "the unblock names the stop too"
    );
    run(stop(&asking, request.id).await);
}

#[tokio::test]
async fn an_agent_cannot_stop_a_running_command() {
    let asking = started(true).await;
    let request = asking.waiting_at_a_prompt(WAITS_AT_AN_ECHOING_PROMPT).await;

    let refusal = refused(
        asking
            .agent
            .request(
                Command::ApprovalStop {
                    request: request.id,
                },
                GENEROUS,
            )
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::NotAuthorized);
    assert_eq!(asking.status(request.id).await.state, RunStateView::Running);
    run(stop(&asking, request.id).await);
}

#[tokio::test]
async fn stopping_a_request_that_has_not_started_is_refused() {
    let asking = started(true).await;
    let asked = asking.asked("echo later").await;

    let refusal = refused(stop(&asking, asked.id).await);

    assert_eq!(refusal.code, ErrorCode::Conflict);
    assert!(refusal.message.contains("asking"), "{}", refusal.message);
    assert_eq!(asking.status(asked.id).await.state, RunStateView::Asking);
}

/// A stop that arrives after the command exited by itself, while a helper it left still holds
/// its terminal open, ends the helper but never claims the owner stopped the command.
#[tokio::test]
async fn a_stop_after_the_command_exited_does_not_mark_it_stopped() {
    let asking = started(true).await;
    let marker = asking.cwd.path().join("helper.pid");
    let request = asking
        .running("sleep 300 & echo $! > helper.pid; exit 0", &marker)
        .await;
    // The exit is observed well within the terminal's drain after it.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let helper = helper_pid(&asking);

    let _ = stop(&asking, request.id).await;

    let result = asking.ended(request.id).await.result.expect("a result");
    assert_eq!(result.exit_code, 0);
    assert!(!result.stopped_by_owner, "{result:?}");
    assert!(
        gone_in_time(helper).await,
        "the helper {helper} outlived the stop"
    );
}
