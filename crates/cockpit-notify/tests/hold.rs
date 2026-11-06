//! #62, report 19: while a hold stands the notifier keeps quiet, and when the last one ends
//! it tells once what still needs the owner (owner's choice A). Detectors are the owner's
//! programs; `away` is a file.

mod common;

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cockpit_notify::{AWAY, Detector, Detectors, Hold, Notice, Notifier, standing_holds};
use cockpit_protocol::SessionView;
use common::{change_session, loaded, report, session};

const WORKER: &str = "s00000000000a";

fn worker() -> SessionView {
    session(WORKER, 1, 1)
}

fn holds(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// A running notifier whose session just finished, and the notice that told it.
fn finished() -> (Notifier, Vec<Notice>) {
    let mut notifier = loaded(&[worker()], vec![report(WORKER)]);
    let notices = change_session(
        &mut notifier,
        &SessionView {
            finished: true,
            ..worker()
        },
    );
    assert_eq!(notices.len(), 1);
    (notifier, notices)
}

#[test]
fn without_a_hold_every_notice_is_told() {
    let (_, notices) = finished();

    assert_eq!(Hold::default().pass(&notices), notices);
}

#[test]
fn while_a_hold_stands_nothing_is_told_and_its_end_tells_what_still_stands() {
    let (notifier, notices) = finished();
    let mut hold = Hold::default();
    hold.set_standing(holds(&[AWAY]), &notifier);

    assert_eq!(hold.pass(&notices), []);
    assert_eq!(hold.pass(&notices), [], "kept once");
    assert_eq!(hold.set_standing(holds(&[]), &notifier), notices);
    assert_eq!(
        hold.pass(&notices),
        notices,
        "told again once nothing holds"
    );
}

#[test]
fn what_no_longer_stands_when_the_hold_ends_is_not_in_the_summary() {
    let (mut notifier, notices) = finished();
    let mut hold = Hold::default();
    hold.set_standing(holds(&[AWAY]), &notifier);
    hold.pass(&notices);
    change_session(&mut notifier, &worker());

    assert_eq!(hold.set_standing(holds(&[]), &notifier), []);
}

#[test]
fn the_summary_keeps_the_latest_detail_of_a_session_s_event() {
    let (notifier, notices) = finished();
    let mut hold = Hold::default();
    hold.set_standing(holds(&[AWAY]), &notifier);
    let later = Notice {
        detail: Some("done again".to_owned()),
        ..notices[0].clone()
    };
    hold.pass(&notices);
    hold.pass(std::slice::from_ref(&later));

    assert_eq!(hold.set_standing(holds(&[]), &notifier), [later]);
}

#[test]
fn one_hold_ending_while_another_stands_tells_nothing() {
    let (notifier, notices) = finished();
    let mut hold = Hold::default();
    hold.set_standing(holds(&[AWAY, "meeting"]), &notifier);
    hold.pass(&notices);

    assert_eq!(hold.set_standing(holds(&["meeting"]), &notifier), []);
    assert_eq!(hold.set_standing(holds(&[]), &notifier).len(), 1);
}

#[test]
fn a_detector_is_named_and_cannot_take_away_s_name() {
    assert_eq!(
        Detector::parse("meeting=/usr/bin/in-a-meeting").unwrap(),
        Detector {
            name: "meeting".to_owned(),
            program: "/usr/bin/in-a-meeting".into(),
        }
    );
    assert!(Detector::parse("away=/bin/true").is_err());
    assert!(Detector::parse("/bin/true").is_err());
}

fn script(dir: &Path, name: &str, body: &str) -> Detector {
    let program = dir.join(name);
    std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    Detector {
        name: name.to_owned(),
        program,
    }
}

#[tokio::test]
async fn each_detector_that_exits_0_stands() {
    let dir = tempfile::tempdir().unwrap();
    let mut detectors = Detectors::new(vec![
        script(dir.path(), "meeting", "exit 0"),
        script(dir.path(), "game", "exit 1"),
        Detector {
            name: "gone".to_owned(),
            program: dir.path().join("no-such-program"),
        },
    ]);

    assert_eq!(detectors.standing().await, holds(&["meeting"]));
}

#[test]
fn away_stands_while_its_file_exists_beside_what_was_detected() {
    let dir = tempfile::tempdir().unwrap();
    let away_file = dir.path().join("away");
    let detected = holds(&["meeting"]);
    std::fs::write(&away_file, "").unwrap();

    assert_eq!(
        standing_holds(Some(&away_file), &detected),
        holds(&[AWAY, "meeting"])
    );

    std::fs::remove_file(&away_file).unwrap();
    assert_eq!(standing_holds(Some(&away_file), &detected), detected);
}
