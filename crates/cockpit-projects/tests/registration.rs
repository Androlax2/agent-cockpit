//! Registering projects: path validation and the main checkout, on a real state database.
mod common;

use cockpit_projects::{CheckoutKind, Error, Git, projects, register_project};
use common::{Scratch, git, register};

#[tokio::test]
async fn a_work_tree_top_level_registers_with_its_main_checkout_at_incarnation_one() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");

    let (project, main) = register(&database, &root).await;

    assert_eq!(project.name.as_str(), "app");
    assert_eq!(project.root, root);
    assert_eq!(
        (main.kind, main.path, main.incarnation),
        (CheckoutKind::Main, root, 1)
    );
}

#[tokio::test]
async fn a_registered_project_is_listed_after_reopening_the_database() {
    let scratch = Scratch::new();
    let root = scratch.repository("app");
    {
        let database = scratch.open().await;
        register(&database, &root).await;
    }

    let database = scratch.open().await;
    let read = database.read().await.unwrap();
    let listed = projects(&read).await.unwrap();

    assert_eq!(
        listed
            .iter()
            .map(|project| &project.root)
            .collect::<Vec<_>>(),
        [&root]
    );
}

#[tokio::test]
async fn a_relative_path_is_refused() {
    let scratch = Scratch::new();
    let database = scratch.open().await;

    let refused = register_project(&database, &Git::from_path(), "app".as_ref(), None).await;

    assert!(
        matches!(&refused, Err(Error::InvalidPath { reason, .. }) if reason.contains("absolute")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_subdirectory_of_a_work_tree_is_refused_naming_the_top_level() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    std::fs::create_dir(root.join("src")).unwrap();

    let refused = register_project(&database, &Git::from_path(), &root.join("src"), None).await;

    assert!(
        matches!(&refused, Err(Error::InvalidPath { reason, .. })
            if reason.contains(&root.display().to_string())),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_checkout_whose_git_metadata_lives_elsewhere_is_refused() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    let worktree = scratch.path().join("app-worktree");
    git(
        &root,
        &["worktree", "add", "--quiet", worktree.to_str().unwrap()],
    );

    let refused = register_project(&database, &Git::from_path(), &worktree, None).await;

    assert!(
        matches!(&refused, Err(Error::InvalidPath { reason, .. }) if reason.contains(".git")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_directory_that_is_not_a_repository_is_refused() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let plain = scratch.path().join("plain");
    std::fs::create_dir(&plain).unwrap();

    let refused = register_project(&database, &Git::from_path(), &plain, None).await;

    assert!(
        matches!(&refused, Err(Error::InvalidPath { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_root_inside_a_registered_project_is_refused() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    register(&database, &root).await;
    let nested = root.join("vendor/lib");
    std::fs::create_dir_all(&nested).unwrap();
    git(&nested, &["init", "--quiet"]);

    let refused = register_project(&database, &Git::from_path(), &nested, None).await;

    assert!(
        matches!(&refused, Err(Error::Overlaps { project, .. }) if project.as_str() == "app"),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_second_project_with_the_same_name_is_refused() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    register(&database, &scratch.repository("app")).await;
    let other = scratch.repository("other");

    let refused = register_project(&database, &Git::from_path(), &other, Some("app")).await;

    assert!(
        matches!(&refused, Err(Error::NameTaken { name }) if name.as_str() == "app"),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_name_that_could_escape_the_clones_directory_is_refused() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");

    for name in ["..", ".hidden", "a/b", "", "-flag"] {
        let refused = register_project(&database, &Git::from_path(), &root, Some(name)).await;
        assert!(
            matches!(&refused, Err(Error::InvalidName { .. })),
            "{name:?}: {refused:?}"
        );
    }
}
