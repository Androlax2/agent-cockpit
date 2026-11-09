//! Send and cancel share the session's operation lock and its recorded launch context.
use super::*;
use crate::Addressed;
use crate::model::Launch;
use cockpit_provider_api::{
    Action, Activity, ConversationId, Delivered, DeliveryTarget, Key, TerminalInput,
};
use cockpit_tmux::Keystroke;

impl Sessions {
    /// The launch, provider and conversation an action needing `capability` reaches in
    /// `session`, or why its state rules the action out before the provider is asked: its
    /// agent does not run, its provider is unknown or lacks `capability`, or it has not named
    /// its conversation. A coordinator queuing a message learns the same (#656).
    pub fn delivery_target(
        &self,
        session: &Session,
        capability: Capability,
    ) -> Result<(Launch, Arc<dyn Provider>, ConversationId), Error> {
        let Some(launch) = session
            .launch
            .clone()
            .filter(|_| session.state == SessionState::Running)
        else {
            return Err(Error::WrongState {
                session: session.id.clone(),
                state: session.state,
                operation: "deliver an action to its agent",
            });
        };
        let provider = Arc::clone(self.provider(Some(session.provider.as_str()))?);
        provider.capabilities().require(capability)?;
        let conversation =
            session
                .conversation
                .clone()
                .ok_or_else(|| Error::UnknownConversation {
                    session: session.id.clone(),
                })?;
        Ok((launch, provider, conversation))
    }

    /// Whether `session`'s agent collects messages from other sessions itself, through its
    /// bridge's hooks ([`Capability::Inbox`], #703), instead of having them typed at its
    /// prompt. Refused when its provider is not one this daemon runs.
    pub fn collects_messages(&self, session: &Session) -> Result<bool, Error> {
        let provider = self.provider(Some(session.provider.as_str()))?;
        Ok(provider.capabilities().supports(Capability::Inbox))
    }

    /// Delivers to the named conversation, using terminal input only when the provider
    /// explicitly has no delivery transport. Unconfirmed outcomes must not be retried as
    /// fresh messages. A busy session refuses immediately.
    pub async fn submit(
        &self,
        id: &SessionId,
        message: &str,
        addressed: Addressed,
    ) -> Result<(), Error> {
        if message.trim().is_empty() || message.contains('\0') {
            return Err(Error::InvalidMessage {
                reason: "provide nonempty text without NUL bytes",
            });
        }
        self.act(id, Some(message), addressed).await
    }

    /// Types `text`, then Enter, into the agent's terminal, as the owner typing into a
    /// viewer attached to it would (#855): no provider transport and no activity check, but
    /// only into a running session and only printable text.
    pub async fn type_line(&self, id: &SessionId, text: &str) -> Result<(), Error> {
        if text.chars().any(char::is_control) {
            return Err(Error::InvalidMessage {
                reason: "type printable text: no control characters or line breaks",
            });
        }
        let session = self.get(id).await?;
        if session.state != SessionState::Running {
            return Err(Error::WrongState {
                session: id.clone(),
                state: session.state,
                operation: "type into its terminal",
            });
        }
        let tmux = self.tmux.clone();
        let name = terminal_name(id)?;
        let text = text.to_owned();
        self.blocking("type into the agent's terminal", move || {
            let tmux = tmux.map_err(|reason| Error::TmuxUnavailable { reason })?;
            let pane = agent_pane(&tmux, &name)?;
            let mut keys = Vec::new();
            if !text.is_empty() {
                keys.push(Keystroke::Text(&text));
            }
            keys.push(Keystroke::Enter);
            tmux.send_keys(&pane, &keys).map_err(Error::Tmux)
        })
        .await
    }

    /// Pastes `text` into the agent's terminal as one bracketed paste (#848), as the owner's
    /// terminal pastes: only into a running session and only printable text.
    pub async fn paste_text(&self, id: &SessionId, text: &str) -> Result<(), Error> {
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(Error::InvalidMessage {
                reason: "paste printable text: no control characters or line breaks",
            });
        }
        let session = self.get(id).await?;
        if session.state != SessionState::Running {
            return Err(Error::WrongState {
                session: id.clone(),
                state: session.state,
                operation: "paste into its terminal",
            });
        }
        let tmux = self.tmux.clone();
        let name = terminal_name(id)?;
        let text = text.to_owned();
        self.blocking("paste into the agent's terminal", move || {
            let tmux = tmux.map_err(|reason| Error::TmuxUnavailable { reason })?;
            let pane = agent_pane(&tmux, &name)?;
            tmux.paste(&pane, &text).map_err(Error::Tmux)
        })
        .await
    }

    /// Interrupts the turn without ending the agent or changing the session's lifecycle.
    pub async fn cancel(&self, id: &SessionId) -> Result<(), Error> {
        self.act(id, None, Addressed::Owner).await
    }

    async fn act(
        &self,
        id: &SessionId,
        message: Option<&str>,
        addressed: Addressed,
    ) -> Result<(), Error> {
        let Ok(guard) = Arc::clone(&self.session_lock(id).operation).try_lock_owned() else {
            return Err(Error::SessionBusy {
                session: id.clone(),
            });
        };
        let read = self.database.read().await?;
        let session = store::session(&read, id).await?;
        let capability = if message.is_some() {
            Capability::Submit
        } else {
            Capability::Cancel
        };
        let (launch, provider, conversation) = self.delivery_target(&session, capability)?;
        let report = crate::reports::latest_report(&read, &authorization_session(id)?).await?;
        let provider_launch =
            crate::delivery_context::read_launch(&read, launch.execution.0).await?;
        let sockets = crate::delivery_context::Sockets {
            context: crate::delivery_context::read(&read, launch.execution.0).await?,
            connect: self.connect_socket,
        };
        drop(read);
        let activity = report
            .filter(|report| report.launch == Some(launch.number))
            .and_then(|report| {
                let answerable = addressed != Addressed::Work
                    && report.question_source == Some(QuestionSource::Agent);
                if report.needs_input.is_some() && !answerable {
                    return Some(Activity::NeedsYou);
                }
                match report.state? {
                    AgentState::Working => Some(Activity::Working),
                    AgentState::Waiting => Some(Activity::Waiting),
                }
            });
        if message.is_some() && activity == Some(Activity::NeedsYou) {
            return Err(Error::Provider(ProviderError {
                reason: "the agent has an open question or dialog; answer it first".into(),
            }));
        }
        let message = message.map(str::to_owned);
        let session_dir = self.session_dir(id);
        let tmux = self.tmux.clone();
        let name = terminal_name(id)?;
        let session_id = id.clone();
        let delivered = self.blocking("deliver an action to the agent", move || {
            // Held until the blocking call returns, even when its caller times out.
            let _guard = guard;
            let action = message.as_deref().map_or(Action::Cancel, Action::Submit);
            let target = DeliveryTarget {
                launch: provider_launch.as_ref(),
                session_dir: &session_dir,
                sockets: &sockets,
            };
            match provider.deliver(&action, &target, &conversation) {
                Ok(Delivered::Unsupported) => {}
                Ok(_) => return Ok(Typing::Done),
                Err(error) => {
                    return Err(Error::DeliveryUnconfirmed {
                        session: session_id,
                        reason: error.to_string(),
                    });
                }
            }
            let input = provider.input(&action, activity)?;
            let tmux = tmux.map_err(|reason| Error::TmuxUnavailable { reason })?;
            let pane = agent_pane(&tmux, &name)?;
            // Typed into a released terminal, the text would sit at a prompt nobody reads,
            // or reach the harness as one stale burst once it resumes (#1271).
            if shows_suspended(&tmux, &pane, provider.as_ref())? {
                return Ok(Typing::AgentSuspended);
            }
            let keys: Vec<Keystroke> = input
                .iter()
                .map(|step| match step {
                    TerminalInput::Text(text) => Keystroke::Text(text),
                    TerminalInput::Key(Key::Enter) => Keystroke::Enter,
                    TerminalInput::Key(Key::Escape) => Keystroke::Escape,
                    TerminalInput::Key(Key::Control(letter)) => Keystroke::Control(*letter),
                })
                .collect();
            refuse_before_typing(&keys).map_err(Error::Tmux)?;
            type_in_separate_writes(&tmux, &pane, &keys)
                .map(|()| Typing::Done)
                .map_err(|failure| match failure {
                    (0, error @ cockpit_tmux::Error::CommandFailed { .. })
                    | (0, error @ cockpit_tmux::Error::InvalidArgument { .. }) => {
                        Error::Tmux(error)
                    }
                    (_, other) => Error::TypingUnconfirmed {
                        session: session_id,
                        reason: format!("tmux: {other}"),
                    },
                })
        });
        match delivered.await {
            Ok(Typing::Done) => Ok(()),
            Ok(Typing::AgentSuspended) => Err(Error::AgentSuspended {
                session: id.clone(),
                is_resumed: self.continue_suspended(id, launch.execution).await,
            }),
            Err(error @ (Error::StepTimedOut { .. } | Error::Blocking(_))) => {
                Err(Error::DeliveryUnconfirmed {
                    session: id.clone(),
                    reason: error.to_string(),
                })
            }
            Err(other) => Err(other),
        }
    }

    /// Sends `SIGCONT` to a suspended agent found by a delivery, which is refused all the
    /// same: whether it was sent. A failure is logged, the delivery's refusal says it.
    async fn continue_suspended(&self, id: &SessionId, execution: ExecutionId) -> bool {
        match self.send_continue(execution).await {
            Ok(is_sent) => is_sent,
            Err(error) => {
                tracing::warn!(session = %id, %error, "a suspended agent could not be resumed");
                false
            }
        }
    }
}

/// What a delivery that reached the agent's terminal found there.
enum Typing {
    Done,
    /// The agent released its terminal (#1271): nothing was typed.
    AgentSuspended,
}

/// How long typed text settles before the keys after it are written. Claude Code reads a
/// burst of text as a paste and drops an Enter that arrives in it: on 2.1.318, a message of
/// about 150 characters or more stayed unsent in its input box when text and Enter came in
/// one write, and was submitted by an Enter written separately 50 ms later or more (#730).
/// Twice that, and no longer: a dialog the agent opens meanwhile would take the keys.
const TEXT_SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

/// The check tmux makes of a control chord, made before anything is typed: with the text and
/// its keys in separate writes, tmux would refuse the chord only after the text was typed.
fn refuse_before_typing(keys: &[Keystroke<'_>]) -> Result<(), cockpit_tmux::Error> {
    match keys.iter().find_map(|key| match key {
        Keystroke::Control(letter) if !letter.is_ascii_lowercase() => Some(*letter),
        _ => None,
    }) {
        Some(letter) => Err(cockpit_tmux::Error::InvalidArgument {
            what: "control key (a lower-case ASCII letter)",
            value: letter.to_string(),
        }),
        None => Ok(()),
    }
}

/// Writes `keys` to `pane` with each run of text and each run of keys as its own tmux
/// command, [`TEXT_SETTLE`] apart, so no key reaches the agent inside the burst of the text
/// before it, and a chord such as Ctrl-X Enter still arrives whole. A failure names the
/// index of the write that failed: the writes before it were typed.
fn type_in_separate_writes(
    tmux: &cockpit_tmux::Server,
    pane: &cockpit_tmux::PaneId,
    keys: &[Keystroke<'_>],
) -> Result<(), (usize, cockpit_tmux::Error)> {
    let is_text = |key: &Keystroke<'_>| matches!(key, Keystroke::Text(_));
    for (index, write) in keys
        .chunk_by(|before, after| is_text(before) == is_text(after))
        .enumerate()
    {
        if index > 0 {
            std::thread::sleep(TEXT_SETTLE);
        }
        tmux.send_keys(pane, write)
            .map_err(|error| (index, error))?;
    }
    Ok(())
}

/// `text` as one line a card can show: controls become spaces, and invisible or
/// bidirectional formatting characters, which could make it read as something else, go.
pub fn visible_line(text: &str) -> String {
    text.chars()
        .filter(|character| !crate::reports::is_invisible_format(*character))
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}
