//! The packages in config/, which make install and make after-apply link into the home with stow.

mod common;
use common::sandbox::*;

use std::fs;
use std::path::{Path, PathBuf};

fn links_under(directory: &Path) -> Vec<PathBuf> {
    let mut links = Vec::new();
    for entry in fs::read_dir(directory).unwrap().flatten() {
        let path = entry.path();
        let file_type = entry.file_type().unwrap();
        if file_type.is_symlink() {
            links.push(path);
        } else if file_type.is_dir() {
            links.extend(links_under(&path));
        }
    }
    links
}

#[test]
fn every_link_in_a_package_is_relative_and_stays_in_its_package() {
    // stow refuses a package holding an absolute link ("source is an absolute symlink") and aborts
    // every package with it: the systemd .wants links made make install and each after-apply fail.
    let config = repo().join("config");
    let links = links_under(&config);
    assert!(!links.is_empty(), "no link found under {}", config.display());
    for link in links {
        let target = fs::read_link(&link).unwrap();
        assert!(target.is_relative(), "{} -> {}", link.display(), target.display());
        let package = config.join(link.strip_prefix(&config).unwrap().components().next().unwrap());
        let resolved = link.parent().unwrap().join(&target).canonicalize()
            .unwrap_or_else(|error| panic!("{} -> {}: {error}", link.display(), target.display()));
        assert!(resolved.starts_with(package.canonicalize().unwrap()), "{} leaves its package: {}", link.display(), resolved.display());
    }
}
