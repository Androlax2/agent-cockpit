//! Owner-only: which events `torlek notify` tells with a desktop notification and which with
//! a sound (#62, #682). Read when the notifier starts.
use crate::{Layer, Source, Sourced};
use serde::Deserialize;
use toml::Spanned;

/// The events the notifier tells, in its names, most urgent first.
pub const NOTIFY_EVENTS: [&str; 5] = ["failure", "hold", "question", "quota", "finished"];

/// Whether each of [`NOTIFY_EVENTS`] is on, in that order. Every event is on by default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyEvents {
    pub failure: Sourced<bool>,
    pub hold: Sourced<bool>,
    pub question: Sourced<bool>,
    pub quota: Sourced<bool>,
    pub finished: Sourced<bool>,
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
        }
    }

    /// Each event's name with its setting, in [`NOTIFY_EVENTS`] order.
    pub fn by_name(&self) -> [(&'static str, &Sourced<bool>); 5] {
        [
            (NOTIFY_EVENTS[0], &self.failure),
            (NOTIFY_EVENTS[1], &self.hold),
            (NOTIFY_EVENTS[2], &self.question),
            (NOTIFY_EVENTS[3], &self.quota),
            (NOTIFY_EVENTS[4], &self.finished),
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NotifyTable {
    desktop: Option<NotifyEventsTable>,
    sound: Option<NotifyEventsTable>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotifyEventsTable {
    failure: Option<Spanned<bool>>,
    hold: Option<Spanned<bool>>,
    question: Option<Spanned<bool>>,
    quota: Option<Spanned<bool>>,
    finished: Option<Spanned<bool>>,
}

pub(super) fn settings(layers: &[Layer]) -> NotifySettings {
    let mut result = NotifySettings {
        desktop: NotifyEvents::all_on(),
        sound: NotifyEvents::all_on(),
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
    }
    result
}

fn apply(layer: &Layer, table: &NotifyEventsTable, events: &mut NotifyEvents) {
    let declared = [
        (&table.failure, &mut events.failure),
        (&table.hold, &mut events.hold),
        (&table.question, &mut events.question),
        (&table.quota, &mut events.quota),
        (&table.finished, &mut events.finished),
    ];
    for (value, setting) in declared {
        if let Some(value) = value {
            *setting = Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
    }
}
