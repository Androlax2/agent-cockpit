//! What a send hands the owner: the issue's title and body, the prefilled new-issue URL of
//! the `bug-from-cockpit` template, and a search for an existing issue with the same
//! fingerprint. Built from the draft alone; nothing here touches the network.

use crate::{Bundle, Report};

/// Where Cockpit's issues go.
pub const REPOSITORY: &str = "Androlax2/agent-cockpit";
/// `.github/ISSUE_TEMPLATE/bug-from-cockpit.yml`; its field ids are [`FIELDS`].
pub const TEMPLATE: &str = "bug-from-cockpit.yml";
/// The template's field ids, in the form's order; the URL prefills each.
pub const FIELDS: [&str; 5] = [
    "description",
    "steps",
    "environment",
    "fingerprint",
    "bundle",
];
/// Browsers and GitHub accept URLs well past this; longer text is cut with a pointer to
/// `cockpit bug-report show`, which prints all of it.
pub const URL_MAX_BYTES: usize = 8000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub title: String,
    /// The body `gh issue create` files: the template's sections, untruncated.
    pub body: String,
    pub new_issue_url: String,
    /// The issues that already carry this fingerprint, for a fingerprint specific enough.
    pub search_url: Option<String>,
    /// Whether the URL cut the description or the steps to fit [`URL_MAX_BYTES`].
    pub is_truncated: bool,
}

/// The issue for `report`.
pub fn issue(report: &Report) -> Issue {
    let fingerprint = report.fingerprint.text();
    let bundle = bundle_field(report);
    let steps = if report.steps.is_empty() {
        "Not known.".to_owned()
    } else {
        report.steps.clone()
    };
    let mut fields = [
        report.description.clone(),
        steps,
        report.environment.clone(),
        fingerprint.clone(),
        bundle,
    ];
    let body = FIELDS
        .iter()
        .zip(&fields)
        .map(|(field, value)| format!("### {}\n\n{value}", heading(field)))
        .collect::<Vec<_>>()
        .join("\n\n");
    let marker = format!(
        "\n\n[cut to fit the URL: `cockpit bug-report show {}` prints all of it]",
        report.id.0
    );
    let mut is_truncated = false;
    let mut url = new_issue_url(&report.title, &fields);
    // The free text shrinks first, the longer of the two each pass; then the bundle's listing
    // gives way to its file name. The environment and fingerprint stay whole.
    while url.len() > URL_MAX_BYTES {
        let kept = |field: &String| field.strip_suffix(&marker).unwrap_or(field).to_owned();
        let shrinkable = [0, 1]
            .into_iter()
            .filter(|&index| !kept(&fields[index]).is_empty())
            .max_by_key(|&index| kept(&fields[index]).len());
        if let Some(index) = shrinkable {
            let text = kept(&fields[index]);
            let chars = text.chars().count();
            fields[index] = text.chars().take(chars * 9 / 10).collect::<String>() + &marker;
        } else if let Some(cut) = listing_cut(report)
            && fields[4] != cut
        {
            fields[4] = cut;
        } else {
            break;
        }
        is_truncated = true;
        url = new_issue_url(&report.title, &fields);
    }
    let search_url = report.fingerprint.is_specific().then(|| {
        format!(
            "https://github.com/{REPOSITORY}/issues?q={}",
            encode(&format!("is:issue \"{fingerprint}\""))
        )
    });
    Issue {
        title: report.title.clone(),
        body,
        new_issue_url: url,
        search_url,
        is_truncated,
    }
}

/// The search `gh issue list --search` runs for an existing issue with this fingerprint.
pub fn search_query(report: &Report) -> String {
    format!("\"{}\" in:body", report.fingerprint.text())
}

fn new_issue_url(title: &str, fields: &[String; 5]) -> String {
    let mut url = format!(
        "https://github.com/{REPOSITORY}/issues/new?template={}&title={}",
        encode(TEMPLATE),
        encode(title)
    );
    for (field, value) in FIELDS.iter().zip(fields) {
        url.push('&');
        url.push_str(field);
        url.push('=');
        url.push_str(&encode(value));
    }
    url
}

/// The template's label for a field, as GitHub renders it in the filed issue's body.
fn heading(field: &str) -> &'static str {
    match field {
        "description" => "What happened",
        "steps" => "Steps to reproduce",
        "environment" => "Versions and platform",
        "fingerprint" => "Fingerprint",
        "bundle" => "Support bundle",
        other => unreachable!("FIELDS holds no {other}"),
    }
}

/// What the issue says about the bundle: its file name and contents, never the owner's path.
fn bundle_field(report: &Report) -> String {
    match &report.bundle {
        Bundle::Written { path, contents } => {
            let file_name = path.rsplit('/').next().unwrap_or(path);
            format!("To attach: {file_name}\n\n```\n{contents}\n```")
        }
        Bundle::Pending => "Not written yet.".to_owned(),
        Bundle::Failed { problem } => format!("None: {problem}"),
    }
}

/// The bundle field once its listing no longer fits the URL; `None` without a bundle.
fn listing_cut(report: &Report) -> Option<String> {
    let Bundle::Written { path, .. } = &report.bundle else {
        return None;
    };
    let file_name = path.rsplit('/').next().unwrap_or(path);
    Some(format!(
        "To attach: {file_name}\n\n[listing cut to fit the URL: `cockpit bug-report show {}` prints it]",
        report.id.0
    ))
}

/// Percent-encodes everything but RFC 3986's unreserved characters.
fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
