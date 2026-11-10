//! #1262: quiet progress frames must not become questions for the owner.
use super::*;

#[tokio::test]
async fn progress_redraws_paused_past_the_idle_window_never_wait_for_input() {
    let asking = started(true).await;
    // Each frame stays longer than the prompt idle window, including the initial frame.
    let command = "for frame in '[==========>-----] 38% 74/192' '[########    ]' \
                   '39%' '75/192' '⠋ Working' '|' '/' '-' '\\' \
                   'abc123 Downloading [====>      ]' 'abc123 Extracting' 'abc123 Waiting'; do \
                   printf '\\033[1G\\033[2K%s' \"$frame\"; sleep 2; done; printf '\\n'";
    let request = asking.asked(command).await;
    run(asking.approve(&request).await);

    let deadline = Instant::now() + std::time::Duration::from_secs(45);
    loop {
        let status = shown_to_owner(&asking, request.id).await;
        assert!(!status.waiting_for_input, "{status:?}");
        assert_eq!(status.waiting_prompt, None);
        let queued = asking.queue().await;
        assert!(
            queued.iter().all(|run| !run.waiting_for_input),
            "{queued:?}"
        );
        if status.state == RunStateView::Finished {
            assert_eq!(status.result.unwrap().exit_code, 0);
            break;
        }
        assert!(Instant::now() < deadline, "progress command never ended");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_prompt_after_cursor_control_redraws_shows_only_the_latest_frame() {
    let asking = started(true).await;
    let request = asking
        .waiting_at_a_prompt(
            "printf 'Downloading 100%%\\033[1G\\033[2KOk to proceed? (y) '; read -r answer",
        )
        .await;

    let queue = asking.queue().await;

    assert_eq!(
        queue[0].waiting_prompt.as_ref().unwrap().text,
        "Ok to proceed? (y) "
    );
    asking.answer_shown(request.id, "n").await;
    asking.ended(request.id).await;
}

#[tokio::test]
async fn erasing_to_the_end_of_a_prompt_preserves_its_secret_prefix_for_redaction() {
    let asking = started(true).await;
    let request = asking
        .waiting_at_a_prompt("printf 'token=\\033[Kprivate-value ok? '; read -r answer")
        .await;

    let queue = asking.queue().await;

    assert_eq!(
        queue[0].waiting_prompt.as_ref().unwrap().text,
        "token=[redacted] ok? "
    );
    asking.answer_shown(request.id, "n").await;
    asking.ended(request.id).await;
}
