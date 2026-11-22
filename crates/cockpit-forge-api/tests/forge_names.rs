//! No forge's name in the contract (#153): nothing above it may key on one, so nothing in it
//! names one.

use std::fs;
use std::path::Path;

const FORGE_NAMES: [&str; 6] = [
    "github",
    "gitlab",
    "forgejo",
    "gitea",
    "codeberg",
    "bitbucket",
];

fn sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("README.md"), root.join("Cargo.toml")];
    for entry in fs::read_dir(root.join("src")).unwrap() {
        files.push(entry.unwrap().path());
    }
    files
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path).unwrap();
            (path.display().to_string(), text.to_lowercase())
        })
        .collect()
}

#[test]
fn the_contract_names_no_forge() {
    let found: Vec<String> = sources()
        .iter()
        .flat_map(|(path, text)| {
            FORGE_NAMES
                .iter()
                .filter(|name| text.contains(*name))
                .map(move |name| format!("{path}: {name}"))
        })
        .collect();

    assert!(found.is_empty(), "{found:#?}");
}
