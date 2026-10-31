//! `ask guide`: the step format, one step at a time, stuck notes and `run:` steps, on a real
//! state database.

mod common;

use cockpit_approvals::{
    Error, GuideId, GuideRequest, GuideState, Refusal, RunState, answer_step, ask_guide,
    close_guide, guide_request, guide_status, open_guides, parse_guide, report_stuck, run_request,
};
use common::{Fixture, NOW, session};

const GUIDE: &str = "\
# Connect the GitHub app

## Create the app
Open the developer settings
  open: https://github.com/settings/apps/new
Name the app and copy its id
  copy: cockpit-dev
  ask: What id did GitHub give the app?

## Install
Install the app's token helper
  run: make -C ~/agent-cockpit agent-github-token PROJECT=demo
";

fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> Refusal {
    match result {
        Err(Error::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn asked(fixture: &Fixture, text: &str) -> GuideRequest {
    let owner = session("s1");
    fixture.grant(&owner).await;
    let guide = parse_guide(text).unwrap();
    let write = fixture.database.write().await.unwrap();
    let asked = ask_guide(&write, &owner, &guide, fixture.cwd(), None, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
    asked
}

#[test]
fn a_guide_reads_its_title_sections_steps_and_their_lines() {
    let guide = parse_guide(GUIDE).unwrap();
    assert_eq!(guide.title, "Connect the GitHub app");
    assert_eq!(guide.steps.len(), 3);
    let first = &guide.steps[0];
    assert_eq!(first.section.as_deref(), Some("Create the app"));
    assert_eq!(first.text, "Open the developer settings");
    assert_eq!(first.open, ["https://github.com/settings/apps/new"]);
    let second = &guide.steps[1];
    assert_eq!(second.copy, ["cockpit-dev"]);
    assert_eq!(
        second.ask.as_deref(),
        Some("What id did GitHub give the app?")
    );
    let third = &guide.steps[2];
    assert_eq!(third.section.as_deref(), Some("Install"));
    assert_eq!(
        third.run.as_deref(),
        Some("make -C ~/agent-cockpit agent-github-token PROJECT=demo")
    );
}

#[test]
fn a_guide_the_owner_could_not_follow_is_refused_with_its_line() {
    let cases = [
        ("", "is empty"),
        ("Step without a title\n", "must start with a `# Title` line"),
        ("# Title\n", "has no step"),
        (
            "# Title\n  open: https://example.com\n",
            "line 2: an indented line must follow a step",
        ),
        (
            "# Title\nStep\n  look: here\n",
            "line 3: `look:` is not one of open, copy, ask, run",
        ),
        (
            "# Title\nStep\n  no key here\n",
            "line 3: an indented line is",
        ),
        (
            "# Title\nStep\n  ask: one\n  ask: two\n",
            "line 4: a step has one `ask:` at most",
        ),
        ("# Title\nStep\n  run:   \n", "line 3: is empty"),
        (
            "# Title\n# Another title\nStep\n",
            "line 2: only one `# Title` line",
        ),
    ];
    for (text, expected) in cases {
        match parse_guide(text) {
            Err(Refusal::InvalidRequest {
                field: "guide",
                reason,
            }) => {
                assert!(reason.contains(expected), "{text:?}: {reason}");
            }
            other => panic!("{text:?}: expected an invalid guide, got {other:?}"),
        }
    }
}

#[test]
fn a_guide_over_its_limits_is_refused() {
    let many_steps = format!("# Title\n{}", "Step\n".repeat(101));
    let long_line = format!("# Title\n{}\n", "x".repeat(1001));
    let huge = format!("# Title\n{}", "Step\n".repeat(64 * 1024 / 5 + 1));
    for text in [many_steps, long_line, huge] {
        assert!(
            matches!(
                parse_guide(&text),
                Err(Refusal::InvalidRequest { field: "guide", .. })
            ),
            "{} bytes",
            text.len()
        );
    }
}

#[tokio::test]
async fn a_session_without_the_run_commands_grant_cannot_ask_a_guide() {
    let fixture = Fixture::new().await;
    let guide = parse_guide(GUIDE).unwrap();
    let write = fixture.database.write().await.unwrap();
    assert!(matches!(
        refusal(ask_guide(&write, &session("s1"), &guide, fixture.cwd(), None, NOW).await),
        Refusal::NotAuthorized(_)
    ));
}

#[tokio::test]
async fn an_asked_guide_starts_at_its_first_step_with_every_line_kept() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    assert_eq!(guide.state, GuideState::Asking);
    assert_eq!(guide.current_step, 0);
    assert_eq!(
        guide
            .steps
            .iter()
            .map(|step| step.step.clone())
            .collect::<Vec<_>>(),
        parse_guide(GUIDE).unwrap().steps
    );
    let read = fixture.database.read().await.unwrap();
    assert_eq!(open_guides(&read, NOW).await.unwrap(), [guide]);
}

#[tokio::test]
async fn only_the_step_shown_now_can_be_answered() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        refusal(answer_step(&write, guide.id, 1, Some("42".to_owned()), NOW).await),
        Refusal::StaleStep { current: 0 }
    );
    let moved = answer_step(&write, guide.id, 0, None, NOW).await.unwrap();
    assert_eq!(moved.current_step, 1);
    assert_eq!(
        refusal(answer_step(&write, guide.id, 0, None, NOW).await),
        Refusal::StaleStep { current: 1 },
        "an answer replayed for a step already done is refused"
    );
}

#[tokio::test]
async fn a_step_that_asks_a_question_needs_an_answer() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    let write = fixture.database.write().await.unwrap();
    answer_step(&write, guide.id, 0, None, NOW).await.unwrap();
    assert!(matches!(
        refusal(answer_step(&write, guide.id, 1, None, NOW).await),
        Refusal::InvalidRequest {
            field: "answer",
            ..
        }
    ));
    let answered = answer_step(&write, guide.id, 1, Some(" 1234 ".to_owned()), NOW)
        .await
        .unwrap();
    assert_eq!(answered.steps[1].answer.as_deref(), Some("1234"));
}

#[tokio::test]
async fn a_run_step_files_its_command_as_a_run_request_of_the_guides_session() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    let write = fixture.database.write().await.unwrap();
    answer_step(&write, guide.id, 0, None, NOW).await.unwrap();
    answer_step(&write, guide.id, 1, Some("1234".to_owned()), NOW)
        .await
        .unwrap();
    let done = answer_step(&write, guide.id, 2, None, NOW).await.unwrap();
    assert_eq!(done.state, GuideState::Done);

    let filed = done.steps[2]
        .run_request
        .expect("the run step filed a request");
    let run = run_request(&write, filed, NOW).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Asking, "answering does not run it");
    assert_eq!(run.session, session("s1"));
    assert_eq!(
        run.command,
        "make -C ~/agent-cockpit agent-github-token PROJECT=demo"
    );
    assert_eq!(
        run.reason,
        "Step 3 of the guide \"Connect the GitHub app\": Install the app's token helper"
    );
}

#[tokio::test]
async fn being_stuck_ends_the_guide_with_the_step_and_the_note() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    let write = fixture.database.write().await.unwrap();
    answer_step(&write, guide.id, 0, None, NOW).await.unwrap();
    let stuck = report_stuck(&write, guide.id, 1, "GitHub shows no app id", NOW)
        .await
        .unwrap();
    assert_eq!(stuck.state, GuideState::Stuck);
    assert_eq!(stuck.current_step, 1);
    assert_eq!(stuck.stuck_note.as_deref(), Some("GitHub shows no app id"));
    assert_eq!(
        refusal(answer_step(&write, guide.id, 1, Some("1".to_owned()), NOW).await),
        Refusal::AlreadyDecided { state: "stuck" }
    );
}

#[tokio::test]
async fn a_closed_guide_takes_no_more_answers() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        close_guide(&write, guide.id, NOW).await.unwrap().state,
        GuideState::Closed
    );
    assert_eq!(
        refusal(answer_step(&write, guide.id, 0, None, NOW).await),
        Refusal::AlreadyDecided { state: "closed" }
    );
    assert!(open_guides(&write, NOW).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_guide_nobody_waits_for_expires_unless_its_session_asks_about_it() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    let expiry = NOW + cockpit_approvals::EXPIRES_AFTER_SECONDS;
    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        refusal(answer_step(&write, guide.id, 0, None, expiry).await),
        Refusal::Expired
    );
    guide_status(&write, &session("s1"), guide.id, expiry - 1)
        .await
        .unwrap();
    answer_step(&write, guide.id, 0, None, expiry)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_run_step_request_stays_before_the_owner_while_the_agent_waits_on_the_guide() {
    let fixture = Fixture::new().await;
    let guide = asked(
        &fixture,
        "# Build\nBuild it\n  run: make build\nCheck the result\n",
    )
    .await;
    let expiry = NOW + cockpit_approvals::EXPIRES_AFTER_SECONDS;
    let write = fixture.database.write().await.unwrap();
    let answered = answer_step(&write, guide.id, 0, None, NOW).await.unwrap();
    let filed = answered.steps[0]
        .run_request
        .expect("the run step filed a request");

    guide_status(&write, &session("s1"), guide.id, expiry - 1)
        .await
        .unwrap();

    let run = run_request(&write, filed, expiry).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Asking);
}

#[tokio::test]
async fn a_guide_poll_while_the_expiry_is_fresh_writes_nothing() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    let before = fixture.revision().await;

    let write = fixture.database.write().await.unwrap();
    let seen = guide_status(&write, &session("s1"), guide.id, NOW + 59)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(seen, guide);
    assert_eq!(fixture.revision().await, before);
}

#[tokio::test]
async fn the_owner_sees_each_open_guide_with_its_own_steps() {
    let fixture = Fixture::new().await;
    let first = asked(&fixture, "# First\nOne\nTwo\n").await;
    let closed = asked(&fixture, "# Closed\nThree\n").await;
    let last = asked(&fixture, GUIDE).await;
    let write = fixture.database.write().await.unwrap();
    close_guide(&write, closed.id, NOW).await.unwrap();
    write.commit().await.unwrap();

    let read = fixture.database.read().await.unwrap();
    assert_eq!(open_guides(&read, NOW).await.unwrap(), [first, last]);
}

/// The guide `id` as stored at `NOW`, for comparing with what a mutation returned.
async fn stored(write: &cockpit_turso::WriteTransaction<'_>, id: GuideId) -> GuideRequest {
    guide_request(write, id, NOW).await.unwrap().unwrap()
}

/// `GUIDE`'s steps in order: an `open:` step, an `ask:` step, then a `run:` step.
async fn answered_up_to(fixture: &Fixture, last_step: usize) -> (GuideRequest, GuideRequest) {
    let guide = asked(fixture, GUIDE).await;
    let write = fixture.database.write().await.unwrap();
    let mut answered = None;
    for step in 0..=last_step {
        let answer = (step == 1).then(|| "42".to_owned());
        answered = Some(
            answer_step(&write, guide.id, step, answer, NOW)
                .await
                .unwrap(),
        );
    }
    let stored = stored(&write, guide.id).await;
    write.commit().await.unwrap();
    (answered.unwrap(), stored)
}

#[tokio::test]
async fn answering_an_open_step_returns_the_guide_as_it_now_reads() {
    let fixture = Fixture::new().await;
    let (answered, stored) = answered_up_to(&fixture, 0).await;
    assert_eq!(answered, stored);
}

#[tokio::test]
async fn answering_a_question_returns_the_guide_as_it_now_reads() {
    let fixture = Fixture::new().await;
    let (answered, stored) = answered_up_to(&fixture, 1).await;
    assert_eq!(answered, stored);
}

#[tokio::test]
async fn answering_the_last_run_step_returns_the_done_guide_as_it_now_reads() {
    let fixture = Fixture::new().await;
    let (answered, stored) = answered_up_to(&fixture, 2).await;
    assert_eq!(answered.state, GuideState::Done);
    assert_eq!(answered, stored);
}

#[tokio::test]
async fn being_stuck_returns_the_guide_as_it_now_reads() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    let write = fixture.database.write().await.unwrap();
    let reported = report_stuck(&write, guide.id, 0, "no such menu", NOW)
        .await
        .unwrap();
    assert_eq!(reported, stored(&write, guide.id).await);
}

#[tokio::test]
async fn closing_returns_the_guide_as_it_now_reads() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    let write = fixture.database.write().await.unwrap();
    let closed = close_guide(&write, guide.id, NOW).await.unwrap();
    assert_eq!(closed, stored(&write, guide.id).await);
}

#[tokio::test]
async fn another_sessions_guide_is_not_found_for_the_asker() {
    let fixture = Fixture::new().await;
    let guide = asked(&fixture, GUIDE).await;
    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        refusal(guide_status(&write, &session("s2"), guide.id, NOW).await),
        Refusal::NotFound {
            kind: "guide",
            id: guide.id.0
        }
    );
    assert_eq!(
        refusal(close_guide(&write, GuideId(guide.id.0 + 1), NOW).await),
        Refusal::NotFound {
            kind: "guide",
            id: guide.id.0 + 1
        }
    );
}
