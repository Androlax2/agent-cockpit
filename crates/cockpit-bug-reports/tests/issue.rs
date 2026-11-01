//! What `cockpit bug-report send` hands the owner, built without the network: the prefilled
//! new-issue URL of the `bug-from-cockpit` template and the search for an existing issue.
use std::path::Path;

use cockpit_bug_reports::{
    Bundle, FIELDS, Fingerprint, Kind, Origin, Report, ReportId, Status, UNKNOWN_COMPONENT,
    URL_MAX_BYTES, issue,
};

fn report() -> Report {
    Report {
        id: ReportId(3),
        origin: Origin::Agent,
        requested_by_owner: false,
        session: Some("s1".to_owned()),
        status: Status::Draft,
        title: "todo add fails".to_owned(),
        description: "It said internal & stopped.".to_owned(),
        steps: String::new(),
        fingerprint: Fingerprint {
            version: "0.1.0".to_owned(),
            kind: Kind::Internal,
            component: "todo-add".to_owned(),
        },
        environment: "cockpit 0.1.0".to_owned(),
        bundle: Bundle::Written {
            path: "/home/someone/.local/state/cockpit/bug-reports/3.tar".to_owned(),
            contents: "88\tcockpit-support-bundle/platform.json".to_owned(),
        },
        occurrences: 1,
        issue_url: None,
        created_at: 0,
        changed_at: 0,
    }
}

#[test]
fn the_url_prefills_every_field_of_the_template() {
    let issue = issue(&report());

    assert_eq!(
        issue.new_issue_url,
        "https://github.com/Androlax2/agent-cockpit/issues/new?template=bug-from-cockpit.yml\
         &title=todo%20add%20fails\
         &description=It%20said%20internal%20%26%20stopped.\
         &steps=Not%20known.\
         &environment=cockpit%200.1.0\
         &fingerprint=cockpit-0.1.0%2Finternal%2Ftodo-add\
         &bundle=To%20attach%3A%203.tar%0A%0A%60%60%60%0A88%09cockpit-support-bundle%2Fplatform.json%0A%60%60%60"
    );
    assert!(!issue.is_truncated);
}

#[test]
fn the_issue_names_the_bundle_file_never_the_owners_path() {
    let issue = issue(&report());

    assert!(!issue.body.contains("/home/someone"), "{}", issue.body);
    assert!(!issue.new_issue_url.contains("someone"));
    assert!(issue.body.contains("To attach: 3.tar"));
}

#[test]
fn a_long_description_is_cut_to_fit_the_url_with_a_pointer_to_the_full_text() {
    let mut long = report();
    long.description = "word ".repeat(4000);

    let issue = issue(&long);

    assert!(issue.new_issue_url.len() <= URL_MAX_BYTES);
    assert!(issue.is_truncated);
    assert!(
        issue
            .new_issue_url
            .contains("cockpit%20bug-report%20show%203")
    );
    assert!(
        issue.body.contains(&long.description),
        "the body stays whole"
    );
}

#[test]
fn a_specific_fingerprint_comes_with_a_search_for_an_existing_issue() {
    let issue = issue(&report());

    assert_eq!(
        issue.search_url.as_deref(),
        Some(
            "https://github.com/Androlax2/agent-cockpit/issues?q=is%3Aissue%20%22cockpit-0.1.0%2Finternal%2Ftodo-add%22"
        )
    );
}

#[test]
fn an_unspecific_fingerprint_looks_for_no_existing_issue() {
    let mut vague = report();
    vague.fingerprint.kind = Kind::Other;
    vague.fingerprint.component = UNKNOWN_COMPONENT.to_owned();

    assert_eq!(issue(&vague).search_url, None);
}

/// The URL fills the form only through the template's field ids.
#[test]
fn the_template_declares_exactly_the_fields_the_url_fills() {
    let template = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.github/ISSUE_TEMPLATE")
            .join(cockpit_bug_reports::TEMPLATE),
    )
    .expect("the issue template is readable");
    let ids: Vec<&str> = template
        .lines()
        .filter_map(|line| line.trim().strip_prefix("id: "))
        .collect();

    assert_eq!(ids, FIELDS);
}
