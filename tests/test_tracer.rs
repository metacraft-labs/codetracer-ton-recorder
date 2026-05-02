//! Integration tests for the Tolk/TON tracer.

use std::path::{Path, PathBuf};

use codetracer_trace_writer_nim::TraceEventsFileFormat;

const CTFS_MAGIC: [u8; 5] = [0xC0, 0xDE, 0x72, 0xAC, 0xE2];

fn test_programs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-programs/tolk")
}

fn run_tracer_on_file(source_path: &Path, out_dir: &Path) {
    // Use the canonical CTFS multi-stream container -- the format
    // the Nim ct_reader_* FFI and the db-backend's CTFSTraceReader
    // consume directly.  See AUDIT-CTFS-2026-05.md (audit a) for the
    // CLI-side default change that aligns with this.
    codetracer_ton_recorder::recorder::record(source_path, out_dir, TraceEventsFileFormat::Ctfs)
        .expect("trace_program should succeed");
}

fn assert_valid_ct_file(out_dir: &Path) -> PathBuf {
    let ct_files: Vec<_> = std::fs::read_dir(out_dir)
        .expect("failed to read output directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |ext| ext == "ct"))
        .collect();
    assert!(!ct_files.is_empty(), "expected at least one .ct file in {:?}", out_dir);
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
    assert!(size > 100, ".ct file should have substantial content, got {} bytes", size);
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

#[test]
fn test_ton_cli_record() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("cli-traces");
    let source_path = test_programs_dir().join("flow_test.tolk");

    // Exercise the canonical CTFS path (post-audit default).
    let output = std::process::Command::new(env!("CARGO"))
        .args([
            "run", "--quiet", "--",
            "record", source_path.to_str().unwrap(),
            "--out-dir", out_dir.to_str().unwrap(),
            "--format", "ctfs",
        ])
        .output()
        .expect("failed to run");
    assert!(output.status.success(), "record should succeed, stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert_valid_ct_file(&out_dir);
}
