//! Which conversation a session's reports may come from (#219). The launch token and the
//! process tree say which session sent a report; the conversation the provider bridge names
//! must also be the session's, so a hook run with another launch's environment (Claude
//! Code's shared background daemon, #184) cannot move this session's state. A legitimate
//! change (`/clear`, an in-session `/resume`) is announced by the current conversation
//! first, and the session then follows the next conversation reported, once, in the same
//! launch.

use cockpit_authorization::SessionId;
use cockpit_provider_api::ConversationId;
use cockpit_turso::WriteTransaction;

use crate::reports::{invalid, refused};
use crate::{Error, store};

/// Accepts a report from `reported` for `session`'s `launch`, following an announced change,
/// or refuses it. `is_ending` announces that `reported` ends for another conversation.
/// Returns whether the session followed a new conversation, which starts afresh (#213, #225).
pub(crate) async fn check(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    launch: Option<i64>,
    reported: &ConversationId,
    is_ending: bool,
) -> Result<bool, Error> {
    let id = crate::SessionId::parse(session.as_str())?;
    let Some(current) = store::session(write, &id).await?.conversation else {
        // A provider that never named its conversation is named by its first report.
        store::change_conversation(write, &id, reported).await?;
        record_end(write, session, launch, reported, is_ending).await?;
        return Ok(false);
    };
    if *reported == current {
        record_end(write, session, launch, reported, is_ending).await?;
        return Ok(false);
    }
    if !is_ending && announced_end(write, session, launch, &current).await? {
        clear_end(write, session).await?;
        store::change_conversation(write, &id, reported).await?;
        return Ok(true);
    }
    Err(refused(invalid(
        "conversation",
        format!(
            "{reported} is not session {}'s conversation ({current}), which announced no \
             change; a report of another conversation never moves this session",
            session.as_str()
        ),
    )))
}

/// A report of the current conversation closes any change it announced, and opens one when
/// it announces its end.
async fn record_end(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    launch: Option<i64>,
    conversation: &ConversationId,
    is_ending: bool,
) -> Result<(), Error> {
    clear_end(write, session).await?;
    if is_ending {
        write
            .execute(
                "INSERT INTO session_conversation_ends (session_id, conversation, launch_id)
                 VALUES (?1, ?2, ?3)",
                (session.as_str(), conversation.as_str(), launch),
            )
            .await?;
    }
    Ok(())
}

async fn clear_end(write: &WriteTransaction<'_>, session: &SessionId) -> Result<(), Error> {
    write
        .execute(
            "DELETE FROM session_conversation_ends WHERE session_id = ?1",
            [session.as_str()],
        )
        .await?;
    Ok(())
}

/// Whether `conversation` announced its end in `launch`: an end from an earlier launch says
/// nothing about this launch's agent.
async fn announced_end(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    launch: Option<i64>,
    conversation: &ConversationId,
) -> Result<bool, Error> {
    let mut rows = write
        .query(
            "SELECT 1 FROM session_conversation_ends
             WHERE session_id = ?1 AND conversation = ?2 AND launch_id IS ?3",
            (session.as_str(), conversation.as_str(), launch),
        )
        .await?;
    Ok(rows.next().await?.is_some())
}
