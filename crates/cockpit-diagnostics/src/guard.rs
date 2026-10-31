//! Refuses records and spans whose field names mark hosted work. Report 12: prompts, agent
//! output, commands the agent runs, credentials and environment values are never logged.
//!
//! The check reads field names from the callsite, never values, so it cannot catch content
//! formatted into a message; it stops the obvious `prompt = …` mistake at its source.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::subscriber::Interest;
use tracing::{Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::queue::{QueueWriter, own_record};

const HOSTED_CONTENT_FIELDS: [&str; 14] = [
    "prompt",
    "agent_output",
    "shell_command",
    "argv",
    "stdin",
    "stdout",
    "stderr",
    "env",
    "environment",
    "env_value",
    "credential",
    "password",
    "secret",
    "token",
];

pub(crate) struct HostedContentGuard {
    writer: QueueWriter,
    refused: Arc<AtomicU64>,
}

impl HostedContentGuard {
    pub(crate) fn new(writer: QueueWriter, refused: Arc<AtomicU64>) -> Self {
        Self { writer, refused }
    }
}

fn carries_hosted_content(metadata: &Metadata<'_>) -> bool {
    metadata
        .fields()
        .iter()
        .any(|field| HOSTED_CONTENT_FIELDS.contains(&field.name()))
}

impl<S: Subscriber> Layer<S> for HostedContentGuard {
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        // `sometimes` makes every occurrence reach `enabled`, so each one is counted.
        if carries_hosted_content(metadata) {
            Interest::sometimes()
        } else {
            Interest::always()
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>, _: Context<'_, S>) -> bool {
        if !carries_hosted_content(metadata) {
            return true;
        }
        self.refused.fetch_add(1, Ordering::Relaxed);
        let callsite = format!(
            "{}:{}",
            metadata.file().unwrap_or("unknown"),
            metadata.line().unwrap_or(0)
        );
        self.writer.push(own_record(
            "WARN",
            "record with hosted content refused",
            [
                ("callsite", callsite.into()),
                ("name", metadata.name().into()),
            ],
        ));
        false
    }
}
