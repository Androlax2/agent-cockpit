//! Symbolic links in a checkout are never followed when artifacts are fingerprinted or
//! checked, so no digest of a file outside the checkout is recorded or compared.
mod common;

use std::os::unix::fs::symlink;

use cockpit_verification::{CheckStatus, Outcome, StaleReason};
use common::{Fixture, check, clean_source, refused_field};

/// A directory beside the checkout holding `lib.rs` with `content`.
fn outside_directory(fixture: &Fixture, content: &str) -> std::path::PathBuf {
    let outside = fixture.dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("lib.rs"), content).unwrap();
    outside
}

#[tokio::test]
async fn fingerprinting_refuses_a_directory_linked_out_of_the_checkout() {
    let fixture = Fixture::new().await;
    let outside = outside_directory(&fixture, "secret\n");
    symlink(&outside, fixture.checkout().join("src")).unwrap();

    let refused = cockpit_verification::fingerprint(&fixture.checkout(), &["src/lib.rs"]);
    assert_eq!(refused_field(refused.unwrap_err()), "artifacts");
}

#[tokio::test]
async fn fingerprinting_refuses_a_file_linked_out_of_the_checkout() {
    let fixture = Fixture::new().await;
    let outside = outside_directory(&fixture, "secret\n");
    symlink(outside.join("lib.rs"), fixture.checkout().join("lib.rs")).unwrap();

    let refused = cockpit_verification::fingerprint(&fixture.checkout(), &["lib.rs"]);
    assert_eq!(refused_field(refused.unwrap_err()), "artifacts");
}

#[tokio::test]
async fn fingerprinting_refuses_a_link_that_stays_inside_the_checkout() {
    let fixture = Fixture::new().await;
    fixture.write_file("src/real.rs", "inside\n");
    symlink("real.rs", fixture.checkout().join("src/lib.rs")).unwrap();

    let refused = cockpit_verification::fingerprint(&fixture.checkout(), &["src/lib.rs"]);
    assert_eq!(refused_field(refused.unwrap_err()), "artifacts");
}

#[tokio::test]
async fn a_directory_swapped_for_a_link_out_of_the_checkout_makes_its_receipt_unreadable() {
    let fixture = Fixture::new().await;
    fixture.write_file("src/lib.rs", "same\n");
    let tests = check("test");
    fixture
        .record_run(
            "run-1",
            &tests,
            &clean_source(),
            &["src/lib.rs"],
            Outcome::Passed,
        )
        .await;
    // The outside file has the same content: following the link would report a pass.
    let outside = outside_directory(&fixture, "same\n");
    std::fs::rename(
        fixture.checkout().join("src"),
        fixture.checkout().join("src.old"),
    )
    .unwrap();
    symlink(&outside, fixture.checkout().join("src")).unwrap();

    let report = fixture.gate(&clean_source(), &[tests]).await;
    assert!(
        matches!(
            &report.checks[0].status,
            CheckStatus::Stale(StaleReason::ArtifactUnreadable { path, .. }) if path == "src/lib.rs"
        ),
        "{:?}",
        report.checks[0].status
    );
}
