//! CLI-surface integration tests for `codetracer-ton-recorder`.
//!
//! Tests cover three areas:
//!
//! 1. **Smoke tests** — basic `--help`, `--version`, error paths.
//! 2. **`ct print` content** — record a fixture and pipe the resulting
//!    `.ct` container through `ct-print --json` from
//!    `codetracer-trace-format-nim` to make content-level assertions.
//!    Skips gracefully when `ct-print` is not present (i.e. when this
//!    crate is built outside the metacraft workspace).
//! 3. **CLI env-var contract** — exercise the post-2026-05-08
//!    `CODETRACER_TON_RECORDER_OUT_DIR` /
//!    `CODETRACER_TON_RECORDER_DISABLED` env vars and the
//!    no-`--format` invariant from `Recorder-CLI-Conventions.md` §4 / §5.
//!
//! History note: pre-2026-05-08 the recorder shipped a `--format
//! ctfs|binary|json` flag at three subcommand levels (`record`,
//! `trace-sandbox`, `replay`).  When the convention switched to
//! CTFS-only the `--format` argument was removed at every level and
//! the dedicated `ctfs_format_advertised_in_record_help` /
//! `ctfs_format_advertised_in_replay_and_sandbox_help` tests in
//! `tests/test_ctfs_audit.rs` were deleted (they asserted on the OLD
//! `--format` contract).  See `AUDIT-CTFS-2026-05.md` ("Convention
//! compliance follow-up — 2026-05-08") for the full record.

use std::path::PathBuf;
use std::process::Command;

fn cargo_bin() -> Command {
    // Invoke the pre-built recorder binary directly via the
    // `CARGO_BIN_EXE_<name>` path Cargo exposes to integration tests.
    //
    // The previous `cargo run --quiet --` form spawned a *nested* `cargo`
    // inside the `cargo test` process.  The nested invocation contends for
    // the build lock on `target/` that the outer `cargo test` already
    // holds; under that contention `cargo run` can exit non-zero before it
    // ever launches the recorder, which surfaced as an intermittent
    // `--help should succeed` failure (the lock-contention window is a
    // race, so only whichever CLI test ran first was affected).  The
    // direct-binary form has no nested cargo and no lock contention.
    Command::new(env!("CARGO_BIN_EXE_codetracer-ton-recorder"))
}

/// Path to the bundled Tolk fixture used across CLI tests.
fn flow_test_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-programs/tolk/flow_test.tolk")
}

/// Path to the `ct-print` binary shipped with `codetracer-trace-format-nim`.
///
/// The TON recorder is CTFS-only; tests that need to make content-level
/// assertions on a recorded trace pipe the `.ct` container through
/// `ct-print --json` and assert on the resulting JSON.  This is the
/// same workflow that `Recorder-CLI-Conventions.md` §4 prescribes for
/// downstream tools / golden snapshots.
fn ct_print_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("codetracer-trace-format-nim")
        .join(format!("ct-print{}", std::env::consts::EXE_SUFFIX))
}

// ===========================================================================
// Smoke tests
// ===========================================================================

#[test]
fn test_help_flag() {
    let output = cargo_bin().arg("--help").output().expect("failed to run");
    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("codetracer-ton-recorder"),
        "help output should mention the program name"
    );
}

#[test]
fn test_version_subcommand() {
    let output = cargo_bin().arg("version").output().expect("failed to run");
    assert!(output.status.success(), "version should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0.1.0"),
        "version output should contain the version number"
    );
}

#[test]
fn test_version_flag() {
    let output = cargo_bin()
        .arg("--version")
        .output()
        .expect("failed to run");
    assert!(output.status.success(), "--version should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0.1.0"),
        "version output should contain the version number"
    );
}

#[test]
fn test_record_nonexistent_file() {
    let output = cargo_bin()
        .args(["record", "nonexistent.tolk"])
        .output()
        .expect("failed to run");
    assert!(
        !output.status.success(),
        "record with nonexistent file should fail"
    );
}

// ===========================================================================
// CTFS content via `ct-print` — replaces the legacy `--format json` content
// assertions
// ===========================================================================

/// Record the bundled `flow_test.tolk` fixture, then convert the
/// produced `.ct` container to JSON via `ct-print` and assert on:
///
/// 1. **Structural anchors** (legacy layer): `ct-print --json` output
///    contains the source filename / variable names / `compute` somewhere
///    in the textual rendering.
/// 2. **Exact decoded values** (the layer enabled by `ct-print --full`):
///    the `flow_test.tolk` program executes `(10 + 32) * 2 + 10 = 94`
///    via the `compute()` function, with intermediate let-bindings
///    `a=10`, `b=32`, `sum_val=42`, `doubled=84`, `final_result=94`.
///    Each binding must surface in the trace as a step event with a
///    decoded `Int` ValueRecord whose `i` field matches the literal
///    value from the source program.  The `compute()` call's
///    `return_value` is also asserted to be `Int { i: 94 }`.
///
/// Pre-2026-05-08 the recorder shipped a `--format json` mode and a
/// `trace.json` file was written directly.  The convention now mandates
/// CTFS-only output; `ct print` is the canonical conversion tool.  See
/// `Recorder-CLI-Conventions.md` §4.  `ct-print --full` (added 2026-05
/// in `codetracer-trace-format-nim`) is what enables the exact-value
/// layer — its output is a deterministic JSON document with every CBOR
/// `ValueRecord` decoded to a structured form like
/// `{"kind":"Int","i":42,"type_id":N}`.
///
/// The TON recorder note about `Variable` integer payloads not
/// round-tripping through `ct-print --json` is empirically obsolete
/// for `--full`: the recorder's `register_variable_with_full_value`
/// path decodes back to `{"kind":"Int","i":<n>,"type_id":N}` with
/// values intact.  If a future Tolk backend emits a different
/// `ValueRecord` variant for integer let-bindings (e.g. `BigInt` for
/// 257-bit Tolk integers, or a tagged variant for the bool primitive),
/// the strict `Int`-kind assertion below fails loudly rather than
/// silently weakening to existence-only.
#[test]
fn test_recorded_trace_via_ct_print_json() {
    let ct_print = ct_print_path();
    if !ct_print.exists() {
        eprintln!(
            "SKIP: ct-print not found at {} — only available within the \
             metacraft workspace where codetracer-trace-format-nim is a sibling.",
            ct_print.display()
        );
        return;
    }

    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = flow_test_path();
    codetracer_ton_recorder::recorder::record(&source_path, &out_dir)
        .expect("recorder should succeed on flow_test.tolk");

    let ct_files: Vec<_> = std::fs::read_dir(&out_dir)
        .expect("failed to read output directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
        .collect();
    assert!(
        !ct_files.is_empty(),
        "expected at least one .ct file in {}",
        out_dir.display()
    );
    let ct_path = &ct_files[0];

    // -----------------------------------------------------------------
    // Layer 1 (legacy): ct-print --json — substring presence checks.
    // Kept as a safety net so a regression in the textual rendering
    // is caught even if --full's JSON shape evolves.
    // -----------------------------------------------------------------
    let output = Command::new(&ct_print)
        .args(["--json"])
        .arg(ct_path)
        .output()
        .expect("failed to run ct-print");

    assert!(
        output.status.success(),
        "ct-print --json should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout_json = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout_json.is_empty(),
        "ct-print --json produced empty output"
    );

    // Structural anchor 1: the fixture source path name appears in the
    // path stream rendered by ct-print.
    assert!(
        stdout_json.contains("flow_test.tolk"),
        "ct-print --json output should mention the fixture source path \
         (flow_test.tolk); got:\n{stdout_json}"
    );

    // Structural anchor 2: at least one of the Tolk variable / function
    // names from the canonical fixture appears.  `flow_test.tolk` declares
    // `a`, `b`, `sum_val`, `doubled`, `final_result` and the function
    // `compute`; we look for the longer/more distinctive names that are
    // unlikely to all rotate out at once.
    let variable_anchor = ["sum_val", "doubled", "final_result", "compute"]
        .iter()
        .any(|v| stdout_json.contains(v));
    assert!(
        variable_anchor,
        "ct-print --json output should mention at least one of the \
         Tolk variable / function names \
         (sum_val/doubled/final_result/compute); got:\n{stdout_json}"
    );

    // -----------------------------------------------------------------
    // Layer 2 (the upgrade): ct-print --full — exact decoded values.
    // -----------------------------------------------------------------
    let full_output = Command::new(&ct_print)
        .args(["--full", "--strip-paths"])
        .arg(ct_path)
        .output()
        .expect("failed to run ct-print --full");

    assert!(
        full_output.status.success(),
        "ct-print --full should succeed; stderr: {}",
        String::from_utf8_lossy(&full_output.stderr)
    );

    let doc: serde_json::Value = serde_json::from_slice(&full_output.stdout)
        .expect("ct-print --full should emit valid JSON");

    // ----- Function table: `compute` and `main` must both appear ------
    // The TON recorder currently registers function names as bare
    // identifiers (no module qualifier), but downstream language
    // backends may add one (e.g. `flow_test::compute`), so we use
    // `ends_with` to stay platform-agnostic.
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        functions.iter().any(|f| f.ends_with("compute")),
        "expected `compute` in functions table; got {:?}",
        functions
    );
    assert!(
        functions.iter().any(|f| f.ends_with("main")),
        "expected `main` in functions table; got {:?}",
        functions
    );

    // ----- Path table: the canonical fixture path must appear ---------
    let paths: Vec<&str> = doc["paths"]
        .as_array()
        .expect("paths array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("flow_test.tolk")),
        "expected flow_test.tolk in paths table; got {:?}",
        paths
    );

    // ----- Step / call counts ----------------------------------------
    // The TON recorder evaluates `compute()` directly (the `main()`
    // function is registered but the recorder doesn't trace its body —
    // only the `compute()` call inside it), emitting one `call_entry`
    // for `compute` and 8 step events (entry/dispatch + five
    // let-bindings + final-expression line + the post-call
    // return-site step).  Stable properties of the canonical fixture —
    // if they change, that's a real regression to investigate, not a
    // flake.
    //
    // The second call is `<toplevel>`, the root of the call tree: every
    // recording opens with it because the writer's `start(path, line)`
    // registers a `<toplevel>` function and opens its frame at depth 0
    // before any recorder-emitted event — see `trace-events.md`
    // §"Recorder Integration — Starting a Recording".  It contributes a
    // call but no step, so the step count is unaffected by it.
    let counts = &doc["counts"];
    assert_eq!(
        counts["steps"].as_u64(),
        Some(8),
        "expected 8 step events for flow_test.tolk; counts={counts}",
    );
    assert_eq!(
        counts["calls"].as_u64(),
        Some(2),
        "expected 2 call events (<toplevel> + compute); counts={counts}",
    );

    let events = doc["events"].as_array().expect("events array");

    // ----- Call sequence: <toplevel> then compute ---------------------
    let call_sequence: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "call_entry")
        .filter_map(|e| e["function"].as_str())
        .collect();
    assert_eq!(
        call_sequence.len(),
        2,
        "expected exactly 2 call_entry events; got {:?}",
        call_sequence
    );
    assert_eq!(
        call_sequence[0], "<toplevel>",
        "expected the root call registered by `start` first; got {:?}",
        call_sequence
    );
    assert!(
        call_sequence[1].ends_with("compute"),
        "expected the second call to be `compute`; got {:?}",
        call_sequence
    );

    // ----- Exact decoded variable values ------------------------------
    // Collect every (varname, i64) pair surfaced by step events.  These
    // come from the recorder writing `ValueRecord::Int` CBOR blobs, then
    // ct-print --full decoding them back to `{"kind":"Int","i":<n>,...}`.
    let observed_vars: Vec<(String, i64)> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| {
            e["vars"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
        })
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            let value = &v["value"];
            // The TON recorder encodes integer let-bindings as
            // ValueRecord::Int.  If something else surfaces (e.g.
            // BigInt for 257-bit Tolk integers, or a tagged variant
            // for Tolk's bool primitive), fail loudly so the test
            // author can decide whether to extend the assertions or
            // accept the new variant.
            assert_eq!(
                value["kind"].as_str(),
                Some("Int"),
                "variable `{}` should decode as Int, got {}; \
                 if a new ValueRecord variant has landed for Tolk \
                 integers, extend this test to assert on it explicitly \
                 rather than weakening the check",
                name,
                value
            );
            let i = value["i"]
                .as_i64()
                .unwrap_or_else(|| panic!("Int.i must be i64 for `{name}`; got {value}"));
            Some((name, i))
        })
        .collect();

    // The canonical flow: a=10, b=32, sum_val=a+b=42, doubled=sum_val*2=84,
    // final_result=doubled+a=94.  Same canonical fixture as cairo,
    // cardano, leo, and the other recorders — if your recorder runs
    // flow_test.* and these five let-bindings don't surface, that's the
    // bug to chase.
    let expected: &[(&str, i64)] = &[
        ("a", 10),
        ("b", 32),
        ("sum_val", 42),
        ("doubled", 84),
        ("final_result", 94),
    ];
    for (name, value) in expected {
        assert!(
            observed_vars.iter().any(|(n, v)| n == name && v == value),
            "expected step variable `{name}` = {value} in --full output; \
             observed = {observed_vars:?}"
        );
    }

    // ----- Call exit return value: compute() returns 94 ---------------
    // The Tolk source program's `compute()` returns `final_result`,
    // which is 94.  ct-print --full surfaces this on the `call_exit`
    // event for the matching `call_key`.  If the recorder ever stops
    // emitting return values (or starts emitting them with a different
    // ValueRecord variant), we want to know loudly.
    let return_values: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| {
            e["kind"] == "call_exit"
                && e["function"]
                    .as_str()
                    .is_some_and(|f| f.ends_with("compute"))
        })
        .map(|e| &e["return_value"])
        .collect();
    assert_eq!(
        return_values.len(),
        1,
        "expected exactly 1 call_exit for `compute`; got {:?}",
        return_values
    );
    assert_eq!(
        return_values[0]["kind"].as_str(),
        Some("Int"),
        "compute() return_value should decode as Int; got {}",
        return_values[0]
    );
    assert_eq!(
        return_values[0]["i"].as_i64(),
        Some(94),
        "compute() should return 94; got {}",
        return_values[0]
    );
}

// ===========================================================================
// CLI env-var contract
// ===========================================================================

/// `CODETRACER_TON_RECORDER_OUT_DIR` must be honoured as a fallback
/// for `--out-dir`.  Convention: `Recorder-CLI-Conventions.md` §5.
#[test]
fn test_env_out_dir_used_when_flag_omitted() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let env_out_dir = tmp_dir.path().join("via-env");

    let source_path = flow_test_path();

    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-ton-recorder"))
        .args(["record"])
        .arg(&source_path)
        .env("CODETRACER_TON_RECORDER_OUT_DIR", &env_out_dir)
        // Make sure the env-var doesn't bleed in from the developer's shell.
        .env_remove("CODETRACER_TON_RECORDER_DISABLED")
        .output()
        .expect("failed to run recorder");

    assert!(
        output.status.success(),
        "recorder should succeed when CODETRACER_TON_RECORDER_OUT_DIR is set; \
         stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The env-var-supplied output dir must contain the .ct bundle.
    let ct_files: Vec<_> = std::fs::read_dir(&env_out_dir)
        .unwrap_or_else(|e| {
            panic!(
                "expected env-supplied out-dir {:?} to exist after record: {e}",
                env_out_dir
            )
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
        .collect();
    assert!(
        !ct_files.is_empty(),
        "expected the env-supplied output dir {:?} to receive the .ct trace bundle",
        env_out_dir
    );
}

/// `CODETRACER_TON_RECORDER_DISABLED=1` must skip recording entirely.
/// The recorder process should still exit 0 (the TON recorder doesn't
/// run a separate target subprocess — it parses & evaluates the Tolk
/// source itself — so "disabled" simply means "don't write any trace
/// artefacts").
#[test]
fn test_env_disabled_skips_recording() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp_dir.path().join("should-stay-empty");

    let source_path = flow_test_path();

    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-ton-recorder"))
        .args(["record"])
        .arg(&source_path)
        .args(["--out-dir"])
        .arg(&out_dir)
        .env("CODETRACER_TON_RECORDER_DISABLED", "1")
        .output()
        .expect("failed to run recorder");

    assert!(
        output.status.success(),
        "recorder should succeed in disabled mode; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // No trace artefacts of any kind should have been written.
    let no_artefacts = !out_dir.exists()
        || (std::fs::read_dir(&out_dir)
            .map(|rd| rd.filter_map(|e| e.ok()).next().is_none())
            .unwrap_or(true));
    assert!(
        no_artefacts,
        "no trace artefacts should be written when \
         CODETRACER_TON_RECORDER_DISABLED=1; got files in {:?}",
        out_dir
    );
}

/// `--format` is no longer accepted at any level — clap must reject it.
/// Convention: §4 (CTFS-only).  Pre-2026-05-08 the flag existed at all
/// three subcommand levels (`record`, `trace-sandbox`, `replay`); we
/// exercise each here so a partial regression is caught.
#[test]
fn test_format_flag_rejected_by_clap() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = flow_test_path();

    // record --format json
    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-ton-recorder"))
        .args(["record"])
        .arg(&source_path)
        .args(["--out-dir"])
        .arg(&out_dir)
        .args(["--format", "json"])
        .output()
        .expect("failed to run recorder");

    assert!(
        !output.status.success(),
        "--format should be rejected by clap on `record`; stdout: {}, stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--format")
            || stderr.contains("unexpected argument")
            || stderr.contains("unrecognized")
            || stderr.contains("found argument"),
        "clap error should mention the unknown --format flag on `record`; \
         got stderr:\n{stderr}"
    );

    // trace-sandbox --format json (clap should reject the flag before the
    // vm-log path even has to exist).
    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-ton-recorder"))
        .args(["trace-sandbox"])
        .args(["--vm-log", "/dev/null"])
        .args(["--format", "json"])
        .output()
        .expect("failed to run recorder");

    assert!(
        !output.status.success(),
        "--format should be rejected by clap on `trace-sandbox`"
    );

    // replay --format json
    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-ton-recorder"))
        .args(["replay"])
        .args(["--tx-hash", "deadbeef"])
        .args(["--address", "EQDtest"])
        .args(["--format", "json"])
        .output()
        .expect("failed to run recorder");

    assert!(
        !output.status.success(),
        "--format should be rejected by clap on `replay`"
    );
}

/// The CLI binary must not expose a `--format` flag at any level.
/// Convention: `Recorder-CLI-Conventions.md` §4 — recorders are
/// CTFS-only.
#[test]
fn test_no_format_flag_in_help() {
    let bin = env!("CARGO_BIN_EXE_codetracer-ton-recorder");

    for subcmd in [None, Some("record"), Some("trace-sandbox"), Some("replay")] {
        let mut cmd = Command::new(bin);
        if let Some(s) = subcmd {
            cmd.arg(s);
        }
        cmd.arg("--help");

        let output = cmd.output().expect("failed to run --help");
        assert!(
            output.status.success(),
            "--help (subcmd={:?}) should exit 0",
            subcmd
        );

        let help = String::from_utf8_lossy(&output.stdout);
        assert!(
            !help.contains("--format"),
            "--help (subcmd={:?}) must not advertise --format; got:\n{help}",
            subcmd
        );
        assert!(
            !help.contains("CODETRACER_FORMAT"),
            "--help (subcmd={:?}) must not advertise CODETRACER_FORMAT; got:\n{help}",
            subcmd
        );
    }
}

/// `--help` must mention `ct print` so users know where to go for
/// human-readable conversion of the recorded CTFS bundle.
#[test]
fn test_help_mentions_ct_print() {
    let bin = env!("CARGO_BIN_EXE_codetracer-ton-recorder");
    let output = Command::new(bin)
        .arg("--help")
        .output()
        .expect("failed to run --help");
    assert!(output.status.success(), "--help should exit 0");

    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("ct print"),
        "--help must mention `ct print` as the conversion tool; got:\n{help}"
    );
}
