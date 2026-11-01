//! A harness's native commands, distinct from messages submitted as literal text.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCommand {
    pub name: String,
    pub description: Option<String>,
    /// The harness's origin label, such as a project command, MCP prompt or skill.
    pub source: Option<String>,
}

/// The harness's reply to a command invocation, not proof of task completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub message: String,
}
