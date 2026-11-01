//! The documents shipped in this binary. Their text lives in `bundle/`, laid out as an Agent
//! Skills skill: `SKILL.md` is the overview a harness preloads by name and description, and
//! each topic is a reference read on demand (progressive disclosure, report 08).

use std::sync::LazyLock;

/// The release these instructions describe; every file and the manual carry it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One instruction packet of the bundle.
#[derive(Debug)]
pub struct Document {
    /// The topic `cockpit docs TOPIC` reads.
    pub name: &'static str,
    /// Where the daemon writes it, relative to the bundle directory.
    pub path: &'static str,
    /// One line for the catalog: when to read it.
    pub summary: &'static str,
    /// The YAML frontmatter without its `---` fences, for the skill file only.
    pub frontmatter: Option<&'static str>,
    pub body: &'static str,
}

impl Document {
    /// The file as the daemon writes it: frontmatter first (Agent Skills requires it), then
    /// the version stamp, then the body.
    pub fn file_text(&self) -> String {
        let stamp = format!(
            "<!-- Cockpit {VERSION} instruction bundle, written at each daemon start; never edit: \
             the next start overwrites it. -->\n\n"
        );
        match self.frontmatter {
            Some(frontmatter) => format!("---\n{frontmatter}\n---\n\n{stamp}{}", self.body),
            None => format!("{stamp}{}", self.body),
        }
    }
}

/// A topic and when to read it: what a harness or `cockpit docs` lists before loading any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub name: &'static str,
    pub summary: &'static str,
}

const SKILL: &str = include_str!("../bundle/SKILL.md");

/// Topic name, file, summary and text, in the overview's order.
const REFERENCES: [(&str, &str, &str, &str); 5] = [
    (
        "workflow",
        "cockpit/references/workflow.md",
        "begin, checkpoint, finish or hand back a workflow step, or block it",
        include_str!("../bundle/references/workflow.md"),
    ),
    (
        "checks",
        "cockpit/references/checks.md",
        "prove a result with receipts and see what a gate still needs",
        include_str!("../bundle/references/checks.md"),
    ),
    (
        "approvals",
        "cockpit/references/approvals.md",
        "ask the owner to run a host command or do manual steps",
        include_str!("../bundle/references/approvals.md"),
    ),
    (
        "coordination",
        "cockpit/references/coordination.md",
        "report status, ask the owner, file and work task items",
        include_str!("../bundle/references/coordination.md"),
    ),
    (
        "sandbox",
        "cockpit/references/sandbox.md",
        "denied paths, blocked hosts, host commands, Docker and secrets",
        include_str!("../bundle/references/sandbox.md"),
    ),
];

static DOCUMENTS: LazyLock<Vec<Document>> = LazyLock::new(|| {
    let (frontmatter, body) = SKILL
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .expect("bundle/SKILL.md opens with YAML frontmatter");
    let overview = Document {
        name: "overview",
        path: "cockpit/SKILL.md",
        summary: "start here: the brief, the rules nothing overrides, the topics",
        frontmatter: Some(frontmatter),
        body: body.trim_start(),
    };
    std::iter::once(overview)
        .chain(
            REFERENCES
                .iter()
                .map(|(name, path, summary, body)| Document {
                    name,
                    path,
                    summary,
                    frontmatter: None,
                    body,
                }),
        )
        .collect()
});

/// Every document, the overview first.
pub fn documents() -> &'static [Document] {
    &DOCUMENTS
}

pub fn document(name: &str) -> Option<&'static Document> {
    documents().iter().find(|document| document.name == name)
}

pub fn catalog() -> Vec<CatalogEntry> {
    documents()
        .iter()
        .map(|document| CatalogEntry {
            name: document.name,
            summary: document.summary,
        })
        .collect()
}

/// The one-read manual `cockpit --skill` prints: every document, the overview first, for an
/// agent whose harness offers nothing else.
pub fn manual() -> String {
    let mut manual = format!(
        "<!-- Cockpit {VERSION} instruction bundle, manifest {}; `cockpit docs TOPIC` reads one \
         topic. -->\n",
        crate::manifest().id
    );
    for document in documents() {
        manual.push('\n');
        manual.push_str(document.body.trim());
        manual.push('\n');
    }
    manual
}
