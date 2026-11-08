//! The daily digest, built from a recorded two-day fixture whose figures are worked out by hand.
mod common;

use cockpit_provider_api::ProviderName;
use cockpit_usage::{Day, DayDigest, Incident, ProjectDay};
use common::{Fixture, clock, reply, scan_of, tokens};

const OPUS: &str = "claude-opus-5-5";

fn incident(
    provider: &str,
    key: &str,
    title: &str,
    started: i64,
    resolved: Option<i64>,
) -> Incident {
    Incident::new(
        ProviderName::new(provider).unwrap(),
        key,
        title,
        started,
        resolved,
    )
    .unwrap()
}

async fn archive_in(
    fixture: &Fixture,
    project: &'static str,
    replies: Vec<cockpit_provider_api::ModelReply>,
) {
    fixture
        .archive_in_project("main", "owner", &scan_of(replies), &move |_| {
            project.to_owned()
        })
        .await
        .unwrap();
}

/// Monday 2026-11-02: four turns in two projects, three approvals, four replies (one of a model
/// with no price) and two incidents; Tuesday: one short turn, one approval.
async fn recorded_two_days() -> Fixture {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 9, 0), clock(0, 9, 30))
        .await;
    fixture
        .turn("s2", "app", clock(0, 9, 20), clock(0, 9, 40))
        .await;
    fixture
        .turn("s1", "app", clock(0, 14, 0), clock(0, 14, 10))
        .await;
    fixture
        .turn("s3", "docs", clock(0, 9, 35), clock(0, 9, 50))
        .await;
    fixture
        .turn("s1", "app", clock(1, 8, 0), clock(1, 8, 5))
        .await;

    fixture.approval("a1", "app", clock(0, 11, 0)).await;
    fixture.approval("a2", "app", clock(0, 11, 5)).await;
    fixture.approval("a3", "docs", clock(0, 12, 0)).await;
    fixture.approval("a4", "app", clock(1, 8, 0)).await;

    archive_in(
        &fixture,
        "app",
        vec![
            reply("m1", clock(0, 9, 10), OPUS, tokens(1000, 100)),
            reply("m2", clock(0, 14, 5), OPUS, tokens(2000, 0)),
        ],
    )
    .await;
    archive_in(
        &fixture,
        "docs",
        vec![
            reply("m3", clock(0, 9, 40), OPUS, tokens(500, 50)),
            reply(
                "m4",
                clock(0, 9, 41),
                "claude-unheard-of-9",
                tokens(100, 10),
            ),
        ],
    )
    .await;

    fixture
        .incident(
            "claude",
            "inc-1",
            "Elevated errors",
            clock(0, 8, 0),
            Some(clock(0, 10, 0)),
        )
        .await;
    fixture
        .incident("codex", "inc-2", "Slow responses", clock(-1, 22, 0), None)
        .await;
    fixture
}

fn project(project: &str) -> ProjectDay {
    ProjectDay {
        project: project.to_owned(),
        ..ProjectDay::default()
    }
}

#[tokio::test]
async fn a_recorded_day_digests_to_the_figures_worked_out_by_hand() {
    let fixture = recorded_two_days().await;

    let digests = fixture.digests_between("2026-11-02", "2026-11-03").await;

    // Opus 5.5 is $4 in and $20 out per million: m1 6 000 000 nano-dollars, m2 8 000 000,
    // m3 3 000 000; m4's model has no price.
    let monday = DayDigest {
        day: Day::parse("2026-11-02").unwrap(),
        sessions: 3,
        turns: 4,
        approvals: 3,
        // 09:00-09:50 and 14:00-14:10.
        seconds_with_agent: 3600,
        replies: 4,
        unpriced_replies: 1,
        cost_nanos: 17_000_000,
        projects: vec![
            ProjectDay {
                sessions: 2,
                turns: 3,
                approvals: 2,
                // 09:00-09:40 and 14:00-14:10.
                seconds_with_agent: 3000,
                replies: 2,
                cost_nanos: 14_000_000,
                ..project("app")
            },
            ProjectDay {
                sessions: 1,
                turns: 1,
                approvals: 1,
                seconds_with_agent: 900,
                replies: 2,
                unpriced_replies: 1,
                cost_nanos: 3_000_000,
                ..project("docs")
            },
        ],
        incidents: vec![
            incident("codex", "inc-2", "Slow responses", clock(-1, 22, 0), None),
            incident(
                "claude",
                "inc-1",
                "Elevated errors",
                clock(0, 8, 0),
                Some(clock(0, 10, 0)),
            ),
        ],
    };
    let tuesday = DayDigest {
        day: Day::parse("2026-11-03").unwrap(),
        sessions: 1,
        turns: 1,
        approvals: 1,
        seconds_with_agent: 300,
        replies: 0,
        unpriced_replies: 0,
        cost_nanos: 0,
        projects: vec![ProjectDay {
            sessions: 1,
            turns: 1,
            approvals: 1,
            seconds_with_agent: 300,
            ..project("app")
        }],
        incidents: vec![incident(
            "codex",
            "inc-2",
            "Slow responses",
            clock(-1, 22, 0),
            None,
        )],
    };
    assert_eq!(digests, [monday, tuesday]);
}

#[tokio::test]
async fn a_day_with_nothing_recorded_is_a_digest_of_zeros() {
    let fixture = recorded_two_days().await;

    let digests = fixture.digests_between("2026-11-04", "2026-11-04").await;

    let wednesday = &digests[0];
    assert_eq!(wednesday.day, Day::parse("2026-11-04").unwrap());
    assert_eq!(
        (
            wednesday.sessions,
            wednesday.turns,
            wednesday.approvals,
            wednesday.seconds_with_agent,
            wednesday.replies,
            wednesday.cost_nanos
        ),
        (0, 0, 0, 0, 0, 0)
    );
    assert!(wednesday.projects.is_empty());
}

#[tokio::test]
async fn a_project_with_only_cost_is_in_the_digest() {
    let fixture = Fixture::new().await;
    archive_in(
        &fixture,
        "app",
        vec![reply("m1", clock(0, 9, 10), OPUS, tokens(1000, 100))],
    )
    .await;

    let digests = fixture.digests_between("2026-11-02", "2026-11-02").await;

    assert_eq!(
        digests[0].projects,
        [ProjectDay {
            replies: 1,
            cost_nanos: 6_000_000,
            ..project("app")
        }]
    );
}

#[tokio::test]
async fn a_turn_counts_on_the_day_it_began_and_its_time_on_the_days_it_ran() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 23, 50), clock(1, 0, 20))
        .await;

    let digests = fixture.digests_between("2026-11-02", "2026-11-03").await;

    let figures: Vec<_> = digests
        .iter()
        .map(|digest| (digest.turns, digest.seconds_with_agent))
        .collect();
    assert_eq!(figures, [(1, 600), (0, 1200)]);
}

#[tokio::test]
async fn an_approval_recorded_again_counts_once() {
    let fixture = Fixture::new().await;

    let first = fixture.approval("a1", "app", clock(0, 11, 0)).await;
    let replay = fixture.approval("a1", "app", clock(0, 11, 0)).await;

    assert!((first, replay) == (true, false));
    assert_eq!(
        fixture.digests_between("2026-11-02", "2026-11-02").await[0].approvals,
        1
    );
}

#[tokio::test]
async fn a_later_status_resolves_an_incident_and_an_older_one_never_reopens_it() {
    let fixture = Fixture::new().await;
    fixture
        .incident("claude", "inc-1", "Investigating", clock(0, 8, 0), None)
        .await;
    fixture
        .incident(
            "claude",
            "inc-1",
            "Resolved: elevated errors",
            clock(0, 8, 0),
            Some(clock(0, 10, 0)),
        )
        .await;
    fixture
        .incident("claude", "inc-1", "Investigating", clock(0, 8, 0), None)
        .await;

    let digests = fixture.digests_between("2026-11-02", "2026-11-03").await;

    // Under way a while on Monday only, the latest title; the late "unresolved" kept the end.
    assert_eq!(digests[0].incidents.len(), 1);
    assert_eq!(digests[0].incidents[0].resolved_at(), Some(clock(0, 10, 0)));
    assert!(digests[1].incidents.is_empty());
}

#[tokio::test]
async fn an_incident_resolved_at_midnight_is_not_on_the_next_day() {
    let fixture = Fixture::new().await;
    fixture
        .incident(
            "claude",
            "inc-1",
            "Elevated errors",
            clock(0, 22, 0),
            Some(clock(1, 0, 0)),
        )
        .await;

    let digests = fixture.digests_between("2026-11-02", "2026-11-03").await;

    assert_eq!(
        digests
            .iter()
            .map(|digest| digest.incidents.len())
            .collect::<Vec<_>>(),
        [1, 0]
    );
}

#[tokio::test]
async fn an_incident_that_began_and_ended_at_midnight_is_on_its_day() {
    let fixture = Fixture::new().await;
    fixture
        .incident(
            "claude",
            "inc-1",
            "Blip",
            clock(1, 0, 0),
            Some(clock(1, 0, 0)),
        )
        .await;

    let digests = fixture.digests_between("2026-11-02", "2026-11-03").await;

    assert_eq!(
        digests
            .iter()
            .map(|digest| digest.incidents.len())
            .collect::<Vec<_>>(),
        [0, 1]
    );
}

#[tokio::test]
async fn the_digest_survives_a_restart() {
    let fixture = recorded_two_days().await;
    let before = fixture.digests_between("2026-11-02", "2026-11-03").await;

    let fixture = fixture.reopened().await;

    assert_eq!(
        fixture.digests_between("2026-11-02", "2026-11-03").await,
        before
    );
}

#[tokio::test]
async fn an_incidents_first_resolution_is_kept() {
    let fixture = Fixture::new().await;
    fixture
        .incident(
            "claude",
            "inc-1",
            "Elevated errors",
            clock(0, 8, 0),
            Some(clock(0, 10, 0)),
        )
        .await;
    fixture
        .incident(
            "claude",
            "inc-1",
            "Elevated errors",
            clock(0, 8, 0),
            Some(clock(0, 11, 0)),
        )
        .await;

    let digests = fixture.digests_between("2026-11-02", "2026-11-02").await;

    assert_eq!(digests[0].incidents[0].resolved_at(), Some(clock(0, 10, 0)));
}

#[tokio::test]
async fn an_incident_whose_start_is_corrected_earlier_takes_the_earlier_start() {
    let fixture = Fixture::new().await;
    fixture
        .incident("claude", "inc-1", "Investigating", clock(0, 10, 0), None)
        .await;
    fixture
        .incident(
            "claude",
            "inc-1",
            "Resolved",
            clock(0, 8, 0),
            Some(clock(0, 9, 0)),
        )
        .await;

    let digests = fixture.digests_between("2026-11-02", "2026-11-02").await;

    let incident = &digests[0].incidents[0];
    assert_eq!(
        (incident.started_at(), incident.resolved_at()),
        (clock(0, 8, 0), Some(clock(0, 9, 0)))
    );
}

#[tokio::test]
async fn an_approval_before_the_year_2000_is_refused() {
    assert!(cockpit_usage::Approval::new("a1", "app", 0).is_err());
}
