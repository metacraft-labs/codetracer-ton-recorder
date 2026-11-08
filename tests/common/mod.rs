//! Helpers shared by the integration-test crates in `tests/`.
//!
//! Each file directly under `tests/` is its own crate, so each pulls this
//! module in with `mod common;`.

use std::path::{Path, PathBuf};

/// Environment variable that turns a missing `ct-print` into a failure.
///
/// CI sets it (the `test` recipe in the `Justfile`, which the shared recorder
/// CI runs). Without it, a missing `ct-print` skips the ct-print-backed
/// assertions, and a skipped test passes, so the job would stay green while
/// asserting nothing. Local checkouts that lack the sibling leave it unset and
/// still skip, with a printed `SKIP:` line.
pub const REQUIRE_CT_PRINT_ENV: &str = "CODETRACER_TON_RECORDER_REQUIRE_CT_PRINT";

/// Path to the `ct-print` binary shipped with `codetracer-trace-format-nim`.
///
/// The TON recorder is CTFS-only; tests that need to make content-level
/// assertions on a recorded trace pipe the `.ct` container through `ct-print`
/// and assert on the resulting JSON.  This is the same workflow that
/// `Recorder-CLI-Conventions.md` §4 prescribes for downstream tools / golden
/// snapshots.
pub fn ct_print_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("codetracer-trace-format-nim")
        .join(format!("ct-print{}", std::env::consts::EXE_SUFFIX))
}

/// Whether [`REQUIRE_CT_PRINT_ENV`] is set to `1` in this run.
pub fn ct_print_required() -> bool {
    std::env::var_os(REQUIRE_CT_PRINT_ENV).is_some_and(|value| value == "1")
}

/// Checks for `ct_print` and reports when it is missing.
///
/// Returns `false` when the binary exists, and the test runs. When it is
/// missing and `required` is `false`, prints a `SKIP:` line and returns `true`,
/// so the caller returns early. When it is missing and `required` is `true`,
/// panics, so the test fails instead of passing as a skip.
pub fn ct_print_absent(test_name: &str, ct_print: &Path, required: bool) -> bool {
    if ct_print.exists() {
        return false;
    }
    let hint = format!(
        "ct-print not found at {} — build it with `nimble buildCtPrint` in the \
         codetracer-trace-format-nim sibling",
        ct_print.display()
    );
    if required {
        panic!("{test_name} requires {hint}; {REQUIRE_CT_PRINT_ENV}=1 forbids skipping");
    }
    eprintln!("SKIP: {test_name} requires {hint}");
    true
}
