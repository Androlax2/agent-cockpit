//! #833: a host whose kernel or container refuses the sandbox's namespaces gets a refusal that
//! names bubblewrap's error and how to allow user namespaces, never an unsandboxed fallback.
//! A fake `bwrap` stands in for one that cannot create them.
#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cockpit_sandbox::Backend;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_torlek-sandbox");

const BUBBLEWRAP_REFUSAL: &str = "bwrap: No permissions to create a new namespace";

/// A `bwrap` that reports its version and refuses every namespace, as a kernel or container
/// without unprivileged user namespaces makes the real one do.
fn refusing_bubblewrap(folder: &Path) {
    let program = folder.join("bwrap");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then echo 'bubblewrap 0.13.1'; exit 0; fi\n\
             echo '{BUBBLEWRAP_REFUSAL}' >&2\n\
             exit 1\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_host_refusing_user_namespaces_has_no_backend_and_is_told_how_to_allow_them() {
    let folder = tempfile::tempdir().unwrap();
    refusing_bubblewrap(folder.path());

    let backend = Backend::probe(Path::new(LAUNCHER), Some(folder.path().as_os_str()));

    let reason = backend
        .missing_reason()
        .unwrap_or_else(|| panic!("a backend from a refusing bwrap: {backend:?}"));
    assert!(reason.contains(BUBBLEWRAP_REFUSAL), "{reason}");
    assert!(reason.contains("user namespaces"), "{reason}");
    assert!(
        reason.contains("sudo sysctl -w")
            || reason.contains("/etc/apparmor.d/bwrap")
            || reason.contains("run Torlek on the host or in a full virtual machine"),
        "{reason}"
    );
}
