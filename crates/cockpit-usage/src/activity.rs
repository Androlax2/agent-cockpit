use std::collections::BTreeMap;

use cockpit_provider_api::ProviderName;
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, params};

use crate::cost::attributed_project;
use crate::day::{END_RECORDABLE_SECOND, FIRST_RECORDABLE_SECOND, SECONDS_PER_DAY};
use crate::error::{invalid, invalid_row};
use crate::{Day, Error};

/// Turns closer than this are one stretch of time with the agent: the owner reading a reply
/// or typing the next prompt is still time with it (report 18, as the legacy cockpit counted).
/// A gap of exactly this long is not joined.
pub const JOIN_GAP_SECONDS: i64 = 300;
/// The longest turn a record may claim: a clock gone wrong must not fill every day after it
/// with time with the agent.
pub const MAX_TURN_SECONDS: i64 = 7 * SECONDS_PER_DAY;
const MAX_KEY_BYTES: usize = 256;

/// One turn of one session, recorded when it ends: the span the agent worked for a prompt.
/// Turn records are the only source of time with the agent, so nothing is polled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    session: String,
    project: String,
    started_at: i64,
    ended_at: i64,
}

impl Turn {
    /// `session` names the session in the caller's own words and is the same whenever the turn
    /// is recorded again; with `started_at` (Unix seconds) it names the turn. A `project` that
    /// is not a plain name is kept as `unknown`, as the cost archive does. Refuses a turn that
    /// ends before it starts, lasts over [`MAX_TURN_SECONDS`], or names a time before the year
    /// 2000 or past 9999.
    pub fn new(
        session: impl Into<String>,
        project: impl Into<String>,
        started_at: i64,
        ended_at: i64,
    ) -> Result<Turn, Error> {
        let session = session.into();
        check_key("session", &session)?;
        check_time("turn start", started_at)?;
        check_time("turn end", ended_at)?;
        if ended_at < started_at || ended_at - started_at > MAX_TURN_SECONDS {
            return Err(invalid(
                "turn",
                format!(
                    "{started_at} to {ended_at} is not at most {MAX_TURN_SECONDS} seconds forward"
                ),
            ));
        }
        Ok(Turn {
            session,
            project: attributed_project(project.into()),
            started_at,
            ended_at,
        })
    }
}

/// One approval the owner decided, in the project it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    key: String,
    project: String,
    at: i64,
}

impl Approval {
    /// `key` names the approval in the caller's own words and is the same whenever it is
    /// recorded again; `at` is when it was decided, in Unix seconds.
    pub fn new(
        key: impl Into<String>,
        project: impl Into<String>,
        at: i64,
    ) -> Result<Approval, Error> {
        let key = key.into();
        check_key("approval key", &key)?;
        check_time("approval time", at)?;
        Ok(Approval {
            key,
            project: attributed_project(project.into()),
            at,
        })
    }
}

/// A provider's incident, from the start the status page gave until it was resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incident {
    provider: ProviderName,
    key: String,
    title: String,
    started_at: i64,
    resolved_at: Option<i64>,
}

impl Incident {
    /// `key` names the incident within its provider and is the same whenever its status is
    /// recorded again, so a later record can resolve it. Refuses an empty or overlong key or
    /// title, control characters in either, a resolution before the start, and a time before
    /// the year 2000 or past 9999.
    pub fn new(
        provider: ProviderName,
        key: impl Into<String>,
        title: impl Into<String>,
        started_at: i64,
        resolved_at: Option<i64>,
    ) -> Result<Incident, Error> {
        let key = key.into();
        let title = title.into();
        check_key("incident key", &key)?;
        check_key("incident title", &title)?;
        check_time("incident start", started_at)?;
        if let Some(resolved_at) = resolved_at {
            check_time("incident resolution", resolved_at)?;
        }
        if resolved_at.is_some_and(|resolved| resolved < started_at) {
            return Err(invalid(
                "incident",
                format!("resolved before it started at {started_at}"),
            ));
        }
        Ok(Incident {
            provider,
            key,
            title,
            started_at,
            resolved_at,
        })
    }

    pub fn provider(&self) -> &ProviderName {
        &self.provider
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn started_at(&self) -> i64 {
        self.started_at
    }

    pub fn resolved_at(&self) -> Option<i64> {
        self.resolved_at
    }

    /// Whether the incident was under way at any moment of `day`, or began on it.
    pub(crate) fn touches(&self, day: Day) -> bool {
        let day_start = day.first_second();
        let day_end = day_start + SECONDS_PER_DAY;
        let began_on_day = (day_start..day_end).contains(&self.started_at);
        let was_under_way = self.started_at < day_start
            && self
                .resolved_at
                .is_none_or(|resolved_at| resolved_at > day_start);
        began_on_day || was_under_way
    }
}

/// Time with the agent on one day: the whole day's, and each project's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayTime {
    pub day: Day,
    /// Every project together: stretches of time that overlap are counted once, never summed.
    pub seconds: u64,
    /// Each project that had time, by name: a project's overlapping sessions are counted once
    /// for it. Two projects worked at once each show their time, so these can add up to more
    /// than `seconds`.
    pub projects: Vec<ProjectTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTime {
    pub project: String,
    pub seconds: u64,
}

/// Records a turn. A turn already recorded (same session and start) is kept, its end raised to
/// the later of the two, so a replay after a restart counts nothing twice and a turn recorded
/// again with its real end gets it. Returns whether this turn was new.
pub async fn record_turn(write: &WriteTransaction<'_>, turn: &Turn) -> Result<bool, Error> {
    let mut rows = write
        .query(
            "SELECT ended_at FROM usage_turns WHERE session = ?1 AND started_at = ?2",
            params![turn.session.as_str(), turn.started_at],
        )
        .await?;
    let recorded_end: Option<i64> = match rows.next().await? {
        Some(row) => Some(row.get(0)?),
        None => None,
    };
    drop(rows);
    match recorded_end {
        None => {
            write
                .execute(
                    "INSERT INTO usage_turns (session, project, day, started_at, ended_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        turn.session.as_str(),
                        turn.project.as_str(),
                        Day::containing(turn.started_at).to_string(),
                        turn.started_at,
                        turn.ended_at
                    ],
                )
                .await?;
            Ok(true)
        }
        Some(recorded_end) => {
            if turn.ended_at > recorded_end {
                write
                    .execute(
                        "UPDATE usage_turns SET ended_at = ?3 WHERE session = ?1 AND started_at = ?2",
                        params![turn.session.as_str(), turn.started_at, turn.ended_at],
                    )
                    .await?;
            }
            Ok(false)
        }
    }
}

/// A turn under way: a session working since `started_at`, not yet ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTurn {
    pub session: String,
    pub project: String,
    pub started_at: i64,
}

/// Notes that `session` began working on `project` at `at`. A turn already open for the session
/// is kept as it is (a session reports working again while its turn runs); returns whether this
/// one opened it.
pub async fn turn_began(
    write: &WriteTransaction<'_>,
    session: &str,
    project: &str,
    at: i64,
) -> Result<bool, Error> {
    check_key("session", session)?;
    check_time("turn start", at)?;
    let opened = write
        .execute(
            "INSERT INTO usage_open_turns (session, project, started_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (session) DO NOTHING",
            params![session, attributed_project(project.to_owned()), at],
        )
        .await?;
    Ok(opened == 1)
}

/// Ends the turn open for `session` at `at` and records it as [`record_turn`] does. Returns
/// `false`, changing nothing, when none is open. An end before the start is taken as the start.
/// The open turn is dropped even when its span is refused (a turn over [`MAX_TURN_SECONDS`]),
/// so one bad record is not retried forever; the refusal is returned.
pub async fn turn_ended(
    write: &WriteTransaction<'_>,
    session: &str,
    at: i64,
) -> Result<bool, Error> {
    check_key("session", session)?;
    let mut rows = write
        .query(
            "SELECT project, started_at FROM usage_open_turns WHERE session = ?1",
            params![session],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(false);
    };
    let (project, started_at): (String, i64) = (row.get(0)?, row.get(1)?);
    drop(rows);
    write
        .execute(
            "DELETE FROM usage_open_turns WHERE session = ?1",
            params![session],
        )
        .await?;
    let turn = Turn::new(session, project, started_at, at.max(started_at))?;
    record_turn(write, &turn).await?;
    Ok(true)
}

/// The turns under way, by start.
pub async fn open_turns(connection: &Connection) -> Result<Vec<OpenTurn>, Error> {
    let mut rows = connection
        .query(
            "SELECT session, project, started_at FROM usage_open_turns
             ORDER BY started_at, session",
            (),
        )
        .await?;
    let mut turns = Vec::new();
    while let Some(row) = rows.next().await? {
        turns.push(OpenTurn {
            session: row.get(0)?,
            project: row.get(1)?,
            started_at: row.get(2)?,
        });
    }
    Ok(turns)
}

/// Records a decided approval; one already recorded under its key is kept as it was. Returns
/// whether this one was new.
pub async fn record_approval(
    write: &WriteTransaction<'_>,
    approval: &Approval,
) -> Result<bool, Error> {
    let added = write
        .execute(
            "INSERT INTO usage_approvals (key, project, day, at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (key) DO NOTHING",
            params![
                approval.key.as_str(),
                approval.project.as_str(),
                Day::containing(approval.at).to_string(),
                approval.at
            ],
        )
        .await?;
    Ok(added == 1)
}

/// Records an incident, or its newer status: the title is replaced, the first resolution
/// recorded is kept, and the start is the earliest recorded.
pub async fn record_incident(
    write: &WriteTransaction<'_>,
    incident: &Incident,
) -> Result<(), Error> {
    write
        .execute(
            "INSERT INTO usage_incidents (provider, key, title, started_at, resolved_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (provider, key) DO UPDATE SET title = excluded.title,
                started_at = MIN(started_at, excluded.started_at),
                resolved_at = COALESCE(resolved_at, excluded.resolved_at)",
            params![
                incident.provider.as_str(),
                incident.key.as_str(),
                incident.title.as_str(),
                incident.started_at,
                incident.resolved_at
            ],
        )
        .await?;
    Ok(())
}

/// The incidents under way at any moment of the days from `first` to `last`, or begun on one,
/// by start. An incident never recorded as resolved stays under way.
pub(crate) async fn incidents(
    connection: &Connection,
    first: Day,
    last: Day,
) -> Result<Vec<Incident>, Error> {
    let mut rows = connection
        .query(
            "SELECT provider, key, title, started_at, resolved_at FROM usage_incidents
             WHERE started_at < ?2 AND (resolved_at IS NULL OR resolved_at >= ?1)
             ORDER BY started_at, provider, key",
            params![first.first_second(), last.first_second() + SECONDS_PER_DAY],
        )
        .await?;
    let mut incidents = Vec::new();
    while let Some(row) = rows.next().await? {
        incidents.push(
            Incident::new(
                ProviderName::new(row.get::<String>(0)?)
                    .map_err(|error| invalid_row(format!("provider: {error}")))?,
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get(3)?,
                row.get(4)?,
            )
            .map_err(|error| invalid_row(format!("incident: {error}")))?,
        );
    }
    Ok(incidents)
}

/// Time with the agent for each day from `first` to `last`, both included, oldest first, a day
/// without time included as zero.
///
/// A stretch is the turns of a project (or of every project, for the day's own figure) with
/// gaps under [`JOIN_GAP_SECONDS`] joined, the joined gaps counted as time. A stretch that
/// crosses midnight is split at it: each UTC day has the seconds that fell on it.
pub async fn time_with_agent(
    connection: &Connection,
    first: Day,
    last: Day,
) -> Result<Vec<DayTime>, Error> {
    let days = Day::range(first, last)?;
    let window_start = first.first_second();
    let window_end = last.first_second() + SECONDS_PER_DAY;

    // A turn up to a gap outside the window can still join one inside it.
    let mut rows = connection
        .query(
            "SELECT project, started_at, ended_at FROM usage_turns
             WHERE started_at <= ?2 AND ended_at >= ?1",
            params![
                window_start - JOIN_GAP_SECONDS,
                window_end + JOIN_GAP_SECONDS
            ],
        )
        .await?;
    let mut every_turn = Vec::new();
    let mut turns_by_project: BTreeMap<String, Vec<(i64, i64)>> = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        let span = (row.get::<i64>(1)?, row.get::<i64>(2)?);
        every_turn.push(span);
        turns_by_project
            .entry(row.get::<String>(0)?)
            .or_default()
            .push(span);
    }

    let mut times: Vec<DayTime> = days
        .iter()
        .map(|day| DayTime {
            day: *day,
            seconds: 0,
            projects: Vec::new(),
        })
        .collect();
    let overall = seconds_per_day(&every_turn, window_start, days.len());
    for (time, seconds) in times.iter_mut().zip(overall) {
        time.seconds = seconds;
    }
    for (project, spans) in &turns_by_project {
        let project_seconds = seconds_per_day(spans, window_start, days.len());
        for (time, seconds) in times.iter_mut().zip(project_seconds) {
            if seconds > 0 {
                time.projects.push(ProjectTime {
                    project: project.clone(),
                    seconds,
                });
            }
        }
    }
    Ok(times)
}

/// The seconds of the joined stretches of `spans` that fall on each of `day_count` days from
/// `window_start`.
fn seconds_per_day(spans: &[(i64, i64)], window_start: i64, day_count: usize) -> Vec<u64> {
    let window_end = window_start + day_count as i64 * SECONDS_PER_DAY;
    let mut seconds = vec![0_u64; day_count];
    for (start, end) in joined(spans) {
        let (start, end) = (start.max(window_start), end.min(window_end));
        if end <= start {
            continue;
        }
        let first_day = (start - window_start) / SECONDS_PER_DAY;
        let last_day = (end - 1 - window_start) / SECONDS_PER_DAY;
        for day in first_day..=last_day {
            let day_start = window_start + day * SECONDS_PER_DAY;
            let overlap = end.min(day_start + SECONDS_PER_DAY) - start.max(day_start);
            seconds[day as usize] += overlap as u64;
        }
    }
    seconds
}

/// `spans` as disjoint stretches, by start: spans that overlap, or leave a gap under
/// [`JOIN_GAP_SECONDS`], become one.
fn joined(spans: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut sorted = spans.to_vec();
    sorted.sort_unstable();
    let mut stretches: Vec<(i64, i64)> = Vec::new();
    for (start, end) in sorted {
        match stretches.last_mut() {
            Some((_, stretch_end)) if start - *stretch_end < JOIN_GAP_SECONDS => {
                *stretch_end = (*stretch_end).max(end);
            }
            _ => stretches.push((start, end)),
        }
    }
    stretches
}

fn check_key(field: &'static str, text: &str) -> Result<(), Error> {
    if text.is_empty() || text.len() > MAX_KEY_BYTES || text.chars().any(char::is_control) {
        return Err(invalid(
            field,
            "1 to 256 bytes of text without control characters",
        ));
    }
    Ok(())
}

fn check_time(field: &'static str, at: i64) -> Result<(), Error> {
    if !(FIRST_RECORDABLE_SECOND..END_RECORDABLE_SECOND).contains(&at) {
        return Err(invalid(
            field,
            format!("{at} is not from the year 2000 to 9999"),
        ));
    }
    Ok(())
}
