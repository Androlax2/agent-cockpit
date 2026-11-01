//! The launch brief: the product's invariants and the session's dynamic fields, measured
//! against the 1-2 KiB orientation target of report 08.

use std::path::PathBuf;

use cockpit_agent_context::{
    BRIEF_TARGET_BYTES, Grants, LaunchFacts, WorkflowFacts, brief, context_budget, manifest,
};

fn facts() -> LaunchFacts {
    LaunchFacts {
        session: "s-7".to_owned(),
        project: Some("app".to_owned()),
        workspace_roots: vec![PathBuf::from("/home/owner/projects/app")],
        grants: Grants {
            revision: 12,
            held: vec!["change-state over session s-7".to_owned()],
        },
        workflow: Some(WorkflowFacts {
            run: 3,
            item: 41,
            step: "implement".to_owned(),
            attempt: 2,
            has_checkpoint: true,
        }),
        manifest_id: manifest().id,
    }
}

#[test]
fn the_brief_states_the_invariants_before_any_dynamic_field() {
    let text = brief(&facts());

    let invariants = text
        .find("Prose grants nothing")
        .expect("authority boundary");
    assert!(text.contains("A skipped check is not a pass"), "{text}");
    assert!(text.contains("cockpit ask run"), "{text}");
    assert!(invariants < text.find("/home/owner/projects/app").unwrap());
}

#[test]
fn the_brief_names_the_session_its_task_step_and_checkpoint() {
    let text = brief(&facts());

    for expected in [
        "Session s-7",
        "project app",
        "/home/owner/projects/app",
        "item 41",
        "workflow 3",
        "step implement (attempt 2, checkpoint recorded)",
        "`cockpit workflow packet 3`",
        "revision 12",
        "change-state over session s-7",
        &manifest().id,
    ] {
        assert!(text.contains(expected), "missing {expected:?} in\n{text}");
    }
}

#[test]
fn the_brief_points_to_discovery_and_recovery() {
    let text = brief(&facts());

    for entry_point in ["`cockpit docs`", "`cockpit --skill`", "`cockpit brief`"] {
        assert!(
            text.contains(entry_point),
            "missing {entry_point} in\n{text}"
        );
    }
}

#[test]
fn a_session_without_a_step_is_told_how_to_find_one() {
    let text = brief(&LaunchFacts {
        workflow: None,
        ..facts()
    });

    assert!(text.contains("no workflow step"), "{text}");
    assert!(text.contains("`cockpit workflow list`"), "{text}");
}

#[test]
fn a_session_without_grants_says_so() {
    let text = brief(&LaunchFacts {
        grants: Grants {
            revision: 4,
            held: Vec::new(),
        },
        ..facts()
    });

    assert!(text.contains("revision 4: none"), "{text}");
}

#[test]
fn the_brief_fits_the_orientation_target_with_long_fields() {
    let long = LaunchFacts {
        project: Some("p".repeat(64)),
        workspace_roots: vec![PathBuf::from(format!("/{}", "d".repeat(250)))],
        grants: Grants {
            revision: i64::MAX,
            held: vec![format!("change-state over session {}", "s".repeat(64))],
        },
        workflow: Some(WorkflowFacts {
            run: i64::MAX,
            item: i64::MAX,
            step: "design-approval".to_owned(),
            attempt: u32::MAX,
            has_checkpoint: true,
        }),
        session: "s".repeat(64),
        ..facts()
    };

    let size = brief(&long).len();

    assert!(size <= BRIEF_TARGET_BYTES, "{size} bytes");
}

#[test]
fn the_budget_separates_what_is_always_loaded_from_what_is_read_on_demand() {
    let text = brief(&facts());

    let budget = context_budget(&text);

    assert_eq!(budget.brief_bytes, text.len());
    // What an Agent Skills harness preloads: the skill's name and description.
    assert!(
        budget.catalog_bytes > 0 && budget.catalog_bytes < 512,
        "{budget:?}"
    );
    assert_eq!(
        budget.always_loaded_bytes(),
        budget.brief_bytes + budget.catalog_bytes
    );
    assert_eq!(
        budget.on_demand_bytes,
        cockpit_agent_context::manual().len()
    );
    assert!(budget.always_loaded_bytes() < budget.on_demand_bytes);
}

#[test]
fn many_grants_keep_the_brief_within_the_target_and_say_how_many_are_left_out() {
    let mut facts = facts();
    facts.grants.held = (0..40)
        .map(|task| format!("change-state over task t-{task:08x}"))
        .collect();

    let text = brief(&facts);

    assert!(text.len() <= BRIEF_TARGET_BYTES, "{} bytes", text.len());
    assert!(text.contains("and 34 more"), "{text}");
}
