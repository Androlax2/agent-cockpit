//! Whether a project's compose files run in parallel sessions on the project's one engine
//! (#1371). Each sandboxed session launches with its own `COMPOSE_PROJECT_NAME` and port
//! block, which help only a file that leaves names to compose and reads its host ports from
//! variables. The scan reads lines, not YAML: it names the line of each fixed container
//! name, fixed volume or network name and literal host port, and the variable form that
//! fixes it.

use cockpit_config::{
    COMPOSE_PROJECT_VARIABLE, PORT_BLOCK_SIZE, PORT_VARIABLE_PREFIX, port_variable,
};

use crate::Requirement;

/// The files `docker compose` reads from a project's folder by default, overrides included.
pub const COMPOSE_FILES: [&str; 8] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
    "compose.override.yaml",
    "compose.override.yml",
    "docker-compose.override.yaml",
    "docker-compose.override.yml",
];

/// One compose file of the project's folder, as the daemon read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeFile {
    /// One of [`COMPOSE_FILES`].
    pub name: String,
    /// Its text, or why it was not read.
    pub text: Result<String, String>,
}

/// What keeps a compose file from running in two sessions at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeProblem {
    /// `container_name:` with no variable in it.
    ContainerName,
    /// A top-level volume or network with a `name:` that has no variable in it.
    FixedName { kind: &'static str },
    /// A published host port written as a number, or a range of them.
    LiteralPort { port: String },
}

/// A problem and its line, from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeFinding {
    pub line: usize,
    pub problem: ComposeProblem,
}

/// Every fixed name and literal host port `text` holds, in line order.
pub fn scan(text: &str) -> Vec<ComposeFinding> {
    let mut findings = Vec::new();
    let mut section = Section::Other;
    let mut ports_indent: Option<usize> = None;
    for (number, raw) in text.lines().enumerate() {
        let line = without_comment(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "---" {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let mut found = |problem| {
            findings.push(ComposeFinding {
                line: number + 1,
                problem,
            });
        };
        if let Some(key_indent) = ports_indent {
            let is_item = trimmed.starts_with('-');
            if indent > key_indent || (indent == key_indent && is_item) {
                if let Some(item) = trimmed.strip_prefix('-') {
                    let item = item.trim();
                    if let Some(mapping) = item.strip_prefix('{') {
                        found_published_ports(mapping.trim_end_matches('}').split(','), &mut found);
                    } else if key_value(item).is_none() {
                        found_short_ports([item], &mut found);
                    }
                }
                found_published_ports([trimmed.trim_start_matches(['-', ' '])], &mut found);
                continue;
            }
            ports_indent = None;
        }
        if indent == 0 {
            findings.extend(section.flush());
            section = match key_value(trimmed) {
                Some(("volumes", _)) => Section::named("volume"),
                Some(("networks", _)) => Section::named("network"),
                _ => Section::Other,
            };
            continue;
        }
        if let Section::Named(named) = &mut section {
            findings.extend(named.read(indent, key_value(trimmed), number + 1));
            continue;
        }
        let Some((key, value)) = key_value(trimmed) else {
            continue;
        };
        match key {
            "container_name" if !value.contains("${") => found(ComposeProblem::ContainerName),
            "ports" if value.is_empty() => ports_indent = Some(indent),
            "ports" => {
                let items = value.trim_start_matches('[').trim_end_matches(']');
                found_short_ports(items.split(','), &mut found);
            }
            _ => {}
        }
    }
    findings.extend(section.flush());
    findings
}

enum Section {
    /// Top-level `volumes:` or `networks:`.
    Named(NamedSection),
    Other,
}

impl Section {
    fn named(kind: &'static str) -> Section {
        Section::Named(NamedSection {
            kind,
            entry_indent: None,
            key_indent: None,
            fixed_name: None,
            is_external: false,
        })
    }

    /// The finding of the section's last entry, once nothing more of it follows.
    fn flush(&mut self) -> Option<ComposeFinding> {
        match self {
            Section::Named(named) => named.entry_ends(),
            Section::Other => None,
        }
    }
}

/// The entries of a top-level `volumes:` or `networks:`, read one line at a time. An entry's
/// own `name:` is fixed unless it reads a variable; an `external` one is shared on purpose.
struct NamedSection {
    kind: &'static str,
    /// Where each entry's key starts.
    entry_indent: Option<usize>,
    /// Where the current entry's own keys start; deeper ones, as under `driver_opts:`,
    /// are not the entry's name.
    key_indent: Option<usize>,
    /// The line of the current entry's fixed `name:`.
    fixed_name: Option<usize>,
    is_external: bool,
}

impl NamedSection {
    fn read(
        &mut self,
        indent: usize,
        key_value: Option<(&str, &str)>,
        line: usize,
    ) -> Option<ComposeFinding> {
        let entry_indent = *self.entry_indent.get_or_insert(indent);
        if indent <= entry_indent {
            return self.entry_ends();
        }
        if *self.key_indent.get_or_insert(indent) != indent {
            return None;
        }
        match key_value {
            Some(("name", value)) if !value.contains("${") => self.fixed_name = Some(line),
            Some(("external", value)) => self.is_external = unquoted(value) == "true",
            _ => {}
        }
        None
    }

    fn entry_ends(&mut self) -> Option<ComposeFinding> {
        let line = self.fixed_name.take();
        let is_external = std::mem::take(&mut self.is_external);
        self.key_indent = None;
        Some(ComposeFinding {
            line: line.filter(|_| !is_external)?,
            problem: ComposeProblem::FixedName { kind: self.kind },
        })
    }
}

/// Each literal host port of `items`, in compose's short syntax: `HOST:CONTAINER`,
/// `IP:HOST:CONTAINER`, either with `/PROTOCOL`. A container port alone publishes on a port
/// the engine picks, which never collides.
fn found_short_ports<'item>(
    items: impl IntoIterator<Item = &'item str>,
    found: &mut impl FnMut(ComposeProblem),
) {
    for item in items {
        let item = unquoted(item.trim());
        if item.contains("${") {
            continue;
        }
        let without_protocol = item.split_once('/').map_or(item, |(ports, _)| ports);
        let parts: Vec<&str> = without_protocol.split(':').collect();
        if parts.len() < 2 {
            continue;
        }
        if let Some(port) = literal_port(parts[parts.len() - 2]) {
            found(ComposeProblem::LiteralPort { port });
        }
    }
}

/// Each literal `published:` port of `pairs`, in compose's long syntax.
fn found_published_ports<'pair>(
    pairs: impl IntoIterator<Item = &'pair str>,
    found: &mut impl FnMut(ComposeProblem),
) {
    for pair in pairs {
        if let Some(("published", value)) = key_value(pair.trim())
            && let Some(port) = literal_port(unquoted(value))
        {
            found(ComposeProblem::LiteralPort { port });
        }
    }
}

/// `text` when it is a port number or a range of them, as a host port is written.
fn literal_port(text: &str) -> Option<String> {
    let is_literal = text.starts_with(|first: char| first.is_ascii_digit())
        && text
            .chars()
            .all(|character| character.is_ascii_digit() || character == '-');
    is_literal.then(|| text.to_owned())
}

/// `key: value` or `key:`, the key a plain word: not a quoted string or a list item.
fn key_value(text: &str) -> Option<(&str, &str)> {
    let (key, value) = text.split_once(':')?;
    let is_word = !key.is_empty()
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_-.".contains(character));
    let is_mapping = value.is_empty() || value.starts_with(' ');
    (is_word && is_mapping).then(|| (key, value.trim()))
}

fn unquoted(text: &str) -> &str {
    let text = text.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = text
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    text
}

/// `line` up to a `#` that starts a comment: one outside quotes, at the start or after a
/// space.
fn without_comment(line: &str) -> &str {
    let mut quote = None;
    let mut previous = ' ';
    for (at, character) in line.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), _) if character == open => quote = None,
            (None, '#') if previous == ' ' || previous == '\t' => return &line[..at],
            _ => {}
        }
        previous = character;
    }
    line
}

/// The readiness lines of the project's compose files: one per problem, with the variable
/// form that fixes it, or one saying a file runs in parallel sessions. Literal ports are
/// numbered across the files, so each gets a port of the block of its own.
pub(crate) fn requirements(files: &[ComposeFile]) -> Vec<Requirement> {
    let mut requirements = Vec::new();
    let mut ports_named: u16 = 0;
    for file in files {
        let name = format!("compose {}", file.name);
        let text = match &file.text {
            Ok(text) => text,
            Err(reason) => {
                requirements.push(Requirement::needs(
                    name,
                    crate::Status::Unknown,
                    format!("not read: {reason}"),
                    "make it a plain file of the project, then run `torlek doctor` again",
                ));
                continue;
            }
        };
        let findings = scan(text);
        if findings.is_empty() {
            requirements.push(Requirement::available(
                name.clone(),
                "runs in parallel sessions: it fixes no container, volume or network name and \
                 publishes no literal host port",
            ));
        }
        for finding in findings {
            let (detail, fix) = advice(&finding.problem, &mut ports_named);
            requirements.push(
                Requirement::available(
                    format!("{name}:{}", finding.line),
                    format!("runs in one session at a time: {detail}"),
                )
                .with_advice(fix),
            );
        }
    }
    requirements
}

fn advice(problem: &ComposeProblem, ports_named: &mut u16) -> (String, String) {
    match problem {
        ComposeProblem::ContainerName => (
            "`container_name:` gives every session's container one name".to_owned(),
            format!(
                "remove it: compose then names the container after ${COMPOSE_PROJECT_VARIABLE}, \
                 which is each session's own"
            ),
        ),
        ComposeProblem::FixedName { kind } => (
            format!("the {kind}'s `name:` gives every session one {kind}"),
            format!(
                "remove `name:` so compose prefixes the {kind} with ${COMPOSE_PROJECT_VARIABLE}, \
                 or write `name: ${{{COMPOSE_PROJECT_VARIABLE}}}-<name>`"
            ),
        ),
        ComposeProblem::LiteralPort { port } => {
            let detail = format!(
                "it publishes the literal host port {port}, which a second session's `up` \
                 cannot bind"
            );
            if port.contains('-') || *ports_named >= PORT_BLOCK_SIZE {
                return (
                    detail,
                    format!(
                        "publish each port as `${{{PORT_VARIABLE_PREFIX}N:-<port>}}` from the \
                         session's block of {PORT_BLOCK_SIZE}, or the container port alone \
                         (`- \"80\"`), which the engine gives a free host port"
                    ),
                );
            }
            let variable = port_variable(*ports_named);
            *ports_named += 1;
            (
                detail,
                format!(
                    "write `${{{variable}:-{port}}}` in place of {port}: each session's block \
                     sets {variable}, and {port} stays the default outside Torlek"
                ),
            )
        }
    }
}
