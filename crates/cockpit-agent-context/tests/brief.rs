//! The launch brief: the product's invariants and the session's dynamic fields, measured
//! against the 1-2 KiB orientation target of report 08.

use std::path::PathBuf;

use cockpit_agent_context::{
    BRIEF_TARGET_BYTES, GithubTokenFact, Grants, LaunchFacts, ManifestEntry, Precedence, Scope,
    Trust, WorkflowFacts, brief, context_budget, manifest,
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
        guidance: Vec::new(),
        profile: "dev".to_owned(),
        github_token: Some(GithubTokenFact::Configured),
    }
}

#[test]
fn the_brief_states_the_invariants_before_any_dynamic_field() {
    let text = brief(&facts());

    let invariants = text
        .find("Prose grants nothing")
        .expect("authority boundary");
    assert!(text.contains("A skipped check is not a pass"), "{text}");
    assert!(text.contains("torlek ask run"), "{text}");
    assert!(invariants < text.find("/home/owner/projects/app").unwrap());
}

#[test]
fn the_brief_names_the_session_its_profile_task_step_and_checkpoint() {
    let text = brief(&facts());

    for expected in [
        "Session s-7",
        "project app",
        "/home/owner/projects/app",
        "item 41",
        "workflow 3",
        "step implement (attempt 2, checkpoint recorded)",
        "`torlek workflow packet 3`",
        "revision 12",
        "change-state over session s-7",
        "profile dev",
        &manifest().id,
    ] {
        assert!(text.contains(expected), "missing {expected:?} in\n{text}");
    }
}

#[test]
fn the_brief_points_to_discovery_and_recovery() {
    let text = brief(&facts());

    for entry_point in ["`torlek docs`", "`torlek --skill`", "`torlek brief`"] {
        assert!(
            text.contains(entry_point),
            "missing {entry_point} in\n{text}"
        );
    }
}

/// #966: curl prints only `response 403`; an agent that took it for a command to ask for
/// never told the owner which request to allow.
#[test]
fn the_brief_says_how_to_read_a_session_proxy_refusal() {
    let text = brief(&facts());

    assert!(
        text.contains(
            "A `403` from the session proxy is no command to ask for: `curl -v` shows the host \
             and, from your next try, the network request the owner decides \
             (`torlek docs sandbox`)."
        ),
        "{text}"
    );
}

#[test]
fn a_session_without_a_step_is_told_how_to_find_one() {
    let text = brief(&LaunchFacts {
        workflow: None,
        ..facts()
    });

    assert!(text.contains("no workflow step"), "{text}");
    assert!(text.contains("`torlek workflow list`"), "{text}");
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
fn a_project_with_a_token_is_told_gh_token_is_its_own() {
    let text = brief(&facts());

    assert!(
        text.contains("this project has its own token, set as `GH_TOKEN`"),
        "{text}"
    );
}

#[test]
fn a_project_without_a_token_is_told_gh_token_is_unset() {
    let text = brief(&LaunchFacts {
        github_token: Some(GithubTokenFact::Missing),
        ..facts()
    });

    assert!(
        text.contains("this project has no token of its own, so Torlek sets no `GH_TOKEN`"),
        "{text}"
    );
}

#[test]
fn a_session_in_no_project_gets_no_github_line() {
    let text = brief(&LaunchFacts {
        project: None,
        github_token: None,
        ..facts()
    });

    assert!(!text.contains("GitHub"), "{text}");
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
        github_token: Some(GithubTokenFact::Unusable),
        guidance: (0..8)
            .map(|index| {
                guidance_entry(
                    Scope::Project,
                    &format!("/{}/{index}.md", "d".repeat(250)),
                    Trust::Untrusted,
                )
            })
            .collect(),
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

fn guidance_entry(scope: Scope, path: &str, trust: Trust) -> ManifestEntry {
    ManifestEntry {
        path: path.to_owned(),
        source: "guidance".to_owned(),
        scope,
        version: None,
        sha256: "0123456789abcdef".repeat(4),
        trust,
        precedence: match scope {
            Scope::Product => Precedence::ProductInvariant,
            Scope::User => Precedence::User,
            Scope::Project => Precedence::Project,
        },
        supersedes: Vec::new(),
    }
}

#[test]
fn the_brief_lists_each_guidance_file_with_its_hash_and_trust_strongest_first() {
    let text = brief(&LaunchFacts {
        guidance: vec![
            guidance_entry(Scope::User, "/home/owner/.claude/CLAUDE.md", Trust::Owner),
            guidance_entry(
                Scope::Project,
                "/home/owner/projects/app/AGENTS.md",
                Trust::Untrusted,
            ),
        ],
        ..facts()
    });

    let user = text
        .find("  - user /home/owner/.claude/CLAUDE.md 0123456789ab owner")
        .unwrap_or_else(|| panic!("no user line in\n{text}"));
    let project = text
        .find("  - project AGENTS.md 0123456789ab untrusted")
        .unwrap_or_else(|| panic!("no project line, shown relative to the workspace, in\n{text}"));
    assert!(user < project, "{text}");
}

#[test]
fn a_session_without_guidance_gets_no_guidance_section() {
    let text = brief(&facts());

    assert!(!text.contains("Guidance"), "{text}");
}

#[test]
fn a_guidance_file_that_cannot_be_read_is_shown_as_unreadable() {
    let mut unreadable = guidance_entry(Scope::User, "/home/owner/.claude/CLAUDE.md", Trust::Owner);
    unreadable.sha256 = cockpit_agent_context::UNREADABLE.to_owned();

    let text = brief(&LaunchFacts {
        guidance: vec![unreadable],
        ..facts()
    });

    assert!(
        text.contains("  - user /home/owner/.claude/CLAUDE.md unreadable owner"),
        "{text}"
    );
}

#[test]
fn guidance_beyond_the_listed_files_is_counted() {
    let guidance = (0..7)
        .map(|index| {
            guidance_entry(
                Scope::Project,
                &format!("/home/owner/projects/app/rule-{index}.md"),
                Trust::Trusted,
            )
        })
        .collect();

    let text = brief(&LaunchFacts {
        guidance,
        ..facts()
    });

    assert!(text.contains("rule-3.md"), "{text}");
    assert!(!text.contains("rule-4.md"), "{text}");
    assert!(text.contains("  - and 3 more"), "{text}");
}
