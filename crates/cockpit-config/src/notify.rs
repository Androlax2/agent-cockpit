//! Owner-only: which events `torlek notify` tells with a desktop notification and which with
//! a sound (#62, #682), and the detectors that hold both (report 19). Read when the notifier
//! starts.
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::{ConfigError, Layer, Source, Sourced};
use serde::Deserialize;
use toml::Spanned;

/// The hold `torlek notify away on` sets; no detector may take its name.
pub const AWAY_HOLD: &str = "away";

/// The events the notifier tells, in its names, most urgent first.
pub const NOTIFY_EVENTS: [&str; 6] = ["failure", "hold", "question", "quota", "finished", "turn"];

/// Whether each of [`NOTIFY_EVENTS`] is on, in that order. Every event is on by default but
/// `turn`, which tells every completed turn (#1127).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyEvents {
    pub failure: Sourced<bool>,
    pub hold: Sourced<bool>,
    pub question: Sourced<bool>,
    pub quota: Sourced<bool>,
    pub finished: Sourced<bool>,
    pub turn: Sourced<bool>,
}

impl NotifyEvents {
    fn all_on() -> NotifyEvents {
        let on = || Sourced::new(true, Source::ProductDefault);
        NotifyEvents {
            failure: on(),
            hold: on(),
            question: on(),
            quota: on(),
            finished: on(),
            turn: Sourced::new(false, Source::ProductDefault),
        }
    }

    /// Each event's name with its setting, in [`NOTIFY_EVENTS`] order.
    pub fn by_name(&self) -> [(&'static str, &Sourced<bool>); 6] {
        [
            (NOTIFY_EVENTS[0], &self.failure),
            (NOTIFY_EVENTS[1], &self.hold),
            (NOTIFY_EVENTS[2], &self.question),
            (NOTIFY_EVENTS[3], &self.quota),
            (NOTIFY_EVENTS[4], &self.finished),
            (NOTIFY_EVENTS[5], &self.turn),
        ]
    }

    /// The events turned on, as `torlek-notify` takes them: comma-separated, `none` when
    /// every one is off.
    pub fn to_argument(&self) -> String {
        let on: Vec<&str> = self
            .by_name()
            .into_iter()
            .filter(|(_, setting)| setting.value)
            .map(|(name, _)| name)
            .collect();
        if on.is_empty() {
            return "none".to_owned();
        }
        on.join(",")
    }
}

#[derive(Debug, Clone)]
pub struct NotifySettings {
    /// What shows a desktop notification.
    pub desktop: NotifyEvents,
    /// What plays a sound.
    pub sound: NotifyEvents,
    /// Each hold's detector, by the hold's name: a program the notifier runs at every refresh,
    /// whose exit status 0 means the hold stands (a meeting, a game). None by default.
    pub holds: BTreeMap<String, Sourced<PathBuf>>,
    /// Whether `torlek daemon install` installs the unit that starts the notifier with the
    /// desktop session (#1126). The one opt-out; `torlek notify` by hand is unaffected.
    pub autostart: Sourced<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NotifyTable {
    desktop: Option<NotifyEventsTable>,
    sound: Option<NotifyEventsTable>,
    hold: Option<BTreeMap<String, Spanned<String>>>,
    autostart: Option<Spanned<bool>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotifyEventsTable {
    failure: Option<Spanned<bool>>,
    hold: Option<Spanned<bool>>,
    question: Option<Spanned<bool>>,
    quota: Option<Spanned<bool>>,
    finished: Option<Spanned<bool>>,
    turn: Option<Spanned<bool>>,
}

pub(super) fn settings(layers: &[Layer]) -> Result<NotifySettings, ConfigError> {
    let mut result = NotifySettings {
        desktop: NotifyEvents::all_on(),
        sound: NotifyEvents::all_on(),
        holds: BTreeMap::new(),
        autostart: Sourced::new(true, Source::ProductDefault),
    };
    for layer in layers {
        let Some(table) = &layer.settings.notify else {
            continue;
        };
        let table = table.get_ref();
        if let Some(desktop) = &table.desktop {
            apply(layer, desktop, &mut result.desktop);
        }
        if let Some(sound) = &table.sound {
            apply(layer, sound, &mut result.sound);
        }
        if let Some(autostart) = &table.autostart {
            result.autostart =
                Sourced::new(*autostart.get_ref(), layer.source_at(autostart.span()));
        }
        for (name, program) in table.hold.iter().flatten() {
            let source = layer.source_at(program.span());
            let detector = hold_detector(name, program.get_ref(), &source)?;
            result
                .holds
                .insert(name.clone(), Sourced::new(detector, source));
        }
    }
    Ok(result)
}

/// A detector's program, refused when the notifier could not run it as named: a relative
/// path would depend on the directory `torlek notify` starts in.
fn hold_detector(name: &str, program: &str, source: &Source) -> Result<PathBuf, ConfigError> {
    let invalid = |reason: String| ConfigError::Invalid {
        key: format!("notify.hold.{name}"),
        source: source.clone(),
        reason,
    };
    let is_plain_name = !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        });
    if !is_plain_name {
        return Err(invalid(
            "a hold's name is lowercase letters, digits and dashes".to_owned(),
        ));
    }
    if name == AWAY_HOLD {
        return Err(invalid(format!(
            "`{AWAY_HOLD}` is the hold `torlek notify away on` sets"
        )));
    }
    let program = PathBuf::from(program);
    if !program.is_absolute() {
        return Err(invalid(format!(
            "{}: name the detector by its absolute path",
            program.display()
        )));
    }
    Ok(program)
}

fn apply(layer: &Layer, table: &NotifyEventsTable, events: &mut NotifyEvents) {
    let declared = [
        (&table.failure, &mut events.failure),
        (&table.hold, &mut events.hold),
        (&table.question, &mut events.question),
        (&table.quota, &mut events.quota),
        (&table.finished, &mut events.finished),
        (&table.turn, &mut events.turn),
    ];
    for (value, setting) in declared {
        if let Some(value) = value {
            *setting = Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
    }
}
