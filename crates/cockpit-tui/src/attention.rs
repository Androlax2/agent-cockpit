//! When the owner should look at the dashboard (#682): the cues a session raises, and the
//! watch that turns a newly raised cue into one bell.

use std::collections::BTreeSet;

/// What a session needs the owner for. A session raises each cue while its cause stands; the
/// bell rings when a cue is raised, never while it stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Cue {
    /// It finished its work: the task list says so, or its turn completed.
    Finished,
    /// It asks a question, or waits for an approval, a guide or a line of input.
    NeedsYou,
    /// It failed, or its agent exited by itself. A session the owner ended raises nothing.
    Failed,
}

/// The daemon's state arrives from three places. A cue seen before all three have answered
/// once is the state the TUI started on, not news.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Source {
    Sessions,
    Reports,
    Queue,
}

const SOURCES: [Source; 3] = [Source::Sessions, Source::Reports, Source::Queue];

#[derive(Debug, Default, Clone)]
pub(crate) struct Watch {
    /// The cues raised as of the last look, by session id.
    raised: BTreeSet<(String, Cue)>,
    loaded: BTreeSet<Source>,
    is_ringing: bool,
}

impl Watch {
    /// Looks at the cues `raised` now. `source` is the state that just changed, if it is one
    /// of the three the TUI waits for at start. A cue not raised at the last look rings,
    /// once the start state is loaded; all that land together ring once.
    pub(crate) fn look(&mut self, source: Option<Source>, raised: BTreeSet<(String, Cue)>) {
        let was_loaded = self.is_loaded();
        self.loaded.extend(source);
        if was_loaded && raised.difference(&self.raised).next().is_some() {
            self.is_ringing = true;
        }
        self.raised = raised;
    }

    /// Whether the bell should ring for what was looked at since the last call.
    pub(crate) fn take_ring(&mut self) -> bool {
        std::mem::take(&mut self.is_ringing)
    }

    fn is_loaded(&self) -> bool {
        SOURCES.iter().all(|source| self.loaded.contains(source))
    }
}
