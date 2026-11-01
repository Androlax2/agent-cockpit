//! Send and cancel share the session's operation lock and its recorded launch context.
use super::*;
use crate::Addressed;
use cockpit_provider_api::{Action, Activity, Delivered, DeliveryTarget, Key, TerminalInput};
use cockpit_tmux::Keystroke;

impl Sessions {
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
        let Some(launch) = session
            .launch
            .as_ref()
            .filter(|_| session.state == SessionState::Running)
        else {
            return Err(Error::WrongState {
                session: id.clone(),
                state: session.state,
                operation: "deliver an action to its agent",
            });
        };
        let provider = Arc::clone(self.provider(Some(session.provider.as_str()))?);
        let capability = if message.is_some() {
            Capability::Submit
        } else {
            Capability::Cancel
        };
        provider.capabilities().require(capability)?;
        let conversation = session
            .conversation
            .ok_or_else(|| Error::UnknownConversation {
                session: id.clone(),
            })?;
        let report = crate::reports::latest_report(&read, &authorization_session(id)?).await?;
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
        self.blocking("deliver an action to the agent", move || {
            // Held until the blocking call returns, even when its caller times out.
            let _guard = guard;
            let action = message.as_deref().map_or(Action::Cancel, Action::Submit);
            let target = DeliveryTarget {
                session_dir: &session_dir,
                sockets: &sockets,
            };
            match provider.deliver(&action, &target, &conversation) {
                Ok(Delivered::Unsupported) => {}
                Ok(_) => return Ok(()),
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
            let keys: Vec<Keystroke> = input
                .iter()
                .map(|step| match step {
                    TerminalInput::Text(text) => Keystroke::Text(text),
                    TerminalInput::Key(Key::Enter) => Keystroke::Enter,
                    TerminalInput::Key(Key::Escape) => Keystroke::Escape,
                    TerminalInput::Key(Key::Control(letter)) => Keystroke::Control(*letter),
                })
                .collect();
            tmux.send_keys(&pane, &keys).map_err(|error| match error {
                cockpit_tmux::Error::CommandFailed { .. }
                | cockpit_tmux::Error::InvalidArgument { .. } => Error::Tmux(error),
                other => Error::TypingUnconfirmed {
                    session: session_id,
                    reason: format!("tmux: {other}"),
                },
            })
        })
        .await
        .map_err(|error| match error {
            Error::StepTimedOut { .. } | Error::Blocking(_) => Error::DeliveryUnconfirmed {
                session: id.clone(),
                reason: error.to_string(),
            },
            other => other,
        })
    }
}
