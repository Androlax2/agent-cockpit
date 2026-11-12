//! Forwards opened live (#390): a `containers` port the owner allowed after the launch. Each
//! one the decisions hold gets a listener the sandbox's binder bound, served like a declared
//! forward; one they no longer hold has its listener closed, beside its relays, which
//! `close_revoked_relays` closes.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cockpit_sandbox::{Forward, ServiceDecisions};

use super::{Listener, Proxy, Upstream};
use crate::handoff::Binder;

/// A live forward's listener, kept to close it.
pub(super) struct LiveListener {
    listener: TcpListener,
    is_closed: Arc<AtomicBool>,
}

impl LiveListener {
    fn close(&self) {
        self.is_closed.store(true, Ordering::SeqCst);
        // Wakes the serving thread's accept, which then ends, dropping the last copy.
        let _ = rustix::net::shutdown(&self.listener, rustix::net::Shutdown::Read);
    }
}

impl<U: Upstream> Proxy<U> {
    /// Opens the live forwards of every later decision through `binder`.
    pub fn open_live_through(&self, binder: Binder) -> Result<(), String> {
        self.binder
            .set(binder)
            .map_err(|_| "the launcher took the sandbox's binder twice".to_owned())
    }

    /// Opens again the live forwards the current decisions allow and a failed bind left
    /// closed: a port taken a moment opens once it is free, without a new decision.
    pub(super) fn retry_live_forwards(self: &Arc<Self>) {
        if !self.live_pending.load(Ordering::SeqCst) {
            return;
        }
        let decided = Arc::clone(
            &self
                .decided
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        self.open_live_forwards(&decided.decisions.services);
    }

    /// Opens a listener for each live forward `services` allows and has none, and closes
    /// those it no longer allows.
    pub(super) fn open_live_forwards(self: &Arc<Self>, services: &ServiceDecisions) {
        let Some(binder) = self.binder.get() else {
            return;
        };
        let wanted: BTreeMap<String, &Forward> = services
            .live_forwards()
            .filter(|forward| forward.live_problem().is_none())
            .map(|forward| (forward.record(), forward))
            .collect();
        let mut live = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        live.retain(|record, open| {
            let is_wanted = wanted.contains_key(record);
            if !is_wanted {
                open.close();
            }
            is_wanted
        });
        let mut is_pending = false;
        for (record, forward) in wanted {
            if live.contains_key(&record) {
                continue;
            }
            let Some(port) = forward.listen_port() else {
                continue;
            };
            match self.open_live(binder, forward.clone(), port) {
                Ok(open) => {
                    live.insert(record, open);
                }
                Err(reason) => {
                    is_pending = true;
                    self.note_live_failure(&forward.service, reason);
                }
            }
        }
        self.live_pending.store(is_pending, Ordering::SeqCst);
    }

    /// Records why a live forward did not open, once while the reason lasts: it is retried
    /// at every decisions poll.
    fn note_live_failure(&self, service: &str, reason: String) {
        let is_new = self
            .report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forwards
            .get(service)
            .and_then(|counters| counters.last_reason.as_ref())
            != Some(&reason);
        if is_new {
            self.count_forward(service, |counters| counters.last_reason = Some(reason));
        }
    }

    fn open_live(
        self: &Arc<Self>,
        binder: &Binder,
        forward: Forward,
        port: u16,
    ) -> Result<LiveListener, String> {
        let listener = binder.listen(port)?;
        let kept = listener
            .try_clone()
            .map_err(|error| format!("cannot keep the listener on port {port}: {error}"))?;
        let is_closed = Arc::new(AtomicBool::new(false));
        let serving = Arc::clone(self);
        let closed = Arc::clone(&is_closed);
        std::thread::Builder::new()
            .name("live-forward".to_owned())
            .spawn(move || {
                let listener = Listener::Port(listener);
                serving.serve_forward_until(forward, &listener, &|| closed.load(Ordering::SeqCst));
            })
            .map_err(|error| format!("cannot serve port {port}: {error}"))?;
        Ok(LiveListener {
            listener: kept,
            is_closed,
        })
    }
}
