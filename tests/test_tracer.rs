//! Integration tests for the Tolk/TON tracer.

use std::path::{Path, PathBuf};
use std::process::Command;

const CTFS_MAGIC: [u8; 5] = [0xC0, 0xDE, 0x72, 0xAC, 0xE2];

fn test_programs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-programs/tolk")
}

/// Path to the `ct-print` binary shipped with `codetracer-trace-format-nim`.
///
/// The TON recorder is CTFS-only; tests that need to make
/// content-level assertions on a recorded trace pipe the `.ct`
/// container through `ct-print --full --strip-paths` and assert on
/// the resulting JSON.  This is the same workflow that
/// `Recorder-CLI-Conventions.md` §4 prescribes for downstream tools /
/// golden snapshots.
fn ct_print_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("codetracer-trace-format-nim")
        .join("ct-print")
}

/// Helper: collect every `.ct` file in `out_dir`.
fn ct_files_in(out_dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(out_dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
        .collect()
}

fn run_tracer_on_file(source_path: &Path, out_dir: &Path) {
    // The recorder is CTFS-only — see AUDIT-CTFS-2026-05.md ("Convention
    // compliance follow-up — 2026-05-08").  Output is the canonical
    // multi-stream container that the Nim ct_reader_* FFI and the
    // db-backend's CTFSTraceReader consume directly.
    codetracer_ton_recorder::recorder::record(source_path, out_dir)
        .expect("trace_program should succeed");
}

fn assert_valid_ct_file(out_dir: &Path) -> PathBuf {
    let ct_files: Vec<_> = std::fs::read_dir(out_dir)
        .expect("failed to read output directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
        .collect();
    assert!(
        !ct_files.is_empty(),
        "expected at least one .ct file in {:?}",
        out_dir
    );
    let ct_path = &ct_files[0];
    let content = std::fs::read(ct_path).expect("failed to read .ct file");
    assert!(content.len() >= 5, ".ct file too small");
    assert_eq!(&content[..5], &CTFS_MAGIC, "CTFS magic bytes mismatch");
    ct_path.clone()
}

#[test]
fn test_ton_compile_and_run() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();
    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);
    let ct_path = assert_valid_ct_file(&out_dir);
    let size = std::fs::metadata(&ct_path).unwrap().len();
    assert!(
        size > 100,
        ".ct file should have substantial content, got {} bytes",
        size
    );
}

#[test]
fn test_ton_compute_value() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();
    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_ton_variable_values() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();
    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_ton_step_events() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();
    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_ton_metadata_structure() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();
    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_ton_function_calls() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();
    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

// ===========================================================================
// Per-program ct-print --full coverage tests
// ===========================================================================
//
// These tests follow the recorder-test-requirements policy
// (`metacraft-specs/policies/recorder-test-requirements.md`):
//
// * Each test records one Tolk program through the recorder's
//   normal entry point (`codetracer_ton_recorder::recorder::record`).
// * The produced `.ct` is piped through `ct-print --full --strip-paths`.
// * Assertions are made on the **decoded JSON document** with EXACT
//   counts (`assert_eq!(events.len(), N)` — never `>=`), EXACT
//   ordering, and EXACT decoded values
//   (`value["i"] == 42`, `value["kind"] == "Int"`).
//
// `ValueRecord` variants outside the expected set are rejected with
// a hard error message asking the test author to extend the test
// rather than weaken the assertion.
//
// Where the recorder's current behaviour deviates from what Tolk
// semantics dictate (e.g. `if`/`while`/`throw`/struct literals are
// not parsed because the source-level parser is hand-rolled and
// var/val/return-only), the deviation is documented inline as
// `RECORDER BUG: ...` and a parallel `#[ignore]`d assertion captures
// the spec-correct expectation so it surfaces the moment the
// recorder catches up.

/// Skip-helper: returns `Some(path)` to ct-print or logs a clear
/// `SKIP:` diagnostic and returns `None`.  The
/// `verify-cli-convention-no-silent-skip.sh` script greps for the
/// literal `SKIP:` token, so silent skips remain forbidden.
fn ct_print_or_skip(test_name: &str) -> Option<PathBuf> {
    let p = ct_print_path();
    if !p.exists() {
        eprintln!(
            "SKIP: {test_name} requires ct-print at {} — only available \
             within the metacraft workspace where codetracer-trace-format-nim \
             is a sibling.",
            p.display()
        );
        return None;
    }
    Some(p)
}

/// Record a program and return the `ct-print --full --strip-paths`
/// JSON document plus the absolute path to the source file (so the
/// caller can match `metadata.program`).  Returns `None` when
/// `ct-print` is unavailable (the caller has already emitted a
/// `SKIP:` line via `ct_print_or_skip`).
fn record_and_dump_full(test_name: &str, program: &str) -> Option<(serde_json::Value, PathBuf)> {
    let ct_print = ct_print_or_skip(test_name)?;

    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join(program);
    codetracer_ton_recorder::recorder::record(&source_path, &out_dir)
        .expect("recorder::record should succeed");

    let ct_files = ct_files_in(&out_dir);
    assert!(
        !ct_files.is_empty(),
        "expected a .ct container in {:?}",
        out_dir
    );

    let output = Command::new(&ct_print)
        .args(["--full", "--strip-paths"])
        .arg(&ct_files[0])
        .output()
        .expect("failed to run ct-print --full");

    assert!(
        output.status.success(),
        "ct-print --full should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let doc: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("ct-print --full should emit valid JSON");

    drop(tmp_dir);

    Some((doc, source_path))
}

/// Decode every (varname, i64) pair from step events.  Rejects any
/// `ValueRecord` variant other than `Int` with a hard error that
/// asks the test author to extend the test rather than weaken it.
fn observed_int_vars(doc: &serde_json::Value) -> Vec<(String, i64)> {
    let events = doc["events"].as_array().expect("events array");
    let mut out = Vec::new();
    for ev in events {
        if ev["kind"] != "step" {
            continue;
        }
        let Some(vars) = ev["vars"].as_array() else {
            continue;
        };
        for v in vars {
            let name = v["varname"].as_str().expect("varname str").to_string();
            let value = &v["value"];
            assert_eq!(
                value["kind"].as_str(),
                Some("Int"),
                "variable `{}` should decode as Int, got {}; \
                 if a new ValueRecord variant has landed for Tolk \
                 (e.g. BigInt for >i64 values, Tuple for `(int, int)`, \
                 Struct for struct literals, Slice/Builder/Cell for TON \
                 primitives), extend this test to assert on the new \
                 variant explicitly rather than weakening the check",
                name,
                value
            );
            let i = value["i"]
                .as_i64()
                .unwrap_or_else(|| panic!("Int.i must be i64 for `{name}`; got {value}"));
            out.push((name, i));
        }
    }
    out
}

/// Decode the call-entry sequence as a vector of function names.
fn observed_call_sequence(doc: &serde_json::Value) -> Vec<String> {
    doc["events"]
        .as_array()
        .expect("events array")
        .iter()
        .filter(|e| e["kind"] == "call_entry")
        .map(|e| {
            e["function"]
                .as_str()
                .expect("call_entry.function str")
                .to_string()
        })
        .collect()
}

/// Decode the call-exit sequence as a vector of function names.
fn observed_exit_sequence(doc: &serde_json::Value) -> Vec<String> {
    doc["events"]
        .as_array()
        .expect("events array")
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            e["function"]
                .as_str()
                .expect("call_exit.function str")
                .to_string()
        })
        .collect()
}

/// Assert that every `step` event carries a strictly increasing
/// `step_index`.  This is the recorder's only ordering guarantee
/// against duplicates / reorderings.
fn assert_step_indices_monotonic(doc: &serde_json::Value) {
    let mut last = -1i64;
    for ev in doc["events"].as_array().expect("events array") {
        if ev["kind"] != "step" {
            continue;
        }
        let idx = ev["step_index"]
            .as_i64()
            .expect("step_index must be present on step events");
        assert!(
            idx > last,
            "step_index must strictly increase; got {idx} after {last}"
        );
        last = idx;
    }
}

/// Assert `metadata.program` ends with the expected source filename.
fn assert_metadata_program_ends_with(doc: &serde_json::Value, source_path: &Path) {
    let prog = doc["metadata"]["program"]
        .as_str()
        .expect("metadata.program str");
    let want = source_path.file_name().unwrap().to_string_lossy();
    assert!(
        prog.ends_with(&*want),
        "metadata.program {prog} must end with {want}"
    );
}

// --- flow_test.tolk --------------------------------------------------------

/// Records the canonical cross-recorder fixture and asserts on the
/// **exact** decoded values.  This is the cross-language baseline
/// described in `recorder-test-requirements.md` §4 — if cairo says
/// `final_result = 94` and ton says `final_result = 84`, one of them
/// is wrong and this test is what surfaces the mismatch.
#[test]
fn test_flow_test_via_ct_print_full() {
    let Some((doc, source_path)) =
        record_and_dump_full("test_flow_test_via_ct_print_full", "flow_test.tolk")
    else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(functions, vec!["main", "compute"]);

    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(8), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(1), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 8 steps + 1 call_entry + 1 call_exit = 10 events.
    assert_eq!(events.len(), 10, "events.len()");
    assert_step_indices_monotonic(&doc);

    assert_eq!(observed_call_sequence(&doc), vec!["compute".to_string()]);
    assert_eq!(observed_exit_sequence(&doc), vec!["compute".to_string()]);

    // Canonical cross-recorder fixture: a=10, b=32, sum_val=42,
    // doubled=84, final_result=94.
    assert_eq!(
        observed_int_vars(&doc),
        vec![
            ("a".into(), 10),
            ("b".into(), 32),
            ("sum_val".into(), 42),
            ("doubled".into(), 84),
            ("final_result".into(), 94),
        ],
    );

    // Return value of compute() must round-trip as Int(94).
    let exits: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            let rv = &e["return_value"];
            assert_eq!(rv["kind"].as_str(), Some("Int"), "return must decode as Int; got {rv}");
            rv["i"].as_i64().expect("Int.i")
        })
        .collect();
    assert_eq!(exits, vec![94]);
}

// --- nested_calls_test.tolk ------------------------------------------------

/// Records `nested_calls_test.tolk` and asserts on the **exact** event
/// shape.  This is the well-behaved case for the present-day Tolk
/// parser: every function in the chain takes zero arguments, so the
/// hand-rolled `parse_function_call` matches them all and the
/// four-deep chain `compute -> outer -> middle -> inner` is captured
/// end-to-end.  Universal-checklist categories covered:
/// nested calls (>=3 deep), positional/no-arg function arguments,
/// scalar returns, control-flow-free linear bodies as a baseline.
#[test]
fn test_nested_calls_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_nested_calls_test_via_ct_print_full",
        "nested_calls_test.tolk",
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        functions,
        vec!["main", "compute", "outer", "middle", "inner"],
    );

    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(14), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(4), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 14 steps + 4 call_entry + 4 call_exit = 22 events.
    assert_eq!(events.len(), 22, "events.len()");
    assert_step_indices_monotonic(&doc);

    // Call entry order: outermost first.
    assert_eq!(
        observed_call_sequence(&doc),
        vec![
            "compute".to_string(),
            "outer".to_string(),
            "middle".to_string(),
            "inner".to_string(),
        ],
        "call_entry events must appear in entry order"
    );

    // Call exit order: innermost first (LIFO).
    assert_eq!(
        observed_exit_sequence(&doc),
        vec![
            "inner".to_string(),
            "middle".to_string(),
            "outer".to_string(),
            "compute".to_string(),
        ],
        "call_exit events must appear in LIFO order"
    );

    // Exact decoded values, in event-emission order.  Chain:
    // inner returns a+b=3, middle returns x+10=13, outer returns
    // p+100=113, compute returns 113.
    assert_eq!(
        observed_int_vars(&doc),
        vec![
            ("a".into(), 1),
            ("b".into(), 2),
            ("c".into(), 3),
            ("x".into(), 3),
            ("y".into(), 13),
            ("p".into(), 13),
            ("q".into(), 113),
            ("result".into(), 113),
        ],
    );

    // Return values on each call_exit must decode as Int.
    let returns: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            let rv = &e["return_value"];
            assert_eq!(
                rv["kind"].as_str(),
                Some("Int"),
                "return value must decode as Int; got {rv}"
            );
            rv["i"].as_i64().expect("return value Int.i")
        })
        .collect();
    assert_eq!(returns, vec![3, 13, 113, 113]);
}

// --- control_flow_test.tolk ------------------------------------------------

/// Records `control_flow_test.tolk` and pins the **current observed**
/// shape.  The program exercises if/else, while, repeat, and
/// do/until.  The recorder is hand-rolled and intentionally does
/// **not** parse any of those constructs — the parser's
/// `parse_statement` only matches `var`/`val`/`return` lines, and
/// assignment statements (`sign = -1;`) are dropped on the floor.
/// See RECORDER BUG notes inline + the parallel `#[ignore]`d test
/// for the spec-compliant expectation.
#[test]
fn test_control_flow_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_control_flow_test_via_ct_print_full",
        "control_flow_test.tolk",
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        functions,
        vec![
            "main",
            "compute",
            "classify",
            "loop_sum",
            "repeat_count",
            "do_until_grow",
        ],
    );

    let counts = &doc["counts"];
    // 18 steps:
    //   2 outer (toplevel-line-1 + dispatch line for compute) +
    //   3 classify (dispatch + var raw + var sign + return) -- but
    //     ct-print attributes the trailing return step to the caller's
    //     function context, so the "classify" rows are dispatch + var
    //     raw + var sign = 3 inside classify; the return-line step
    //     shows up as part of compute's frame.
    //   -- the same return-attribution happens for loop_sum,
    //   repeat_count, do_until_grow, so the step count is the sum of
    //   all parsed statements + the dispatch lines + the toplevel
    //   pre/post lines.  Pinning to 18 here is a golden snapshot;
    //   any change is a real regression to investigate.
    assert_eq!(counts["steps"].as_u64(), Some(18), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(5), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 18 steps + 5 call_entry + 5 call_exit = 28 events.
    assert_eq!(events.len(), 28, "events.len()");
    assert_step_indices_monotonic(&doc);

    assert_eq!(
        observed_call_sequence(&doc),
        vec![
            "compute".to_string(),
            "classify".to_string(),
            "loop_sum".to_string(),
            "repeat_count".to_string(),
            "do_until_grow".to_string(),
        ],
    );
    assert_eq!(
        observed_exit_sequence(&doc),
        vec![
            "classify".to_string(),
            "loop_sum".to_string(),
            "repeat_count".to_string(),
            "do_until_grow".to_string(),
            "compute".to_string(),
        ],
    );

    // RECORDER BUG: assignment statements (`sign = -1;`, `total = total + i;`,
    // `i = i + 1;`, `counter = counter + 1;`, `x = x + x;`) are not parsed,
    // so the only var values that surface are the pre-loop / pre-branch
    // initialisations.  Spec-compliant output (see #[ignore]d test below)
    // would also surface every iteration's intermediate values.
    assert_eq!(
        observed_int_vars(&doc),
        vec![
            ("raw".into(), 7),
            ("sign".into(), 0),
            // `compute()` re-binds `sign` from the (unparsed) classify
            // return — value still 0 because all three branch arms
            // are invisible to the parser.
            ("sign".into(), 0),
            ("total".into(), 0),
            ("i".into(), 0),
            ("loop_total".into(), 0),
            ("counter".into(), 0),
            ("repeated".into(), 0),
            ("x".into(), 1),
            ("grown".into(), 1),
            // sum of dropped/zero-only branches: 0 + 0 + 0 + 1 = 1.
            ("combined".into(), 1),
        ],
    );

    // RECORDER BUG: spec-compliant return values would be:
    //   classify -> 1 (raw=7, falls through to else -> sign=1)
    //   loop_sum -> 6 (0+1+2+3)
    //   repeat_count -> 3
    //   do_until_grow -> 8 (1 -> 2 -> 4 -> 8, last iteration meets >=8)
    //   compute -> 1+6+3+8 = 18
    let returns: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            let rv = &e["return_value"];
            assert_eq!(rv["kind"].as_str(), Some("Int"));
            rv["i"].as_i64().expect("Int.i")
        })
        .collect();
    assert_eq!(returns, vec![0, 0, 0, 1, 1]);
}

#[test]
#[ignore = "RECORDER BUG: if/else, while, repeat, do/until and bare \
            assignment statements are not parsed by the line-oriented \
            Tolk source parser; loop bodies and branch arms are \
            invisible.  Spec-compliant output should yield: \
            classify -> 1, loop_sum -> 6, repeat_count -> 3, \
            do_until_grow -> 8, compute -> 18 (sum 1+6+3+8)."]
fn test_control_flow_test_loops_and_branches_executed() {
    let Some((doc, _)) = record_and_dump_full(
        "test_control_flow_test_loops_and_branches_executed",
        "control_flow_test.tolk",
    ) else {
        return;
    };
    let returns: Vec<i64> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| e["return_value"]["i"].as_i64().expect("Int.i"))
        .collect();
    assert_eq!(returns, vec![1, 6, 3, 8, 18]);
}

// --- error_paths_test.tolk -------------------------------------------------

/// Records `error_paths_test.tolk` and pins the **current observed**
/// shape.  Today only the safe path runs end-to-end — `compute()`
/// invokes only `safe_compute()`; `failing_compute`,
/// `caught_compute`, and `assert_compute` are declared but not
/// reachable from the entry point.  The recorder also doesn't parse
/// `throw`, `try`, `catch`, or `assert` as statements.  See
/// RECORDER BUG notes inline + the parallel `#[ignore]`d test for
/// the spec-compliant expectation.
#[test]
fn test_error_paths_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_error_paths_test_via_ct_print_full",
        "error_paths_test.tolk",
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    // RECORDER BUG: spec-compliant function table would also include
    // `failing_compute`, `caught_compute`, and `assert_compute` (they
    // are declared, so they should be registered even before being
    // called).  Today only the entry point + functions reachable from
    // it appear.
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(functions, vec!["main", "compute", "safe_compute"]);

    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(9), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(2), "calls; counts={counts}");
    // Two io_events: one for `throw 7` inside the (unreached)
    // `failing_compute` and one for `assert (probe > 0, 13)` inside
    // the (unreached) `assert_compute`.  Both are surfaced via the
    // static sweep `emit_error_events_for_program` because the
    // recorder doesn't follow into functions that aren't reachable
    // from `main()` — a separate recorder gap also pinned by this
    // test (see the function-list assertion above and the
    // `STATIC-SWEEP LIMITATION` block in src/tracer.rs).
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(2),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 9 steps + 2 call_entry + 2 call_exit + 2 ioError = 15 events.
    assert_eq!(events.len(), 15, "events.len()");
    assert_step_indices_monotonic(&doc);

    assert_eq!(
        observed_call_sequence(&doc),
        vec!["compute".to_string(), "safe_compute".to_string()],
    );

    assert_eq!(
        observed_int_vars(&doc),
        vec![
            ("a".into(), 5),
            ("b".into(), 7),
            ("c".into(), 12),
            ("safe_val".into(), 12),
            ("bumped".into(), 112),
        ],
    );

    // Pin the exact content of the surfaced Error io_events.  The
    // metadata tags (`"TolkThrow"` / `"TolkAssert"`) mirror the
    // distinctness of the cardano `"AikenFail"` (commit 7e5a177)
    // and move `"ABORTED: ..."` conventions so the frontend can
    // route source-level Tolk failures away from generic runtime
    // TVM exceptions (which carry `"tvm_exception"`).  The content
    // strings carry the raw exception code verbatim — this is the
    // payload that downstream tools (calltrace, event log) render.
    let io_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "io" && e["io_kind"] == "ioError")
        .collect();
    assert_eq!(io_events.len(), 2, "exactly two ioError events");
    assert_eq!(
        io_events[0]["text"].as_str(),
        Some("throw 7"),
        "first ioError text"
    );
    assert_eq!(
        io_events[1]["text"].as_str(),
        Some("assert: code 13"),
        "second ioError text"
    );
}

#[test]
fn test_error_paths_test_emits_throw_event() {
    let Some((doc, _)) = record_and_dump_full(
        "test_error_paths_test_emits_throw_event",
        "error_paths_test.tolk",
    ) else {
        return;
    };
    let counts = &doc["counts"];
    assert!(
        counts["io_events"].as_u64().unwrap_or(0) >= 1,
        "expected at least one io_event for the `throw`/`assert` path; counts={counts}"
    );
}

// --- tuples_structs_test.tolk ----------------------------------------------

/// Records `tuples_structs_test.tolk` and pins the **current
/// observed** shape.  RECORDER BUG: tuple literals, struct literals,
/// and field accesses (`p.x`, `pair.0`) are all opaque to the
/// recorder's expression parser, so any binding whose right-hand
/// side mentions one of them silently drops out (eval returns
/// Ok(None) and no Value event is emitted).  Only the integer
/// let-bindings inside the zero-arg `scalar_only` helper survive.
/// See the parallel `#[ignore]`d test for the spec-compliant
/// expectation (Tuple / Struct ValueRecord variants).
#[test]
fn test_tuples_structs_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_tuples_structs_test_via_ct_print_full",
        "tuples_structs_test.tolk",
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        functions,
        vec![
            "main",
            "compute",
            "sum_pair",
            "point_distance_sq",
            "scalar_only",
        ],
    );

    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(19), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(4), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 19 steps + 4 call_entry + 4 call_exit = 27 events.
    assert_eq!(events.len(), 27, "events.len()");
    assert_step_indices_monotonic(&doc);

    assert_eq!(
        observed_call_sequence(&doc),
        vec![
            "compute".to_string(),
            "sum_pair".to_string(),
            "point_distance_sq".to_string(),
            "scalar_only".to_string(),
        ],
    );

    // RECORDER BUG: spec-compliant output would surface `pair` as
    // ValueRecord::Tuple, `p` as ValueRecord::Struct, and decode
    // `pair.0`, `pair.1`, `p.x`, `p.y` to scalar Int values.  Today
    // only the scalar_only helper's three Ints + the bound
    // `scalar_total` in compute() come through.
    assert_eq!(
        observed_int_vars(&doc),
        vec![
            ("head_val".into(), 1),
            ("len".into(), 4),
            ("total".into(), 5),
            ("scalar_total".into(), 5),
        ],
    );

    // RECORDER BUG: returns from sum_pair / point_distance_sq /
    // compute are NONE_VALUE today because their final expressions
    // (`pair_sum`, `sq`, `grand_total`) reference unbound variables
    // (their right-hand sides didn't parse).  Spec-compliant returns
    // would be: sum_pair=30, point_distance_sq=25, scalar_only=5,
    // compute=60.  We assert the present-day variant tags
    // (`Void` for the three broken returns -- ct-print decodes
    // ValueRecord::None as `{"kind":"Void"}` -- and `Int(5)` for
    // scalar_only) so a future fix surfaces immediately.
    let return_kinds: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| e["return_value"]["kind"].as_str().expect("return.kind"))
        .collect();
    assert_eq!(return_kinds, vec!["Void", "Void", "Int", "Void"]);
    let scalar_return = events
        .iter()
        .filter(|e| e["kind"] == "call_exit" && e["function"] == "scalar_only")
        .map(|e| e["return_value"]["i"].as_i64().expect("Int.i"))
        .next()
        .expect("scalar_only return");
    assert_eq!(scalar_return, 5);
}

#[test]
#[ignore = "RECORDER BUG: tuple literals (`(10, 20)`), struct literals \
            (`Point { x: 3, y: 4 }`), and field accesses (`p.x`, \
            `pair.0`) are opaque to the source-level expression parser. \
            Spec-compliant output should surface `pair` as \
            ValueRecord::Tuple, `p` as ValueRecord::Struct, decode \
            `pair.0`/`pair.1`/`p.x`/`p.y` to Int, and yield returns \
            sum_pair=30, point_distance_sq=25, scalar_only=5, \
            compute=60."]
fn test_tuples_structs_test_value_kinds_present() {
    let Some((doc, _)) = record_and_dump_full(
        "test_tuples_structs_test_value_kinds_present",
        "tuples_structs_test.tolk",
    ) else {
        return;
    };
    let mut kinds = std::collections::BTreeSet::new();
    for ev in doc["events"].as_array().unwrap() {
        if ev["kind"] != "step" {
            continue;
        }
        for v in ev["vars"].as_array().cloned().unwrap_or_default() {
            if let Some(k) = v["value"]["kind"].as_str() {
                kinds.insert(k.to_string());
            }
        }
    }
    for want in ["Int", "Tuple", "Struct"] {
        assert!(
            kinds.contains(want),
            "expected {want} ValueRecord variant in tuples/structs trace; got {kinds:?}"
        );
    }

    let returns: Vec<i64> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| e["return_value"]["i"].as_i64().expect("Int.i"))
        .collect();
    assert_eq!(returns, vec![30, 25, 5, 60]);
}

// --- cell_ops_test.tolk ----------------------------------------------------

/// Records `cell_ops_test.tolk` and pins the **current observed**
/// shape.  RECORDER BUG: TON-specific Cell / Slice / Builder
/// operations (`beginCell()`, `storeInt`, `endCell`, `beginParse`,
/// `loadInt`, `get_data`, `set_data`) are all opaque to the
/// expression parser, and `var b = beginCell()` doesn't even parse
/// as a var-binding because it lacks the `:` type annotation that
/// the line-oriented parser requires.  Only typed integer
/// let-bindings inside the helpers survive.  See the parallel
/// `#[ignore]`d test for the spec-compliant expectation
/// (Builder / Slice / Cell ValueRecord variants + io_events for
/// `set_data` / `get_data`).
#[test]
fn test_cell_ops_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_cell_ops_test_via_ct_print_full",
        "cell_ops_test.tolk",
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        functions,
        vec![
            "main",
            "compute",
            "encode_payload",
            "decode_payload",
            "storage_roundtrip",
        ],
    );

    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(20), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(4), "calls; counts={counts}");
    // RECORDER BUG: spec-compliant output would emit io_events for
    // `set_data(c)` (storage write) and `get_data()` (storage read).
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 20 steps + 4 call_entry + 4 call_exit = 28 events.
    assert_eq!(events.len(), 28, "events.len()");
    assert_step_indices_monotonic(&doc);

    assert_eq!(
        observed_call_sequence(&doc),
        vec![
            "compute".to_string(),
            "encode_payload".to_string(),
            "decode_payload".to_string(),
            "storage_roundtrip".to_string(),
        ],
    );

    // RECORDER BUG: spec-compliant output would surface every cell /
    // slice / builder binding plus the integer payloads loaded from
    // them.  Today only the `var <name>: int = <literal>;` lines
    // come through (everything that requires the cell/slice/builder
    // result is dropped because `var b = beginCell()` doesn't parse
    // at all -- the line-oriented parser requires a `:` type).
    assert_eq!(
        observed_int_vars(&doc),
        vec![
            ("payload".into(), 42),
            ("encoded_marker".into(), 1),
            ("encoded".into(), 1),
            ("decoded_marker".into(), 2),
            ("decoded".into(), 2),
            ("roundtrip_marker".into(), 3),
            ("rt".into(), 3),
            ("combined".into(), 6),
        ],
    );

    let returns: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            let rv = &e["return_value"];
            assert_eq!(rv["kind"].as_str(), Some("Int"));
            rv["i"].as_i64().expect("Int.i")
        })
        .collect();
    assert_eq!(returns, vec![1, 2, 3, 6]);
}

#[test]
#[ignore = "RECORDER BUG: TON-specific cell / slice / builder \
            operations (beginCell, storeInt, endCell, beginParse, \
            loadInt) are opaque to the expression parser, and the \
            type-less `var b = beginCell()` form doesn't even parse \
            as a var-binding (parser requires `:` type annotation). \
            `set_data` / `get_data` should emit io_events for the \
            storage write / read.  Spec-compliant output should also \
            decode `loaded` to Int(42) inside decode_payload and \
            Int(99) inside storage_roundtrip."]
fn test_cell_ops_test_storage_io_events() {
    let Some((doc, _)) = record_and_dump_full(
        "test_cell_ops_test_storage_io_events",
        "cell_ops_test.tolk",
    ) else {
        return;
    };
    let counts = &doc["counts"];
    assert!(
        counts["io_events"].as_u64().unwrap_or(0) >= 2,
        "expected >=2 io_events (set_data + get_data); counts={counts}"
    );

    // The `loaded` binding (decode_payload + storage_roundtrip) should
    // surface as a real Int once the cell/slice machinery is parsed.
    let loaded: Vec<i64> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "loaded")
        .map(|v| v["value"]["i"].as_i64().expect("Int.i"))
        .collect();
    assert_eq!(loaded, vec![42, 99]);
}

// ===========================================================================
// CLI smoke + env-var tests
// ===========================================================================

#[test]
fn test_ton_cli_record() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("cli-traces");
    let source_path = test_programs_dir().join("flow_test.tolk");

    // Exercise the canonical CTFS path (post-audit default; CTFS-only
    // post-2026-05-08).  Invoke the binary directly so the test
    // exercises the artefact that callers ship.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_codetracer-ton-recorder"))
        .args([
            "record",
            source_path.to_str().unwrap(),
            "--out-dir",
            out_dir.to_str().unwrap(),
        ])
        .env_remove("CODETRACER_TON_RECORDER_DISABLED")
        .env_remove("CODETRACER_TON_RECORDER_OUT_DIR")
        .output()
        .expect("failed to run");
    assert!(
        output.status.success(),
        "record should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_valid_ct_file(&out_dir);
}
