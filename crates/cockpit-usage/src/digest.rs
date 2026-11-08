use std::collections::BTreeMap;

use cockpit_turso::sql::{Connection, params};

use crate::activity::{Incident, incidents, time_with_agent};
use crate::error::invalid_row;
use crate::{Day, Error, costs};

/// Everything the owner's digest says about one UTC day (report 18), from the archives alone:
/// a client renders it without asking anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayDigest {
    pub day: Day,
    /// Distinct sessions that began a turn on the day.
    pub sessions: u64,
    /// Turns that began on the day.
    pub turns: u64,
    pub approvals: u64,
    /// Time with the agent, projects together: overlapping stretches are counted once.
    pub seconds_with_agent: u64,
    pub replies: u64,
    /// Replies of a model with no known price: counted in `replies`, absent from `cost_nanos`.
    pub unpriced_replies: u64,
    /// At the provider's API prices, in nano-dollars, exact (see the cost archive).
    pub cost_nanos: u64,
    /// Each project with anything on the day, by name.
    pub projects: Vec<ProjectDay>,
    /// The provider incidents under way at some moment of the day, or begun on it, by start.
    /// One never recorded as resolved is listed on every day since it began.
    pub incidents: Vec<Incident>,
}

/// One project's day. Its figures add up to the day's, except `sessions` (a session that worked
/// in two projects counts in both) and `seconds_with_agent` (projects worked at once overlap).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectDay {
    pub project: String,
    pub sessions: u64,
    pub turns: u64,
    pub approvals: u64,
    pub seconds_with_agent: u64,
    pub replies: u64,
    pub unpriced_replies: u64,
    pub cost_nanos: u64,
}

/// The digest of each day from `first` to `last`, both included, oldest first. A day with
/// nothing is still there, with zeros: a client shows "nothing" without guessing.
pub async fn digests(
    connection: &Connection,
    first: Day,
    last: Day,
) -> Result<Vec<DayDigest>, Error> {
    let times = time_with_agent(connection, first, last).await?;
    let mut digests: BTreeMap<Day, DayDigest> = times
        .iter()
        .map(|time| {
            (
                time.day,
                DayDigest {
                    day: time.day,
                    sessions: 0,
                    turns: 0,
                    approvals: 0,
                    seconds_with_agent: time.seconds,
                    replies: 0,
                    unpriced_replies: 0,
                    cost_nanos: 0,
                    projects: Vec::new(),
                    incidents: Vec::new(),
                },
            )
        })
        .collect();
    let mut projects: BTreeMap<(Day, String), ProjectDay> = BTreeMap::new();
    for time in &times {
        for project_time in &time.projects {
            project_day(&mut projects, time.day, project_time.project.clone()).seconds_with_agent =
                project_time.seconds;
        }
    }
    for cost in costs(connection, first, last).await? {
        let day = digests
            .get_mut(&cost.day)
            .expect("the archive answers the days asked");
        day.replies += cost.replies;
        day.unpriced_replies += cost.unpriced_replies;
        day.cost_nanos += cost.cost_nanos;
        let entry = project_day(&mut projects, cost.day, cost.project);
        entry.replies += cost.replies;
        entry.unpriced_replies += cost.unpriced_replies;
        entry.cost_nanos += cost.cost_nanos;
    }

    let (first_text, last_text) = (first.to_string(), last.to_string());
    let mut rows = connection
        .query(
            "SELECT day, project, COUNT(*), COUNT(DISTINCT session) FROM usage_turns
             WHERE day >= ?1 AND day <= ?2 GROUP BY day, project",
            params![first_text.as_str(), last_text.as_str()],
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let day = stored_day(&row.get::<String>(0)?)?;
        let (turns, sessions) = (count(row.get(2)?)?, count(row.get(3)?)?);
        let entry = project_day(&mut projects, day, row.get::<String>(1)?);
        entry.turns = turns;
        entry.sessions = sessions;
        digests.get_mut(&day).expect("asked").turns += turns;
    }
    let mut rows = connection
        .query(
            "SELECT day, COUNT(DISTINCT session) FROM usage_turns
             WHERE day >= ?1 AND day <= ?2 GROUP BY day",
            params![first_text.as_str(), last_text.as_str()],
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let day = stored_day(&row.get::<String>(0)?)?;
        digests.get_mut(&day).expect("asked").sessions = count(row.get(1)?)?;
    }
    let mut rows = connection
        .query(
            "SELECT day, project, COUNT(*) FROM usage_approvals
             WHERE day >= ?1 AND day <= ?2 GROUP BY day, project",
            params![first_text.as_str(), last_text.as_str()],
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let day = stored_day(&row.get::<String>(0)?)?;
        let approvals = count(row.get(2)?)?;
        project_day(&mut projects, day, row.get::<String>(1)?).approvals = approvals;
        digests.get_mut(&day).expect("asked").approvals += approvals;
    }

    for incident in incidents(connection, first, last).await? {
        for digest in digests
            .values_mut()
            .filter(|digest| incident.touches(digest.day))
        {
            digest.incidents.push(incident.clone());
        }
    }
    for ((day, _), project) in projects {
        digests.get_mut(&day).expect("asked").projects.push(project);
    }
    Ok(digests.into_values().collect())
}

fn project_day(
    projects: &mut BTreeMap<(Day, String), ProjectDay>,
    day: Day,
    project: String,
) -> &mut ProjectDay {
    projects
        .entry((day, project.clone()))
        .or_insert_with(|| ProjectDay {
            project,
            ..ProjectDay::default()
        })
}

fn stored_day(text: &str) -> Result<Day, Error> {
    Day::parse(text).map_err(|error| invalid_row(format!("day: {error}")))
}

fn count(value: i64) -> Result<u64, Error> {
    u64::try_from(value).map_err(|_| invalid_row(format!("a negative count {value}")))
}
