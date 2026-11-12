//! What lets parallel sessions of one project run one compose file on the project's shared
//! engine (#1371): each sandboxed session gets a compose project name and a block of host
//! ports of its own, as variables its compose file reads. The ports are the engine's
//! network namespace's, which no other project shares; a session reaches one through a
//! `containers` forward the owner allows.

use std::ops::RangeInclusive;

/// The compose project name a session's containers, default networks and default volumes
/// take.
pub const COMPOSE_PROJECT_VARIABLE: &str = "COMPOSE_PROJECT_NAME";
/// The first port of the session's block.
pub const PORT_BASE_VARIABLE: &str = "TORLEK_PORT_BASE";
/// How many ports the block holds: [`PORT_BLOCK_SIZE`].
pub const PORT_COUNT_VARIABLE: &str = "TORLEK_PORT_COUNT";
/// Each port of a block, `TORLEK_PORT_0` up: a compose file cannot add to a variable, so
/// each port it publishes reads one of these.
pub const PORT_VARIABLE_PREFIX: &str = "TORLEK_PORT_";

/// The ports one session's block holds.
pub const PORT_BLOCK_SIZE: u16 = 10;
/// The first block's first port.
const FIRST_BLOCK_PORT: u16 = 20_000;
/// How many blocks one project's engine hands out: the last ends at 32759, below the
/// kernel's default ephemeral ports (32768 up), which the engine's own outgoing connections
/// use.
pub const PORT_BLOCKS: u16 = 1_276;

/// A session's block of [`PORT_BLOCK_SIZE`] host ports in its project engine's namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PortBlock(u16);

impl PortBlock {
    /// The block numbered `index`, from 0; None past the last of [`PORT_BLOCKS`].
    pub fn new(index: u16) -> Option<PortBlock> {
        (index < PORT_BLOCKS).then_some(PortBlock(index))
    }

    /// The block holding `port`, if a block does.
    pub fn containing(port: u16) -> Option<PortBlock> {
        let offset = port.checked_sub(FIRST_BLOCK_PORT)?;
        PortBlock::new(offset / PORT_BLOCK_SIZE)
    }

    pub fn index(self) -> u16 {
        self.0
    }

    pub fn first_port(self) -> u16 {
        FIRST_BLOCK_PORT + self.0 * PORT_BLOCK_SIZE
    }

    pub fn ports(self) -> RangeInclusive<u16> {
        self.first_port()..=self.first_port() + (PORT_BLOCK_SIZE - 1)
    }

    /// The variables a session holding this block launches with: its base, its count and
    /// each of its ports.
    pub fn variables(self) -> Vec<(String, String)> {
        let mut variables = vec![
            (PORT_BASE_VARIABLE.to_owned(), self.first_port().to_string()),
            (PORT_COUNT_VARIABLE.to_owned(), PORT_BLOCK_SIZE.to_string()),
        ];
        variables.extend(
            self.ports()
                .zip(0..)
                .map(|(port, offset)| (port_variable(offset), port.to_string())),
        );
        variables
    }
}

impl std::fmt::Display for PortBlock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ports = self.ports();
        write!(formatter, "{}-{}", ports.start(), ports.end())
    }
}

/// The variable naming a block's port at `offset` from its base: `TORLEK_PORT_0` is the
/// base itself.
pub fn port_variable(offset: u16) -> String {
    format!("{PORT_VARIABLE_PREFIX}{offset}")
}

/// Whether `name` is one of the variables a session's block sets, so a launch replaces any
/// the owner's environment carries.
pub fn is_container_port_variable(name: &str) -> bool {
    name == COMPOSE_PROJECT_VARIABLE || name.starts_with(PORT_VARIABLE_PREFIX)
}

/// The compose project name of `session` in `project`: `<project>-<session>`, the project's
/// name lower-cased as compose accepts one (lower-case letters, digits, `_` and `-`, starting with a letter
/// or a digit). The session id alone keeps it unique; the project's name tells the owner
/// whose containers `docker ps` lists.
pub fn compose_project_name(project: &str, session: &str) -> String {
    let project: String = project
        .chars()
        .map(|character| match character.to_ascii_lowercase() {
            plain @ ('a'..='z' | '0'..='9' | '_' | '-') => plain,
            _ => '-',
        })
        .collect();
    let project = project.trim_start_matches(['_', '-']);
    if project.is_empty() {
        return session.to_owned();
    }
    format!("{project}-{session}")
}
