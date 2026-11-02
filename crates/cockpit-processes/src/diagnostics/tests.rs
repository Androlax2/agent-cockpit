use super::*;
use std::path::Path;
use std::time::Instant;

fn write_stat(root: &Path, pid: i32, parent: i32, group: i32, state: char) {
    std::fs::create_dir_all(root.join(pid.to_string())).unwrap();
    std::fs::write(root.join(format!("{pid}/stat")), format!(
        "{pid} (secret ) with (parens)) {state} {parent} {group} 123 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 987\n"
    )).unwrap();
}

fn limits() -> Limits {
    Limits {
        entries: 16,
        members: 2,
        stat_bytes: 4096,
    }
}

#[test]
fn sampling_retains_zombies_and_reaper_identity_without_command_text() {
    let root = tempfile::tempdir().unwrap();
    write_stat(root.path(), 100, 200, 100, 'Z');
    write_stat(root.path(), 101, 200, 100, 'S');
    write_stat(root.path(), 200, 1, 200, 'S');
    let sample = scan(
        root.path(),
        100,
        limits(),
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(sample.members.len(), 2);
    assert!(sample.members.contains(&ProcessState {
        pid: 100,
        parent: 200,
        group: 100,
        session: 123,
        state: 'Z',
        start_ticks: 987
    }));
    assert_eq!(sample.parents.len(), 1);
    assert!(
        matches!(&sample.parents[0].read, ProcessRead::Present(parent) if parent.pid == 200 && parent.start_ticks == 987)
    );
    assert!(!format!("{sample:?}").contains("secret"));
}

#[test]
fn failed_stat_reads_are_explicit_without_raw_content() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("100")).unwrap();
    let missing = read_process(root.path(), 100, 4096);
    assert_eq!(missing, ProcessRead::Absent);
    std::fs::create_dir(root.path().join("100/stat")).unwrap();
    assert!(matches!(
        read_process(root.path(), 100, 4096),
        ProcessRead::Unreadable { .. }
    ));
    std::fs::remove_dir(root.path().join("100/stat")).unwrap();
    std::fs::write(root.path().join("100/stat"), "secret malformed stat").unwrap();
    assert_eq!(read_process(root.path(), 100, 4096), ProcessRead::Malformed);
    assert_eq!(read_process(root.path(), 100, 4), ProcessRead::TooLarge);
}

#[test]
fn a_reused_or_malformed_pid_is_not_labelled_as_the_requested_process() {
    let root = tempfile::tempdir().unwrap();
    write_stat(root.path(), 100, 1, 100, 'S');
    std::fs::rename(root.path().join("100"), root.path().join("101")).unwrap();
    assert_eq!(read_process(root.path(), 101, 4096), ProcessRead::Malformed);
}

#[test]
fn sampling_marks_work_limits_and_missing_records() {
    let root = tempfile::tempdir().unwrap();
    write_stat(root.path(), 100, 999, 100, 'Z');
    write_stat(root.path(), 101, 999, 100, 'S');
    let capped = Limits {
        members: 1,
        ..limits()
    };
    let sample = scan(
        root.path(),
        100,
        capped,
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(sample.status, SnapshotStatus::MemberLimit);
    assert_eq!(sample.members.len(), 1);
    assert_eq!(sample.parents[0].read, ProcessRead::Absent);
    let capped = Limits {
        entries: 0,
        ..limits()
    };
    let sample = scan(
        root.path(),
        100,
        capped,
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(sample.status, SnapshotStatus::EntryLimit);
    assert_eq!(sample.entries_examined, 0);
    assert_eq!(
        scan(root.path(), 100, limits(), Instant::now()).status,
        SnapshotStatus::Deadline
    );
    assert!(matches!(
        scan(
            &root.path().join("missing"),
            100,
            limits(),
            Instant::now() + Duration::from_secs(1)
        )
        .status,
        SnapshotStatus::ReadError { .. }
    ));
}

#[test]
fn unreadable_candidates_prevent_a_complete_census_claim() {
    let root = tempfile::tempdir().unwrap();
    write_stat(root.path(), 100, 1, 100, 'S');
    std::fs::create_dir(root.path().join("101")).unwrap();
    std::fs::write(root.path().join("101/stat"), "bad stat").unwrap();
    std::fs::create_dir(root.path().join("102")).unwrap();
    let sample = scan(
        root.path(),
        100,
        limits(),
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(sample.unreadable_entries, 1);
    assert_eq!(sample.vanished_entries, 1);
}

#[tokio::test]
async fn a_timed_out_worker_keeps_its_slot_until_it_really_finishes() {
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let (release, wait) = std::sync::mpsc::channel();
    let sample = collect(&SLOTS, Duration::from_millis(20), move |_| {
        wait.recv_timeout(Duration::from_secs(1)).unwrap();
        ProcessSnapshot::unavailable(SnapshotStatus::Sampled)
    })
    .await;
    assert_eq!(sample.status, SnapshotStatus::Deadline);
    let second = collect(&SLOTS, Duration::from_millis(20), |_| {
        panic!("no second worker admitted")
    })
    .await;
    assert_eq!(second.status, SnapshotStatus::Busy);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if SLOTS.available_permits() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}
