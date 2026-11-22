//! The push-branch patterns the owner adds per project (#155), kept in the project store.

mod common;

use cockpit_projects::{BranchPattern, Error, add_push_branch, push_branches, remove_push_branch};
use common::{Scratch, register};

fn pattern(text: &str) -> BranchPattern {
    BranchPattern::new(text).unwrap()
}

async fn registered() -> (Scratch, cockpit_turso::Database, String) {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let (project, _) = register(&database, &scratch.repository("app")).await;
    let name = project.name.as_str().to_owned();
    (scratch, database, name)
}

async fn listed(database: &cockpit_turso::Database, project: &str) -> Vec<String> {
    let read = database.read().await.unwrap();
    push_branches(&read, project)
        .await
        .unwrap()
        .iter()
        .map(|pattern| pattern.as_str().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_has_no_push_branch_until_the_owner_adds_one() {
    let (_scratch, database, project) = registered().await;

    assert!(listed(&database, &project).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn added_patterns_are_listed_in_name_order_once_each() {
    let (_scratch, database, project) = registered().await;

    let first = add_push_branch(&database, &project, &pattern("release/*")).await;
    let second = add_push_branch(&database, &project, &pattern("agent/**")).await;
    let again = add_push_branch(&database, &project, &pattern("agent/**")).await;

    assert_eq!(
        (first.unwrap(), second.unwrap(), again.unwrap()),
        (true, true, false)
    );
    assert_eq!(listed(&database, &project).await, ["agent/**", "release/*"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_pattern_is_no_longer_listed() {
    let (_scratch, database, project) = registered().await;
    add_push_branch(&database, &project, &pattern("agent/*"))
        .await
        .unwrap();

    let removed = remove_push_branch(&database, &project, &pattern("agent/*")).await;
    let absent = remove_push_branch(&database, &project, &pattern("agent/*")).await;

    assert_eq!((removed.unwrap(), absent.unwrap()), (true, false));
    assert!(listed(&database, &project).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pattern_for_a_project_that_is_not_registered_is_refused() {
    let (_scratch, database, _) = registered().await;

    let added = add_push_branch(&database, "elsewhere", &pattern("agent/*")).await;

    assert!(
        matches!(added, Err(Error::UnknownProject { .. })),
        "{added:?}"
    );
}
