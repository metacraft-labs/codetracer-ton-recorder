//! Integration tests for the Tolk/TON tracer.
//!
//! These tests parse and evaluate real Tolk program files through the
//! source-level evaluator and verify the resulting CodeTracer trace output.
//!
//! Tests verify actual trace content with specific computed values,
//! not just file existence or non-emptiness.

use std::path::{Path, PathBuf};

use codetracer_trace_writer::TraceEventsFileFormat;

/// Helper: path to the test-programs directory.
fn test_programs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-programs/tolk")
}

/// Helper: run the tracer on a Tolk source file and return the output directory.
fn run_tracer_on_file(source_path: &Path, out_dir: &Path) {
    codetracer_ton_recorder::recorder::record(
        source_path,
        out_dir,
        TraceEventsFileFormat::Json,
    )
    .expect("trace_program should succeed");
}

/// Helper: parse the trace events JSON from the output directory.
fn load_trace_events(out_dir: &Path) -> Vec<serde_json::Value> {
    let events_path = out_dir.join("trace.bin");
    let content = std::fs::read_to_string(&events_path).expect("failed to read trace events");
    let events: serde_json::Value =
        serde_json::from_str(&content).expect("trace events should be valid JSON");
    events
        .as_array()
        .expect("events should be an array")
        .clone()
}

/// Helper: parse trace_metadata.json from the output directory.
fn load_trace_metadata(out_dir: &Path) -> serde_json::Value {
    let metadata_path = out_dir.join("trace_metadata.json");
    let content =
        std::fs::read_to_string(&metadata_path).expect("failed to read trace_metadata.json");
    serde_json::from_str(&content).expect("trace_metadata.json should be valid JSON")
}

/// Helper: collect all Int values from Value events in the trace.
/// Returns a vec of (variable_id, i64_value) pairs.
fn collect_int_values(events: &[serde_json::Value]) -> Vec<(i64, i64)> {
    events
        .iter()
        .filter_map(|e| {
            let val = e.get("Value")?;
            let variable_id = val.get("variable_id")?.as_i64()?;
            let value = val.get("value")?;
            if value.get("kind").and_then(|k| k.as_str()) == Some("Int") {
                let i = value.get("i").and_then(|v| v.as_i64())?;
                Some((variable_id, i))
            } else {
                None
            }
        })
        .collect()
}

/// Helper: collect all VariableName events and return the names in order.
fn collect_variable_names(events: &[serde_json::Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| {
            e.get("VariableName")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

/// Helper: find all Int values for a given variable name across the trace.
fn find_variable_values(events: &[serde_json::Value], var_name: &str) -> Vec<i64> {
    let var_names = collect_variable_names(events);
    let var_id = var_names.iter().position(|name| name == var_name);

    match var_id {
        Some(id) => {
            let int_values = collect_int_values(events);
            int_values
                .iter()
                .filter(|(vid, _)| *vid == id as i64)
                .map(|(_, v)| *v)
                .collect()
        }
        None => vec![],
    }
}

// ---------------------------------------------------------------------------
// Test 1: Record flow_test.tolk, verify 3-file output
// ---------------------------------------------------------------------------

#[test]
fn test_tolk_compile_and_run() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);

    // Verify the three output files exist and are non-empty.
    for filename in &["trace.bin", "trace_metadata.json", "trace_paths.json"] {
        let path = out_dir.join(filename);
        assert!(path.exists(), "{} should exist", filename);
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 0, "{} should be non-empty", filename);
    }

    // trace.bin should be valid JSON containing an array of events.
    let events = load_trace_events(&out_dir);
    assert!(!events.is_empty(), "trace should have at least one event");

    // There should be Step events (actual execution was recorded).
    let step_count = events.iter().filter(|e| e.get("Step").is_some()).count();
    assert!(
        step_count > 0,
        "trace should contain at least one Step event, got none"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Verify trace contains value 94 (final_result = doubled + a)
// ---------------------------------------------------------------------------

#[test]
fn test_tolk_compute_value() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    // Collect all integer values from the trace.
    let int_values = collect_int_values(&events);
    let all_values: Vec<i64> = int_values.iter().map(|(_, v)| *v).collect();

    // The program calculates: a=10, b=32, sum_val=a+b=42, doubled=sum_val*2=84,
    // final_result=doubled+a=94. This value should appear in the trace.
    assert!(
        all_values.contains(&94),
        "trace should contain value 94 (final_result = doubled + a = 84 + 10), got values: {:?}",
        {
            let mut unique: Vec<i64> = all_values.clone();
            unique.sort();
            unique.dedup();
            unique
        }
    );
}

// ---------------------------------------------------------------------------
// Test 3: Verify variable values a=10, b=32, sum_val=42, doubled=84,
//         final_result=94
// ---------------------------------------------------------------------------

#[test]
fn test_tolk_variable_values() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    // Check that VariableName events exist.
    let var_names = collect_variable_names(&events);
    assert!(
        !var_names.is_empty(),
        "trace should contain VariableName events"
    );

    // Verify specific variable names appear.
    assert!(
        var_names.contains(&"a".to_string()),
        "variable 'a' should appear in trace, got names: {:?}",
        var_names
    );
    assert!(
        var_names.contains(&"b".to_string()),
        "variable 'b' should appear in trace, got names: {:?}",
        var_names
    );
    assert!(
        var_names.contains(&"sum_val".to_string()),
        "variable 'sum_val' should appear in trace, got names: {:?}",
        var_names
    );
    assert!(
        var_names.contains(&"doubled".to_string()),
        "variable 'doubled' should appear in trace, got names: {:?}",
        var_names
    );
    assert!(
        var_names.contains(&"final_result".to_string()),
        "variable 'final_result' should appear in trace, got names: {:?}",
        var_names
    );

    // Verify variable values.
    let a_values = find_variable_values(&events, "a");
    assert!(
        a_values.contains(&10),
        "variable 'a' should have value 10, got: {:?}",
        a_values
    );

    let b_values = find_variable_values(&events, "b");
    assert!(
        b_values.contains(&32),
        "variable 'b' should have value 32, got: {:?}",
        b_values
    );

    let sum_values = find_variable_values(&events, "sum_val");
    assert!(
        sum_values.contains(&42),
        "variable 'sum_val' should have value 42 (a + b = 10 + 32), got: {:?}",
        sum_values
    );

    let doubled_values = find_variable_values(&events, "doubled");
    assert!(
        doubled_values.contains(&84),
        "variable 'doubled' should have value 84 (sum_val * 2 = 42 * 2), got: {:?}",
        doubled_values
    );

    let final_values = find_variable_values(&events, "final_result");
    assert!(
        final_values.contains(&94),
        "variable 'final_result' should have value 94 (doubled + a = 84 + 10), got: {:?}",
        final_values
    );
}

// ---------------------------------------------------------------------------
// Test 4: Verify Step events at correct source lines
// ---------------------------------------------------------------------------

#[test]
fn test_tolk_step_events() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    // Count Step events.
    let step_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e.get("Step").is_some())
        .collect();

    // flow_test.tolk has 5 var bindings + 1 return in compute() + 1 return in main()
    // but main()'s return calls compute() which adds its own steps.
    // At minimum: 5 var + 1 return in compute = 6, plus 1 return in main = 7.
    assert!(
        step_events.len() >= 6,
        "should have at least 6 step events for flow_test.tolk, got {}",
        step_events.len()
    );

    // Verify step events have valid structure.
    for event in &step_events {
        let step = event.get("Step").unwrap();
        assert!(
            step.get("path_id").is_some(),
            "Step event should have path_id field"
        );
        let line = step["line"].as_i64().expect("Step line should be an integer");
        assert!(line > 0, "Step line should be positive, got {}", line);
        assert!(
            line <= 15,
            "Step line should be within source file range, got {}",
            line
        );
    }

    // Verify that step events include the var binding lines.
    let step_lines: Vec<i64> = step_events
        .iter()
        .map(|e| e.get("Step").unwrap()["line"].as_i64().unwrap())
        .collect();

    // Line 2: var a: int = 10;
    assert!(
        step_lines.contains(&2),
        "step events should include line 2 (var a), got lines: {:?}",
        step_lines
    );
    // Line 3: var b: int = 32;
    assert!(
        step_lines.contains(&3),
        "step events should include line 3 (var b), got lines: {:?}",
        step_lines
    );
    // Line 4: var sum_val: int = a + b;
    assert!(
        step_lines.contains(&4),
        "step events should include line 4 (var sum_val), got lines: {:?}",
        step_lines
    );
    // Line 5: var doubled: int = sum_val * 2;
    assert!(
        step_lines.contains(&5),
        "step events should include line 5 (var doubled), got lines: {:?}",
        step_lines
    );
    // Line 6: var final_result: int = doubled + a;
    assert!(
        step_lines.contains(&6),
        "step events should include line 6 (var final_result), got lines: {:?}",
        step_lines
    );
}

// ---------------------------------------------------------------------------
// Test 5: Verify Call/Return events for compute() function call
// ---------------------------------------------------------------------------

#[test]
fn test_tolk_function_calls() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    // There should be Call events (function entries were recorded).
    let call_count = events.iter().filter(|e| e.get("Call").is_some()).count();
    assert!(
        call_count >= 2,
        "trace should contain at least 2 Call events (main + compute), got {}",
        call_count
    );

    // There should be Return events.
    let return_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e.get("Return").is_some())
        .collect();
    assert!(
        return_events.len() >= 2,
        "trace should contain at least 2 Return events, got {}",
        return_events.len()
    );

    // The last Return event should be after the last Step.
    let last_return_idx = events
        .iter()
        .rposition(|e| e.get("Return").is_some())
        .expect("should have a Return event");

    let steps_after_return = events[last_return_idx + 1..]
        .iter()
        .filter(|e| e.get("Step").is_some())
        .count();
    assert_eq!(
        steps_after_return, 0,
        "no Step events should appear after the final Return"
    );
}

// ---------------------------------------------------------------------------
// Test 6: Verify metadata JSON structure
// ---------------------------------------------------------------------------

#[test]
fn test_tolk_metadata_structure() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.tolk");
    run_tracer_on_file(&source_path, &out_dir);

    let metadata = load_trace_metadata(&out_dir);

    // TraceMetadata must contain "program", "args", and "workdir" fields.
    assert!(
        metadata.get("program").is_some(),
        "metadata should have 'program' field, got: {}",
        metadata
    );
    assert!(
        metadata["program"].is_string(),
        "metadata 'program' should be a string"
    );
    let program_str = metadata["program"].as_str().unwrap();
    assert!(
        program_str.contains("flow_test.tolk"),
        "metadata 'program' should reference the tolk source file, got: {}",
        program_str
    );

    assert!(
        metadata.get("args").is_some(),
        "metadata should have 'args' field, got: {}",
        metadata
    );
    assert!(
        metadata["args"].is_array(),
        "metadata 'args' should be an array"
    );

    assert!(
        metadata.get("workdir").is_some(),
        "metadata should have 'workdir' field, got: {}",
        metadata
    );
    assert!(
        metadata["workdir"].is_string(),
        "metadata 'workdir' should be a string"
    );
}

// ---------------------------------------------------------------------------
// Test 7: CLI record end-to-end test
// ---------------------------------------------------------------------------

#[test]
fn test_tolk_cli_record() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("cli-traces");
    let source_path = test_programs_dir().join("flow_test.tolk");

    let output = std::process::Command::new(env!("CARGO"))
        .args([
            "run",
            "--quiet",
            "--",
            "record",
            source_path.to_str().unwrap(),
            "--out-dir",
            out_dir.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run");

    assert!(
        output.status.success(),
        "record should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify output files exist.
    assert!(out_dir.join("trace.bin").exists());
    assert!(out_dir.join("trace_metadata.json").exists());
    assert!(out_dir.join("trace_paths.json").exists());

    // Verify the CLI-produced trace has actual content.
    let events = load_trace_events(&out_dir);
    assert!(!events.is_empty(), "CLI trace should have events");

    let step_count = events.iter().filter(|e| e.get("Step").is_some()).count();
    assert!(step_count > 0, "CLI trace should contain Step events");

    // Verify values are present in the CLI-produced trace too.
    let int_values = collect_int_values(&events);
    let all_values: Vec<i64> = int_values.iter().map(|(_, v)| *v).collect();
    assert!(
        all_values.contains(&94),
        "CLI trace should contain value 94, got values: {:?}",
        all_values
    );
}
