use super::*;
use flate2::{Compression, write::GzEncoder};
use std::io::Cursor;
use tar::{Builder, EntryType, Header};

fn archive(engine: &str, commit: &str, extra: Option<(&str, EntryType, &str)>) -> Vec<u8> {
    let mut tar = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let prefix = if engine == "greywall" {
        "greywall_fixture/"
    } else {
        ""
    };
    let script = if engine == "greywall" {
        format!("#!/bin/sh\nprintf 'Version: fixture\\nCommit: {commit}\\n'\n")
    } else {
        format!("#!/bin/sh\nprintf 'greyproxy fixture\\ncommit: {commit}\\n'\n")
    };
    let mut files = vec![
        (format!("{prefix}{engine}"), script),
        (format!("{prefix}LICENSE"), "licence".into()),
    ];
    if engine == "greywall" {
        files.extend([
            (format!("{prefix}NOTICE"), "notice".into()),
            (
                format!("{prefix}LICENSES/tun2socks-GPL-3.0.txt"),
                "GPL".into(),
            ),
        ]);
    }
    for (path, data) in files {
        member(&mut tar, &path, EntryType::Regular, &data);
    }
    if engine == "greywall" {
        member(
            &mut tar,
            "greywall_fixture/greywatch",
            EntryType::Symlink,
            "greywall",
        );
    }
    if let Some((path, kind, data)) = extra {
        member(&mut tar, path, kind, data);
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn member(tar: &mut Builder<GzEncoder<Vec<u8>>>, path: &str, kind: EntryType, data: &str) {
    let mut header = Header::new_ustar();
    // Raw header names deliberately allow malformed fixture paths.
    header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
    header.set_entry_type(kind);
    header.set_mode(0o7777);
    let content = if kind.is_symlink() || kind.is_hard_link() {
        header.set_link_name(data).unwrap();
        ""
    } else {
        data
    };
    header.set_size(content.len() as u64);
    header.set_cksum();
    tar.append(&header, Cursor::new(content)).unwrap();
}

fn fixture(extra: Option<(&str, EntryType, &str)>) -> (tempfile::TempDir, [Pin; 2], [Vec<u8>; 2]) {
    let home = tempfile::tempdir().unwrap();
    fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let assets = [
        archive("greywall", crate::PINNED_GREYWALL_COMMIT, extra),
        archive("greyproxy", crate::PINNED_GREYPROXY_COMMIT, None),
    ];
    let mut pins = pins_for(host_target().unwrap()).unwrap();
    for (pin, bytes) in pins.iter_mut().zip(&assets) {
        pin.sha256 = format!("{:x}", Sha256::digest(bytes));
    }
    pins[0].archive_root = "greywall_fixture".into();
    (home, pins, assets)
}

fn run(
    home: &Path,
    pins: &[Pin; 2],
    assets: &[Vec<u8>; 2],
) -> Result<GreywallPrograms, InstallError> {
    install_with(home, pins, |pin, file| {
        fs::write(file, &assets[usize::from(pin.name == "greyproxy")]).map_err(InstallError::Io)
    })
}

#[test]
fn installs_both_layouts_licences_and_the_greywatch_alias_atomically() {
    let (home, pins, assets) = fixture(None);
    let installed = run(home.path(), &pins, &assets).unwrap();
    assert_eq!(
        fs::read_link(installed.greywall.with_file_name("greywatch")).unwrap(),
        Path::new("greywall")
    );
    assert_eq!(
        fs::read_to_string(installed.greywall.with_file_name("NOTICE")).unwrap(),
        "notice"
    );
    assert_eq!(
        fs::read_to_string(
            installed
                .greywall
                .with_file_name("LICENSES/tun2socks-GPL-3.0.txt")
        )
        .unwrap(),
        "GPL"
    );
    assert_eq!(
        fs::read_to_string(installed.greyproxy.with_file_name("LICENSE")).unwrap(),
        "licence"
    );
    assert_eq!(
        fs::metadata(&installed.greywall)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o755
    );
    assert_eq!(
        run(home.path(), &pins, &assets).unwrap().greywall,
        installed.greywall
    );
    assert_eq!(
        fs::read_dir(home.path().join("engines")).unwrap().count(),
        1
    );
}

#[test]
fn tampered_second_asset_leaves_no_installation_or_staging() {
    let (home, pins, mut assets) = fixture(None);
    assets[1].push(1);
    assert!(matches!(
        run(home.path(), &pins, &assets),
        Err(InstallError::Checksum {
            engine: "greyproxy",
            ..
        })
    ));
    assert_eq!(
        fs::read_dir(home.path().join("engines")).unwrap().count(),
        0
    );
}

#[test]
fn unsafe_paths_links_duplicates_and_special_entries_are_refused() {
    for (path, kind, data) in [
        ("../outside", EntryType::Regular, "bad"),
        ("/absolute", EntryType::Regular, "bad"),
        ("greywall_fixture/evil", EntryType::Symlink, "../../outside"),
        ("greywall_fixture/LICENSE", EntryType::Link, "../../outside"),
        (
            "greywall_fixture/greywatch/child",
            EntryType::Regular,
            "bad",
        ),
        ("greywall_fixture/LICENSE", EntryType::Regular, "duplicate"),
        ("greywall_fixture/fifo", EntryType::Fifo, ""),
    ] {
        let (home, pins, assets) = fixture(Some((path, kind, data)));
        assert!(
            matches!(
                run(home.path(), &pins, &assets),
                Err(InstallError::Archive { .. })
            ),
            "{path}"
        );
        assert_eq!(
            fs::read_dir(home.path().join("engines")).unwrap().count(),
            0
        );
    }
}

#[test]
fn a_verified_archive_with_the_wrong_commit_is_refused_without_activation() {
    let (home, mut pins, mut assets) = fixture(None);
    assets[1] = archive(
        "greyproxy",
        "0000000000000000000000000000000000000000",
        None,
    );
    pins[1].sha256 = format!("{:x}", Sha256::digest(&assets[1]));
    assert!(matches!(
        run(home.path(), &pins, &assets),
        Err(InstallError::Commit {
            engine: "greyproxy",
            ..
        })
    ));
    assert_eq!(
        fs::read_dir(home.path().join("engines")).unwrap().count(),
        0
    );
}

#[test]
fn unsupported_target_and_incomplete_pins_are_typed_refusals() {
    assert!(matches!(
        pins_for("x86_64-apple-darwin"),
        Err(InstallError::UnsupportedTarget(_))
    ));
    for target in [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
    ] {
        let pins = pins_for(target).unwrap();
        assert_eq!(pins[0].commit, crate::PINNED_GREYWALL_COMMIT);
        assert_eq!(pins[1].commit, crate::PINNED_GREYPROXY_COMMIT);
    }
}

#[test]
fn a_failed_upgrade_keeps_the_previous_release_and_a_linked_root_is_refused() {
    let (home, pins, mut assets) = fixture(None);
    let previous = run(home.path(), &pins, &assets).unwrap();
    let mut next = pins.clone();
    next[1].commit = "1111111111111111111111111111111111111111".into();
    assets[1].push(2);
    assert!(run(home.path(), &next, &assets).is_err());
    assert!(previous.greywall.is_file());
    assert_eq!(
        fs::read_dir(home.path().join("engines")).unwrap().count(),
        1
    );
    let other = tempfile::tempdir().unwrap();
    fs::set_permissions(other.path(), fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(home.path().join("engines"), other.path().join("engines")).unwrap();
    assert!(matches!(
        run(other.path(), &pins, &assets),
        Err(InstallError::UnsafePath(_))
    ));
}

#[test]
fn truncated_gzip_and_missing_notice_are_refused() {
    for truncate in [true, false] {
        let (home, mut pins, mut assets) = fixture(None);
        if truncate {
            assets[0].truncate(assets[0].len() - 8);
        } else {
            let mut source = tar::Archive::new(GzDecoder::new(Cursor::new(&assets[0])));
            let mut replacement = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
            for entry in source.entries().unwrap() {
                let mut entry = entry.unwrap();
                if entry.path().unwrap().ends_with("NOTICE") {
                    continue;
                }
                replacement
                    .append(&entry.header().clone(), &mut entry)
                    .unwrap();
            }
            assets[0] = replacement.into_inner().unwrap().finish().unwrap();
        }
        pins[0].sha256 = format!("{:x}", Sha256::digest(&assets[0]));
        let error = run(home.path(), &pins, &assets).unwrap_err();
        assert!(matches!(error, InstallError::Archive { .. }), "{error}");
        if !truncate {
            assert!(error.to_string().contains("missing NOTICE"), "{error}");
        }
        assert_eq!(
            fs::read_dir(home.path().join("engines")).unwrap().count(),
            0
        );
    }
}

#[test]
fn an_oversized_member_is_refused_before_reading_its_body() {
    let (home, mut pins, mut assets) = fixture(None);
    let mut header = Header::new_ustar();
    header.set_path("greywall_fixture/greywall").unwrap();
    header.set_size(MAX_ARCHIVE + 1);
    header.set_mode(0o755);
    header.set_cksum();
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(header.as_bytes()).unwrap();
    assets[0] = gzip.finish().unwrap();
    pins[0].sha256 = format!("{:x}", Sha256::digest(&assets[0]));
    assert!(matches!(
        run(home.path(), &pins, &assets),
        Err(InstallError::Archive { .. })
    ));
}

#[test]
fn a_rerun_refuses_an_altered_installation_and_concurrent_installs_agree() {
    let (home, pins, assets) = fixture(None);
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| run(home.path(), &pins, &assets));
        let second = run(home.path(), &pins, &assets);
        (first.join().unwrap().unwrap(), second.unwrap())
    });
    assert_eq!(first.greywall, second.greywall);
    fs::write(&first.greywall, "tampered").unwrap();
    assert!(matches!(
        run(home.path(), &pins, &assets),
        Err(InstallError::ExistingInstallation(_))
    ));
    assert_eq!(fs::read_to_string(first.greywall).unwrap(), "tampered");
    assert_eq!(
        fs::read_dir(home.path().join("engines")).unwrap().count(),
        1
    );
}
