//! The events that need the owner, each turned on or off apart for the desktop and for sound
//! (#62, #682).

use std::collections::BTreeSet;
use std::fmt;

/// What a session needs the owner for. Declared most urgent first: several events that land
/// together are told by the most urgent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Event {
    /// It failed, or its agent exited by itself. A session the owner ended raises nothing.
    Failure,
    /// It is held on the owner's permission: a command to approve, a refused network host,
    /// or a provider's permission dialog.
    Hold,
    /// It asks a question, waits for a guide's step or for a line of input.
    Question,
    /// The plan's usage limit stopped its turn.
    Quota,
    /// It finished its work: the task list says so (#1127).
    Finished,
    /// Its agent completed a turn, whether or not its work is done. Off by default for both
    /// outlets: with several sessions it is the notification that spams (#1127).
    Turn,
}

impl Event {
    pub const ALL: [Event; 6] = [
        Event::Failure,
        Event::Hold,
        Event::Question,
        Event::Quota,
        Event::Finished,
        Event::Turn,
    ];

    /// The events on for an outlet the owner did not configure: all but [`Event::Turn`].
    pub fn defaults() -> BTreeSet<Event> {
        Event::ALL
            .into_iter()
            .filter(|event| *event != Event::Turn)
            .collect()
    }

    /// The event's name in settings and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Event::Failure => "failure",
            Event::Hold => "hold",
            Event::Question => "question",
            Event::Quota => "quota",
            Event::Finished => "finished",
            Event::Turn => "turn",
        }
    }

    pub fn from_name(name: &str) -> Option<Event> {
        Event::ALL.into_iter().find(|event| event.name() == name)
    }

    /// Parses `none` or a comma-separated list of event names.
    pub fn parse_list(list: &str) -> Result<BTreeSet<Event>, String> {
        if list == "none" {
            return Ok(BTreeSet::new());
        }
        list.split(',')
            .map(|name| {
                Event::from_name(name).ok_or_else(|| {
                    format!(
                        "unknown event {name:?}: use none or a comma-separated list of {}",
                        names(&Event::ALL)
                    )
                })
            })
            .collect()
    }
}

impl fmt::Display for Event {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// `events` as a comma-separated list, `none` when empty: what [`Event::parse_list`] reads.
pub fn names<'a>(events: impl IntoIterator<Item = &'a Event>) -> String {
    let names: Vec<&str> = events.into_iter().map(|event| event.name()).collect();
    if names.is_empty() {
        return "none".to_owned();
    }
    names.join(",")
}
