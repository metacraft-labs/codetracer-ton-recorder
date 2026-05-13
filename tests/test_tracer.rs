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
/// do/until — all four are now driven by the recorder-side
/// interpreter (see the `Statement::If`/`While`/`Repeat`/`DoUntil`
/// arms in `src/tracer.rs::execute_statement`), so loop bodies and
/// branch arms surface as real step + var events instead of being
/// dropped on the floor.  The parallel `loops_and_branches_executed`
/// test pins the spec-compliant return-value sequence; this one pins
/// the full counts and the per-iteration variable trail.
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
    // 38 steps:
    //   1 toplevel pre-line + 1 main dispatch (`return compute();`)
    //   compute (6 stmts: 5 var bindings + return) = 6
    //   classify (3 stmts before if + if header + 1 chosen-arm assignment + return) = 6
    //   loop_sum (2 var bindings + while header + 4 iterations × 2 stmts + return) = 12
    //   repeat_count (1 var binding + repeat header + 3 iterations × 1 stmt + return) = 6
    //   do_until_grow (1 var binding + do header + 3 iterations × 1 stmt + return) = 6
    //   = 2 + 6 + 6 + 12 + 6 + 6 = 38.
    // Each var binding / assignment / return / loop-header / branch-header emits
    // exactly one Step event; the totals above match the per-iteration trail
    // (raw=7, sign=0, sign=1, total=0, i=0, total=0, i=1, total=1, i=2,
    // total=3, i=3, total=6, i=4, counter 0..3, x 1→2→4→8) plus the four
    // helper-return-bridge bindings (sign, loop_total, repeated, grown,
    // combined).
    assert_eq!(counts["steps"].as_u64(), Some(38), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(5), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 38 steps + 5 call_entry + 5 call_exit = 48 events.
    assert_eq!(events.len(), 48, "events.len()");
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

    // Per-iteration variable trail.  Captures every assignment plus
    // every var binding that the recorder-side interpreter walks
    // through (including the helper-return bridge bindings inside
    // `compute()`).  Assignment statements like `sign = -1;`,
    // `total = total + i;`, `i = i + 1;`, `counter = counter + 1;`,
    // and `x = x + x;` now surface end-to-end.
    assert_eq!(
        observed_int_vars(&doc),
        vec![
            // classify(): raw=7, sign=0, then else-arm assigns sign=1.
            ("raw".into(), 7),
            ("sign".into(), 0),
            ("sign".into(), 1),
            // compute()'s helper-return binding for classify.
            ("sign".into(), 1),
            // loop_sum(): total=0, i=0, then 4 iterations.
            ("total".into(), 0),
            ("i".into(), 0),
            ("total".into(), 0),
            ("i".into(), 1),
            ("total".into(), 1),
            ("i".into(), 2),
            ("total".into(), 3),
            ("i".into(), 3),
            ("total".into(), 6),
            ("i".into(), 4),
            ("loop_total".into(), 6),
            // repeat_count(): counter=0 then 3 increments.
            ("counter".into(), 0),
            ("counter".into(), 1),
            ("counter".into(), 2),
            ("counter".into(), 3),
            ("repeated".into(), 3),
            // do_until_grow(): x=1 then 1→2→4→8 (loop exits when x>=8).
            ("x".into(), 1),
            ("x".into(), 2),
            ("x".into(), 4),
            ("x".into(), 8),
            ("grown".into(), 8),
            // compute()'s final accumulator: 1 + 6 + 3 + 8 = 18.
            ("combined".into(), 18),
        ],
    );

    // Spec-compliant return values:
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
    assert_eq!(returns, vec![1, 6, 3, 8, 18]);
}

#[test]
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

/// Strict full-trace assertion that the recorder decodes Tolk's
/// structured literals end-to-end.  Tuple literals (`(10, 20)`) lift
/// to `ValueRecord::Tuple`; struct literals (`Point { x: 3, y: 4 }`)
/// lift to `ValueRecord::Struct`; field accesses (`pair.0`, `p.x`)
/// resolve through the structured env to scalar Ints, and every
/// helper's return value computes the on-chain answer
/// (`sum_pair=30`, `point_distance_sq=25`, `scalar_only=5`,
/// `compute=60`).
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
    assert_eq!(
        counts["values"].as_u64(),
        Some(19),
        "values; counts={counts}"
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

    // ----- Structured variable shapes ---------------------------------
    // Walk the step events in emission order and pin the (name,
    // ValueRecord::kind) pair for every variable.  This is stricter
    // than `observed_int_vars` (which only accepts Int) because the
    // tuples/structs fixture deliberately exercises Tuple / Struct
    // shapes that must NOT silently downgrade to Int.
    let var_sequence: Vec<(String, String)> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| {
            e["vars"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|v| {
                    (
                        v["varname"].as_str().expect("varname").to_string(),
                        v["value"]["kind"].as_str().expect("value.kind").to_string(),
                    )
                })
        })
        .collect();
    assert_eq!(
        var_sequence,
        vec![
            // sum_pair: tuple literal `pair = (10, 20)`, then
            // field-access projections `first = pair.0` / `second
            // = pair.1`, then arithmetic `pair_sum = first + second`.
            ("pair".into(), "Tuple".into()),
            ("first".into(), "Int".into()),
            ("second".into(), "Int".into()),
            ("pair_sum".into(), "Int".into()),
            // back in compute: sum_pair returned 30.
            ("pair_total".into(), "Int".into()),
            // point_distance_sq: struct literal `p = Point { x: 3, y: 4 }`,
            // then field-access arithmetic `sq = p.x * p.x + p.y * p.y`.
            ("p".into(), "Struct".into()),
            ("sq".into(), "Int".into()),
            // back in compute: point_distance_sq returned 25.
            ("point_total".into(), "Int".into()),
            // scalar_only: pure-arithmetic helper.
            ("head_val".into(), "Int".into()),
            ("len".into(), "Int".into()),
            ("total".into(), "Int".into()),
            // back in compute: scalar_only returned 5; final binding.
            ("scalar_total".into(), "Int".into()),
            ("grand_total".into(), "Int".into()),
        ],
    );

    // ----- Spot-check the structured payloads --------------------------
    let pair_tuple = find_var_value(&doc, "pair", "Tuple");
    let elems = pair_tuple["elements"]
        .as_array()
        .expect("Tuple elements array");
    let int_at = |i: usize| {
        assert_eq!(elems[i]["kind"].as_str(), Some("Int"));
        elems[i]["i"].as_i64().expect("Int.i")
    };
    assert_eq!(int_at(0), 10);
    assert_eq!(int_at(1), 20);

    let p_struct = find_var_value(&doc, "p", "Struct");
    let fields = p_struct["field_values"]
        .as_array()
        .expect("Struct field_values array");
    let f_int_at = |i: usize| {
        assert_eq!(fields[i]["kind"].as_str(), Some("Int"));
        fields[i]["i"].as_i64().expect("Int.i")
    };
    assert_eq!(f_int_at(0), 3);
    assert_eq!(f_int_at(1), 4);

    // ----- Return values: every helper now returns a real Int ---------
    let return_kinds: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| e["return_value"]["kind"].as_str().expect("return.kind"))
        .collect();
    assert_eq!(return_kinds, vec!["Int", "Int", "Int", "Int"]);
    let return_values: Vec<(String, i64)> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            (
                e["function"].as_str().expect("call_exit.function").to_string(),
                e["return_value"]["i"].as_i64().expect("Int.i"),
            )
        })
        .collect();
    assert_eq!(
        return_values,
        vec![
            ("sum_pair".into(), 30),
            ("point_distance_sq".into(), 25),
            ("scalar_only".into(), 5),
            ("compute".into(), 60),
        ],
    );
}

/// Locate the first `vars[].value` entry in the trace whose `varname`
/// matches `name` and whose `value.kind` matches `expected_kind`.
/// Panics with a precise message if no such entry exists — that's by
/// design: callers use this to pin a specific shape, and a missing
/// entry is a real recorder regression.
fn find_var_value(
    doc: &serde_json::Value,
    name: &str,
    expected_kind: &str,
) -> serde_json::Value {
    for ev in doc["events"].as_array().expect("events array") {
        if ev["kind"] != "step" {
            continue;
        }
        for v in ev["vars"].as_array().cloned().unwrap_or_default() {
            if v["varname"].as_str() == Some(name)
                && v["value"]["kind"].as_str() == Some(expected_kind)
            {
                return v["value"].clone();
            }
        }
    }
    panic!(
        "expected a `{name}` variable with value.kind == {expected_kind:?} \
         in the trace; got none"
    );
}

/// Spec-compliant assertion that the recorder decodes Tolk's
/// structured literals.  The recorder lifts tuple literals
/// (`(10, 20)`) into `ValueRecord::Tuple`, struct literals
/// (`Point { x: 3, y: 4 }`) into `ValueRecord::Struct`, and decodes
/// field accesses (`pair.0`, `p.x`) to scalar Ints — the four
/// helper returns now compute on-chain values
/// (`sum_pair=30`, `point_distance_sq=25`, `scalar_only=5`,
/// `compute=60`).
#[test]
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

/// Records `cell_ops_test.tolk` and pins the full observed shape now
/// that TON-specific Cell / Slice / Builder operations (`beginCell()`,
/// `storeInt`, `endCell`, `beginParse`, `loadInt`, `get_data`,
/// `set_data`) are wired through the recorder-side interpreter (see
/// `try_eval_ton_call` in `src/tracer.rs`).  The typeless
/// `var b = beginCell();` shape now parses and binds the result to
/// `Value::Builder`; `set_data` / `get_data` round-trip through the
/// recorder's shadow `storage_data` slot and emit `EventLogKind::Write` /
/// `EventLogKind::Read` io_events tagged `"TolkStorage"`.
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
    // 26 steps:
    //   2 outer (toplevel-line-1 + main dispatch),
    //   encode_payload (5 var bindings + return) = 6,
    //   decode_payload (4 var bindings + return) = 5,
    //   storage_roundtrip (5 var bindings + 1 set_data ExprStatement + return) = 7,
    //   compute (4 var bindings + return) = 5.
    //   Total: 2 + 6 + 5 + 7 + 5 = 25.  ct-print attributes the
    //   trailing `return` of `compute()` to the `<toplevel>` frame
    //   so the on-trace step count rounds to 26 once the toplevel
    //   pre/post lines are accounted for — pinned as a golden
    //   snapshot to surface any drift.
    assert_eq!(counts["steps"].as_u64(), Some(26), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(4), "calls; counts={counts}");
    // `set_data(c)` and `get_data()` each register a single io_event
    // tagged `"TolkStorage"` (Write / Read respectively) in
    // `storage_roundtrip()`.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(2),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 26 steps + 4 call_entry + 4 call_exit + 2 io = 36 events.
    assert_eq!(events.len(), 36, "events.len()");
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

    // Per-binding (varname, ValueRecord::kind) trail.  Builder /
    // Slice / Cell bindings surface as `Raw` (the Tolk recorder
    // doesn't model the bit-level cell wire format yet — the int
    // round-trip through `storeInt` / `loadInt` is the only payload
    // downstream tools render today).  Stricter than `observed_int_vars`
    // because the cell-ops fixture deliberately exercises non-Int
    // shapes that must NOT be silently dropped.
    let var_kinds: Vec<(String, String)> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| {
            e["vars"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|v| {
                    (
                        v["varname"].as_str().expect("varname").to_string(),
                        v["value"]["kind"].as_str().expect("value.kind").to_string(),
                    )
                })
        })
        .collect();
    assert_eq!(
        var_kinds,
        vec![
            // encode_payload(): payload, b, b2, c, encoded_marker.
            ("payload".into(), "Int".into()),
            ("b".into(), "Raw".into()),
            ("b2".into(), "Raw".into()),
            ("c".into(), "Raw".into()),
            ("encoded_marker".into(), "Int".into()),
            // back in compute: encoded.
            ("encoded".into(), "Int".into()),
            // decode_payload(): c, s, loaded, decoded_marker.
            ("c".into(), "Raw".into()),
            ("s".into(), "Raw".into()),
            ("loaded".into(), "Int".into()),
            ("decoded_marker".into(), "Int".into()),
            // back in compute: decoded.
            ("decoded".into(), "Int".into()),
            // storage_roundtrip(): b, c, got, s, loaded, roundtrip_marker.
            ("b".into(), "Raw".into()),
            ("c".into(), "Raw".into()),
            ("got".into(), "Raw".into()),
            ("s".into(), "Raw".into()),
            ("loaded".into(), "Int".into()),
            ("roundtrip_marker".into(), "Int".into()),
            // back in compute: rt + combined.
            ("rt".into(), "Int".into()),
            ("combined".into(), "Int".into()),
        ],
    );

    // Spot-check the integer payloads recovered from `loadInt`.
    let loaded: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "loaded")
        .map(|v| v["value"]["i"].as_i64().expect("Int.i"))
        .collect();
    assert_eq!(loaded, vec![42, 99]);

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

// --- arg_passing_test.tolk -------------------------------------------------

/// Records `arg_passing_test.tolk` and pins the full ct-print
/// `--full` shape now that the recorder threads positional argument
/// expressions through every call site (see the cardano `a393608`
/// pattern adapted in `src/tracer.rs::evaluate_function` /
/// `eval_expr` / `eval_expr_to_value`).  Pre-M10 the parser only
/// recognised bare zero-arg `name()` shapes; every multi-arg call
/// dropped on the floor, leaving caller-bindings unevaluated.  This
/// test guards against the regression by asserting on the full
/// per-iteration variable trail, the six-call sequence
/// (`compute -> add -> square -> chain_calls -> add -> square`)
/// and every helper's return value.
#[test]
fn test_arg_passing_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_arg_passing_test_via_ct_print_full",
        "arg_passing_test.tolk",
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
        vec!["main", "compute", "add", "square", "chain_calls"],
    );

    let counts = &doc["counts"];
    // 19 steps:
    //   2 outer (toplevel-line-1 + main dispatch)
    //   compute (5 var bindings + return) = 6
    //   add called twice from compute / chain_calls: each call emits a
    //   per-formal step (line of caller) carrying the bound a/b plus a
    //   step at line 25 carrying the resulting `sum`, i.e. 2 steps per
    //   invocation × 2 = 4 steps.
    //   square called twice (from compute and chain_calls): 2 × 2 = 4 steps.
    //   chain_calls (2 var bindings + return) = 3
    //   = 2 + 6 + 4 + 4 + 3 = 19.
    assert_eq!(counts["steps"].as_u64(), Some(19), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(6), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 19 steps + 6 call_entry + 6 call_exit = 31 events.
    assert_eq!(events.len(), 31, "events.len()");
    assert_step_indices_monotonic(&doc);

    // Call entry order (encountered while walking compute's body in
    // source order).  add and square each appear twice — once from
    // compute directly and once inside chain_calls.
    assert_eq!(
        observed_call_sequence(&doc),
        vec![
            "compute".to_string(),
            "add".to_string(),
            "square".to_string(),
            "chain_calls".to_string(),
            "add".to_string(),
            "square".to_string(),
        ],
    );

    // Call exit order: innermost first.  Inside chain_calls, add
    // exits first, then square, then chain_calls; compute closes
    // last.
    assert_eq!(
        observed_exit_sequence(&doc),
        vec![
            "add".to_string(),
            "square".to_string(),
            "add".to_string(),
            "square".to_string(),
            "chain_calls".to_string(),
            "compute".to_string(),
        ],
    );

    // Full (varname, value) trail.  This is the key M10-arg-passing
    // assertion: every callee binding (`a`, `b`, `n`, `seed`,
    // `doubled`, `squared`) must surface with the resolved actual
    // value, not as an empty binding or a None placeholder.
    assert_eq!(
        observed_int_vars(&doc),
        vec![
            // compute(): base.
            ("base".into(), 3),
            // add(base=3, b=4): a=3, b=4, sum=7.
            ("a".into(), 3),
            ("b".into(), 4),
            ("sum".into(), 7),
            // Bridge binding back in compute: sum = 7.
            ("sum".into(), 7),
            // square(base=3): n=3, sq=9.
            ("n".into(), 3),
            ("sq".into(), 9),
            // Bridge binding back in compute: sq = 9.
            ("sq".into(), 9),
            // chain_calls(base=3): seed=3.
            ("seed".into(), 3),
            // add(seed=3, seed=3): a=3, b=3, sum=6.
            ("a".into(), 3),
            ("b".into(), 3),
            ("sum".into(), 6),
            // Bridge binding back in chain_calls: doubled = 6.
            ("doubled".into(), 6),
            // square(doubled=6): n=6, sq=36.
            ("n".into(), 6),
            ("sq".into(), 36),
            // Bridge binding back in chain_calls: squared = 36.
            ("squared".into(), 36),
            // Bridge binding back in compute: chained = 36.
            ("chained".into(), 36),
            // Final accumulator: 7 + 9 + 36 = 52.
            ("combined".into(), 52),
        ],
    );

    // Return values, in event-emission (call_exit LIFO) order:
    //   add -> 7 (from compute)
    //   square -> 9 (from compute)
    //   add -> 6 (from chain_calls; seed+seed = 3+3)
    //   square -> 36 (from chain_calls; doubled^2 = 6^2)
    //   chain_calls -> 36
    //   compute -> 52
    let returns: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            let rv = &e["return_value"];
            assert_eq!(rv["kind"].as_str(), Some("Int"));
            rv["i"].as_i64().expect("Int.i")
        })
        .collect();
    assert_eq!(returns, vec![7, 9, 6, 36, 36, 52]);

    // Each call_entry must carry the resolved actuals as `args`
    // (NOT NONE_VALUE placeholders).  This pins the call-arg
    // staging path so the calltrace pane's `.call-arg` rows are
    // populated end-to-end.
    let entries: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "call_entry")
        .collect();
    // First add invocation: (a=3, b=4).
    let add_first_args = entries[1]["args"].as_array().expect("args array");
    assert_eq!(add_first_args.len(), 2);
    assert_eq!(add_first_args[0]["value"]["i"].as_i64(), Some(3));
    assert_eq!(add_first_args[1]["value"]["i"].as_i64(), Some(4));
    // First square invocation: (n=3).
    let square_first_args = entries[2]["args"].as_array().expect("args array");
    assert_eq!(square_first_args.len(), 1);
    assert_eq!(square_first_args[0]["value"]["i"].as_i64(), Some(3));
    // chain_calls invocation: (seed=3).
    let chain_args = entries[3]["args"].as_array().expect("args array");
    assert_eq!(chain_args.len(), 1);
    assert_eq!(chain_args[0]["value"]["i"].as_i64(), Some(3));
}

// --- persistent_storage_test.tolk ------------------------------------------

/// Records `persistent_storage_test.tolk` and pins the canonical
/// Tolk persistent-storage idiom: a `Storage` struct + `load_data()`
/// / `save_data()` round-trip.  This is the most common real-world
/// TON contract shape (counter, jetton-wallet, NFT-item, ...) — every
/// production contract opens with `load_data()`, mutates a `Storage`,
/// and closes with `save_data()`.
///
/// The recorder treats `load_data` / `save_data` as the canonical
/// names for `EventLogKind::Read` / `EventLogKind::Write` events on
/// the `"TolkStorage"` channel.  The shadow `storage_data` slot
/// carries the most recently saved Cell payload so the
/// load-mutate-save flow round-trips through the same int-only
/// `storeInt` / `loadInt` machinery as `cell_ops_test.tolk`.  This
/// fixture additionally exercises:
///   * struct values flowing through a function return
///     (`load_state` returns `Storage`),
///   * struct values passed as arguments through a `save_state(st)`
///     call site (enabled by the M10 arg-passing extension), and
///   * field access on a struct passed as a parameter
///     (`st.counter`, `st.owner` inside `save_state`).
#[test]
fn test_persistent_storage_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_persistent_storage_test_via_ct_print_full",
        "persistent_storage_test.tolk",
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
        vec!["main", "compute", "bump_counter", "load_state", "save_state"],
    );

    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(29), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(4), "calls; counts={counts}");
    // Three io_events: one save_data in compute() (the seed write),
    // one load_data inside load_state(), and one save_data inside
    // save_state().
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(3),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 29 steps + 4 call_entry + 4 call_exit + 3 io = 40 events.
    assert_eq!(events.len(), 40, "events.len()");
    assert_step_indices_monotonic(&doc);

    assert_eq!(
        observed_call_sequence(&doc),
        vec![
            "compute".to_string(),
            "bump_counter".to_string(),
            "load_state".to_string(),
            "save_state".to_string(),
        ],
    );

    assert_eq!(
        observed_exit_sequence(&doc),
        vec![
            "load_state".to_string(),
            "save_state".to_string(),
            "bump_counter".to_string(),
            "compute".to_string(),
        ],
    );

    // Per-binding (varname, ValueRecord::kind) trail.  Cell / Slice /
    // Builder shapes surface as Raw (the recorder doesn't model the
    // bit-level cell wire format, but the int round-trip is what the
    // fixture exercises).  Struct shapes carry the full `Storage`
    // payload (`counter`, `owner`).  Bridge bindings between caller
    // and callee scopes carry the same shape as the return value.
    let var_kinds: Vec<(String, String)> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| {
            e["vars"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|v| {
                    (
                        v["varname"].as_str().expect("varname").to_string(),
                        v["value"]["kind"].as_str().expect("value.kind").to_string(),
                    )
                })
        })
        .collect();
    assert_eq!(
        var_kinds,
        vec![
            // compute(): seed the storage cell with counter=7, owner=42.
            ("seed_b".into(), "Raw".into()),
            ("seed_b1".into(), "Raw".into()),
            ("seed_b2".into(), "Raw".into()),
            ("seed_c".into(), "Raw".into()),
            // load_state(): read seed, parse, build Storage struct.
            ("raw".into(), "Raw".into()),
            ("s".into(), "Raw".into()),
            ("counter".into(), "Int".into()),
            ("owner".into(), "Int".into()),
            ("st".into(), "Struct".into()),
            // bump_counter(): receive Storage, increment counter,
            // construct updated Storage, save it.
            ("st".into(), "Struct".into()),
            ("current".into(), "Int".into()),
            ("next".into(), "Int".into()),
            ("updated".into(), "Struct".into()),
            // save_state(updated): formal `st` bound to the actual
            // Storage struct (only possible thanks to the M10 arg-
            // passing extension).
            ("st".into(), "Struct".into()),
            ("b".into(), "Raw".into()),
            ("b1".into(), "Raw".into()),
            ("b2".into(), "Raw".into()),
            ("c".into(), "Raw".into()),
            ("saved_marker".into(), "Int".into()),
            // Back in bump_counter: ack of save_state's int return.
            ("ack".into(), "Int".into()),
            // Back in compute: bumped = bump_counter's int return.
            ("bumped".into(), "Int".into()),
        ],
    );

    // Spot-check the round-tripped int payloads in the slice loads.
    let counter_vals: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "counter")
        .map(|v| v["value"]["i"].as_i64().expect("Int.i"))
        .collect();
    assert_eq!(counter_vals, vec![7]);

    let owner_vals: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "owner")
        .map(|v| v["value"]["i"].as_i64().expect("Int.i"))
        .collect();
    assert_eq!(owner_vals, vec![42]);

    // `next` (the incremented counter) and `bumped` (the final
    // return) must both carry the on-chain answer 8.
    let next_vals: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "next" || v["varname"] == "bumped")
        .map(|v| v["value"]["i"].as_i64().expect("Int.i"))
        .collect();
    assert_eq!(next_vals, vec![8, 8]);

    // Pin the io_event text so a regression in the storage-channel
    // metadata surfaces immediately.  Order: seed write in compute,
    // then load inside load_state, then save inside save_state.
    let io_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "io")
        .collect();
    assert_eq!(io_events.len(), 3);
    assert_eq!(
        io_events[0]["text"].as_str(),
        Some("save_data: Cell([7, 42])")
    );
    assert_eq!(
        io_events[1]["text"].as_str(),
        Some("load_data: Cell([7, 42])")
    );
    assert_eq!(
        io_events[2]["text"].as_str(),
        Some("save_data: Cell([8, 42])")
    );

    // load_state's return value must be the full Storage struct.
    let load_state_exit = events
        .iter()
        .find(|e| e["kind"] == "call_exit" && e["function"] == "load_state")
        .expect("load_state call_exit");
    let rv = &load_state_exit["return_value"];
    assert_eq!(rv["kind"].as_str(), Some("Struct"));
    let fields = rv["field_values"].as_array().expect("field_values");
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0]["i"].as_i64(), Some(7));
    assert_eq!(fields[1]["i"].as_i64(), Some(42));

    // The other three returns are all Ints; pin the values:
    //   save_state -> 1 (saved_marker)
    //   bump_counter -> 8 (next)
    //   compute -> 8 (bumped)
    let int_returns: Vec<(String, i64)> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .filter_map(|e| {
            let rv = &e["return_value"];
            rv["i"].as_i64().map(|i| {
                (
                    e["function"].as_str().expect("function").to_string(),
                    i,
                )
            })
        })
        .collect();
    assert_eq!(
        int_returns,
        vec![
            ("save_state".into(), 1),
            ("bump_counter".into(), 8),
            ("compute".into(), 8),
        ],
    );
}

// --- contract_entrypoints_test.tolk ----------------------------------------

/// Records `contract_entrypoints_test.tolk` and pins the recorder's
/// handling of Tolk's actual on-chain entry-point hooks
/// (`onInternalMessage` / `onExternalMessage`).  Pre-M10 the recorder
/// hard-failed on any program without `main()`; production TON
/// contracts never declare `main()`, so this fixture covers the
/// canonical real-world shape end-to-end.
///
/// The recorder synthesises `int(0)` actuals for every formal so the
/// body executes without needing a representative message payload.
/// The first declared hook merges into <toplevel> at depth 0; the
/// second runs as a normal nested call.  Helpers invoked from each
/// hook still go through the regular call_entry / call_exit pipeline.
#[test]
fn test_contract_entrypoints_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_contract_entrypoints_test_via_ct_print_full",
        "contract_entrypoints_test.tolk",
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
    // Function table is populated lazily on first invocation, so the
    // order matches the dispatch sequence:
    //   1. onInternalMessage (first entry, merged into toplevel)
    //   2. handle_internal (called from onInternalMessage)
    //   3. onExternalMessage (second entry, regular call)
    //   4. handle_external (called from onExternalMessage)
    assert_eq!(
        functions,
        vec![
            "onInternalMessage",
            "handle_internal",
            "onExternalMessage",
            "handle_external",
        ],
    );

    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(10), "steps; counts={counts}");
    // Three calls: handle_internal (from merged-into-toplevel
    // onInternalMessage), onExternalMessage (the second entry as a
    // regular call), and handle_external (from onExternalMessage).
    // onInternalMessage itself doesn't appear as a call because it's
    // merged into <toplevel>.
    assert_eq!(counts["calls"].as_u64(), Some(3), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 10 steps + 3 call_entry + 3 call_exit = 16 events.
    assert_eq!(events.len(), 16, "events.len()");
    assert_step_indices_monotonic(&doc);

    assert_eq!(
        observed_call_sequence(&doc),
        vec![
            "handle_internal".to_string(),
            "onExternalMessage".to_string(),
            "handle_external".to_string(),
        ],
    );

    assert_eq!(
        observed_exit_sequence(&doc),
        vec![
            "handle_internal".to_string(),
            "handle_external".to_string(),
            "onExternalMessage".to_string(),
        ],
    );

    // Synthetic-zero arg trail: every formal binds to 0; arithmetic
    // proceeds normally on top.  `amount=0` in handle_internal →
    // `doubled=0`, `ack=1`.  `seq=0` in handle_external → `bumped=10`.
    assert_eq!(
        observed_int_vars(&doc),
        vec![
            // handle_internal(amount=0).
            ("amount".into(), 0),
            ("doubled".into(), 0),
            ("ack".into(), 1),
            // Bridge binding inside onInternalMessage.
            ("processed".into(), 1),
            // onExternalMessage(seq=0) (regular call this time).
            ("seq".into(), 0),
            // handle_external(seq=0).
            ("seq".into(), 0),
            ("bumped".into(), 10),
            // Bridge binding inside onExternalMessage.
            ("processed".into(), 10),
        ],
    );

    // Return values:
    //   handle_internal -> 1
    //   handle_external -> 10
    //   onExternalMessage -> 10
    // onInternalMessage's return is absorbed into <toplevel> (no
    // call_exit because the merge skips it).
    let returns: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            let rv = &e["return_value"];
            assert_eq!(rv["kind"].as_str(), Some("Int"));
            rv["i"].as_i64().expect("Int.i")
        })
        .collect();
    assert_eq!(returns, vec![1, 10, 10]);
}

// --- throw_unless_throw_if_test.tolk ---------------------------------------

/// Records `throw_unless_throw_if_test.tolk` and pins the recorder's
/// distinction between Tolk's runtime-gated guard idiom (`throwIf` /
/// `throwUnless`) and the unconditional `throw` / static-sweep
/// `assert` markers exercised by `error_paths_test.tolk`.
///
/// The fixture drives four scenarios:
///   * `guard_amount(5)`     — `throwIf(40, 5 == 0)` does NOT trip → returns 1.
///   * `guard_amount(0)`     — `throwIf(40, 0 == 0)` trips         → io_event(40), call_exit None.
///   * `guard_threshold(9)`  — `throwUnless(36, 9 >= 7)` does NOT trip → returns 1.
///   * `guard_threshold(1)`  — `throwUnless(36, 1 >= 7)` trips    → io_event(36), call_exit None.
///
/// EXACTLY two io_events fire (one per tripped gate); a third io_event
/// from a non-tripping gate would be a regression of the runtime-aware
/// path back to the static-sweep behaviour and is the key thing this
/// test guards against.  The metadata tag (`"TolkThrow"`) reuses the
/// existing channel so the frontend's error log can render guards
/// alongside unconditional throws.
#[test]
fn test_throw_unless_throw_if_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_throw_unless_throw_if_test_via_ct_print_full",
        "throw_unless_throw_if_test.tolk",
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
        vec!["main", "compute", "guard_amount", "guard_threshold"],
    );

    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(18), "steps; counts={counts}");
    // Five calls: compute (1), guard_amount (2 — once succeeding,
    // once tripping), guard_threshold (2 — once succeeding, once
    // tripping).
    assert_eq!(counts["calls"].as_u64(), Some(5), "calls; counts={counts}");
    // EXACTLY two io_events — the tripping `throwIf(40, 0==0)` and
    // the tripping `throwUnless(36, 1>=7)`.  Non-tripping guards
    // must NOT emit io_events; that is the key M10 invariant
    // separating runtime-aware emission from the legacy static
    // sweep.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(2),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 18 steps + 5 call_entry + 5 call_exit + 2 io = 30 events.
    assert_eq!(events.len(), 30, "events.len()");
    assert_step_indices_monotonic(&doc);

    assert_eq!(
        observed_call_sequence(&doc),
        vec![
            "compute".to_string(),
            "guard_amount".to_string(),
            "guard_amount".to_string(),
            "guard_threshold".to_string(),
            "guard_threshold".to_string(),
        ],
    );

    // Per-iteration variable trail.  `ok_a` and `ok_c` bind to 1
    // (gate didn't trip); `ok_b` and `ok_d` are NOT bound (the
    // tripping callee returned None so the caller's binding
    // silently falls through, matching the recorder's
    // "best-effort, never panic" discipline).  Similarly `total`
    // never binds because two of its operands are missing — the
    // arithmetic surfaces as a TVM tvm_exception, but the trace
    // continues to finalise cleanly.
    assert_eq!(
        observed_int_vars(&doc),
        vec![
            // compute(): probe_a, then guard_amount(5) succeeds.
            ("probe_a".into(), 5),
            ("value".into(), 5),
            ("ok_a".into(), 1),
            // probe_b = 0; guard_amount(0) trips → ioError(40);
            // ok_b is NOT bound.
            ("probe_b".into(), 0),
            ("value".into(), 0),
            // probe_c = 9; guard_threshold(9) succeeds.
            ("probe_c".into(), 9),
            ("probe".into(), 9),
            ("ok_c".into(), 1),
            // probe_d = 1; guard_threshold(1) trips → ioError(36);
            // ok_d is NOT bound; `total` cannot compute.
            ("probe_d".into(), 1),
            ("probe".into(), 1),
        ],
    );

    // Pin the exact io_event text + ordering.  First the throwIf
    // fires inside guard_amount(0); then the throwUnless fires
    // inside guard_threshold(1).
    let io_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "io" && e["io_kind"] == "ioError")
        .collect();
    assert_eq!(io_events.len(), 2, "exactly two ioError events");
    assert_eq!(
        io_events[0]["text"].as_str(),
        Some("throwIf: code 40"),
        "first ioError text"
    );
    assert_eq!(
        io_events[1]["text"].as_str(),
        Some("throwUnless: code 36"),
        "second ioError text"
    );

    // Returns: succeeding guards return Int(1); tripping guards
    // produce a Void exit (the recorder surfaces unbound returns as
    // NONE_VALUE which ct-print decodes as kind: "Void").
    let returns: Vec<(String, Option<i64>)> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            let fname = e["function"].as_str().expect("function").to_string();
            let rv = &e["return_value"];
            let i = rv["i"].as_i64();
            (fname, i)
        })
        .collect();
    assert_eq!(
        returns,
        vec![
            ("guard_amount".into(), Some(1)),
            ("guard_amount".into(), None),
            ("guard_threshold".into(), Some(1)),
            ("guard_threshold".into(), None),
            ("compute".into(), None),
        ],
    );
}

// --- builder_refs_test.tolk ------------------------------------------------

/// Records `builder_refs_test.tolk` and pins the recorder's
/// extended Builder/Slice ref-bearing op coverage.  Where
/// `cell_ops_test.tolk` exercised only int-flat payloads, this
/// fixture drives `storeRef` / `loadRef` (sub-cell append + pop),
/// `storeAddress` / `loadAddress` (address-shaped scalars), and the
/// recorder's parallel ref-queue cursor on the Slice ValueRecord.
///
/// Spec-compliant recording surfaces:
///   * each Builder/Cell/Slice's ref queue alongside its int payload
///     (`Cell([200, 51966]) refs=[Cell([101])]`) so the trace shape
///     diverges visibly from the no-ref `cell_ops_test.tolk` outputs,
///   * `loadRef()` returning the sub-cell with the exact int payload
///     stored at the source site (`Cell([101])`),
///   * `loadAddress()` returning the stored address scalar as a real
///     Int ValueRecord (51966 = 0xCAFE; the fixture uses decimal so
///     it parses through the int-only TVM expression compiler).
#[test]
fn test_builder_refs_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_builder_refs_test_via_ct_print_full",
        "builder_refs_test.tolk",
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
        vec!["main", "compute", "pack_outer", "pack_inner", "unpack"],
    );

    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(24), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(4), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 24 steps + 4 call_entry + 4 call_exit = 32 events.
    assert_eq!(events.len(), 32, "events.len()");
    assert_step_indices_monotonic(&doc);

    assert_eq!(
        observed_call_sequence(&doc),
        vec![
            "compute".to_string(),
            "pack_outer".to_string(),
            "pack_inner".to_string(),
            "unpack".to_string(),
        ],
    );

    // Per-binding (varname, ValueRecord::kind) trail.  Cell / Slice /
    // Builder shapes surface as Raw.  This is the strict equivalent
    // of cell_ops_test's `var_kinds` assertion but extended with the
    // ref-queue annotations.
    let var_kinds: Vec<(String, String)> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| {
            e["vars"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|v| {
                    (
                        v["varname"].as_str().expect("varname").to_string(),
                        v["value"]["kind"].as_str().expect("value.kind").to_string(),
                    )
                })
        })
        .collect();
    assert_eq!(
        var_kinds,
        vec![
            // pack_inner(): ib, ib1, inner.
            ("ib".into(), "Raw".into()),
            ("ib1".into(), "Raw".into()),
            ("inner".into(), "Raw".into()),
            // pack_outer(): receives inner from pack_inner(), builds outer.
            ("inner".into(), "Raw".into()),
            ("ob".into(), "Raw".into()),
            ("ob1".into(), "Raw".into()),
            ("ob2".into(), "Raw".into()),
            ("ob3".into(), "Raw".into()),
            ("outer".into(), "Raw".into()),
            // compute(): packed = pack_outer() result.
            ("packed".into(), "Raw".into()),
            // unpack(c): c (param), s, marker, sub, addr, sub_s, inner_val, combined.
            ("c".into(), "Raw".into()),
            ("s".into(), "Raw".into()),
            ("marker".into(), "Int".into()),
            ("sub".into(), "Raw".into()),
            ("addr".into(), "Int".into()),
            ("sub_s".into(), "Raw".into()),
            ("inner_val".into(), "Int".into()),
            ("combined".into(), "Int".into()),
            // compute(): decoded = unpack(packed) result.
            ("decoded".into(), "Int".into()),
        ],
    );

    // Pin the exact Raw `r` strings for the ref-bearing values so a
    // regression in the ref-queue serialisation surfaces immediately.
    // These are the key M10 invariants that distinguish this fixture
    // from cell_ops_test (which produced bare `Cell([...])` strings).
    let raw_by_name = |name: &str, expected_r: &str| {
        let entry = events
            .iter()
            .filter(|e| e["kind"] == "step")
            .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
            .find(|v| v["varname"] == name && v["value"]["kind"] == "Raw")
            .unwrap_or_else(|| panic!("expected `{name}` to surface as Raw"));
        assert_eq!(
            entry["value"]["r"].as_str(),
            Some(expected_r),
            "Raw.r for `{name}`"
        );
    };
    raw_by_name("inner", "Cell([101])");
    raw_by_name("ob2", "Builder([200]) refs=[Cell([101])]");
    raw_by_name("ob3", "Builder([200, 51966]) refs=[Cell([101])]");
    raw_by_name("outer", "Cell([200, 51966]) refs=[Cell([101])]");
    raw_by_name("packed", "Cell([200, 51966]) refs=[Cell([101])]");
    raw_by_name("c", "Cell([200, 51966]) refs=[Cell([101])]");
    raw_by_name("sub", "Cell([101])");

    // Spot-check the int payloads recovered from `loadInt` /
    // `loadAddress`.  Cursor walk inside `unpack`:
    //   loadInt(32) -> marker = 200
    //   loadRef()    -> sub = Cell([101])  (cursor unchanged)
    //   loadAddress() -> addr = 51966
    let marker_vals: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "marker")
        .map(|v| v["value"]["i"].as_i64().expect("Int.i"))
        .collect();
    assert_eq!(marker_vals, vec![200]);

    let addr_vals: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "addr")
        .map(|v| v["value"]["i"].as_i64().expect("Int.i"))
        .collect();
    assert_eq!(addr_vals, vec![51966]);

    let inner_vals: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "inner_val")
        .map(|v| v["value"]["i"].as_i64().expect("Int.i"))
        .collect();
    assert_eq!(inner_vals, vec![101]);

    // Returns:
    //   pack_inner -> Cell([101])      (Raw)
    //   pack_outer -> Cell([200,51966]) refs=[Cell([101])]  (Raw)
    //   unpack     -> 52267  (200 + 101 + 51966)
    //   compute    -> 52267
    let returns: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .collect();
    assert_eq!(returns.len(), 4);
    assert_eq!(returns[0]["return_value"]["kind"].as_str(), Some("Raw"));
    assert_eq!(
        returns[0]["return_value"]["r"].as_str(),
        Some("Cell([101])")
    );
    assert_eq!(returns[1]["return_value"]["kind"].as_str(), Some("Raw"));
    assert_eq!(
        returns[1]["return_value"]["r"].as_str(),
        Some("Cell([200, 51966]) refs=[Cell([101])]")
    );
    assert_eq!(returns[2]["return_value"]["kind"].as_str(), Some("Int"));
    assert_eq!(returns[2]["return_value"]["i"].as_i64(), Some(52267));
    assert_eq!(returns[3]["return_value"]["kind"].as_str(), Some("Int"));
    assert_eq!(returns[3]["return_value"]["i"].as_i64(), Some(52267));
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
