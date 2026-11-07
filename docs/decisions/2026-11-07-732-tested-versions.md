# Tested harness versions are a list in a data file per adapter, not one pin

Issue #732. The owner agreed the proposal on 2026-11-04; the worker chose the file format.

- **Each adapter ships `tested-versions.txt`**: one version per line, `#` comments and blank
  lines allowed, read at compile time (`TESTED_VERSIONS`). A running version on the list is
  `Provenance::Tested`; any other is `Discovered`
  (`cockpit_provider_api::Provenance::for_version`). Several harness versions are tested at
  once, and the nightly conformance job can add a line without touching Rust.
  `parse_tested_versions` refuses an empty file, a line that is not a version and a
  duplicate; each adapter's `tests/tested_versions.rs` runs it on its own file.
- **`CONFORMANCE_VERSION` stays**, as the version the recorded fixtures and native evidence
  come from. It no longer decides provenance, and a test keeps it on the list.
- **Rejected:** a manifest fetched from the network. New tested versions ship with Torlek
  releases; the runtime probe covers the gap (#732).
- **Rejected:** a Rust array per adapter. A job editing a data file is simpler to review and
  to automate than one editing source.
