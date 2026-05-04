//! Section 5.6 CTFS audit tests for the codetracer-ton-recorder.
//!
//! Each test pins a behaviour that the 2026-05-02 audit
//! (AUDIT-CTFS-2026-05.md, isonim-migration.txt §1.57) closed.
//! Pre-fix versions of these tests would have failed; post-fix they
//! stay green.

use std::path::PathBuf;
use std::process::Command;

use codetracer_trace_writer_nim::{NimTraceReaderHandle, TraceEventsFileFormat};

/// Canonical CodeTracer multi-stream (CTFS) container magic bytes.
///
/// Mirrors `CTFS_MAGIC` defined in the trace-format Nim writer.  The
/// db-backend's `CTFSTraceReader` rejects any file that does not
/// start with these five bytes.
const CTFS_MAGIC: [u8; 5] = [0xC0, 0xDE, 0x72, 0xAC, 0xE2];

/// Path to the bundled Tolk fixture used across audit tests.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-programs/tolk/flow_test.tolk")
}

/// Locate the single `.ct` file produced by a recorder run.
fn locate_ct_file(out_dir: &std::path::Path) -> PathBuf {
    let mut ct_files: Vec<_> = std::fs::read_dir(out_dir)
        .expect("failed to read output directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
        .collect();
    ct_files.sort();
    assert_eq!(
        ct_files.len(),
        1,
        "expected exactly one .ct file in {out_dir:?}, got {ct_files:?}",
    );
    ct_files.remove(0)
}

fn read_events(out_dir: &std::path::Path) -> Vec<serde_json::Value> {
    let ct_path = locate_ct_file(out_dir);
    let reader = NimTraceReaderHandle::open(&ct_path.to_string_lossy()).unwrap_or_else(|e| {
        panic!(
            "failed to open Nim CTFS reader for {}: {e}",
            ct_path.display()
        )
    });
    (0..reader.event_count())
        .map(|index| {
            let json = reader.event_json(index).expect("read event JSON");
            serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("invalid event JSON: {e}: {json}"))
        })
        .collect()
}

fn string_from_json_byte_array(value: &serde_json::Value) -> String {
    let bytes: Vec<u8> = value
        .as_array()
        .unwrap_or_else(|| panic!("expected byte array JSON, got {value:#}"))
        .iter()
        .map(|byte| {
            byte.as_u64()
                .unwrap_or_else(|| panic!("expected byte value, got {byte:#}")) as u8
        })
        .collect();
    String::from_utf8(bytes).unwrap_or_else(|e| panic!("expected UTF-8 event payload: {e}"))
}

/// Audit (a) + (g): the canonical CTFS dispatch produces a valid
/// multi-stream container.
///
/// Pre-fix the CLI's `--format` flag had no `ctfs` value, so the
/// canonical container was unreachable from the CLI.  Post-fix the
/// flag accepts `ctfs` and defaults to it; the produced file starts
/// with the CTFS magic bytes and is materially populated.
#[test]
fn ctfs_writer_produces_ct_container() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    codetracer_ton_recorder::recorder::record(
        &fixture_path(),
        &out_dir,
        TraceEventsFileFormat::Ctfs,
    )
    .expect("record should succeed");

    let ct_path = locate_ct_file(&out_dir);
    let bytes = std::fs::read(&ct_path).expect("read .ct");
    assert!(
        bytes.len() >= 64,
        ".ct file should be materially populated; got {} bytes",
        bytes.len()
    );
    assert_eq!(
        &bytes[..5],
        &CTFS_MAGIC,
        "CTFS magic bytes mismatch at {ct_path:?}",
    );
}

/// Audit (a): `record --help` advertises `ctfs` as a valid `--format`
/// value with `[default: ctfs]`.
///
/// Same idiom as Flow 1.52 / Fuel 1.53 / PolkaVM 1.55 / Miden 1.56:
/// catches accidental defaults regressions at the CLI surface.
#[test]
fn ctfs_format_advertised_in_record_help() {
    let bin = env!("CARGO_BIN_EXE_codetracer-ton-recorder");
    let output = Command::new(bin)
        .args(["record", "--help"])
        .output()
        .expect("failed to run record --help");
    assert!(
        output.status.success(),
        "record --help should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("ctfs"),
        "record --help should advertise the ctfs format; help text:\n{help}",
    );
    assert!(
        help.contains("[default: ctfs]"),
        "record --help should show ctfs as the default; help text:\n{help}",
    );
}

/// Audit (a): `replay --help` and `trace-sandbox --help` also default
/// to `ctfs`.  Same default-format gap, same fix surface.
#[test]
fn ctfs_format_advertised_in_replay_and_sandbox_help() {
    let bin = env!("CARGO_BIN_EXE_codetracer-ton-recorder");
    for sub in ["replay", "trace-sandbox"] {
        let output = Command::new(bin)
            .args([sub, "--help"])
            .output()
            .unwrap_or_else(|e| panic!("failed to run {sub} --help: {e}"));
        assert!(
            output.status.success(),
            "{sub} --help should succeed; stderr: {}",
            String::from_utf8_lossy(&output.stderr),
        );
        let help = String::from_utf8_lossy(&output.stdout);
        assert!(
            help.contains("[default: ctfs]"),
            "{sub} --help should default to ctfs; help text:\n{help}",
        );
    }
}

/// Audit (c): the call-arg staging path introduced in this audit
/// (`TraceWriter::arg(param_name, NONE_VALUE)` per declared formal
/// parameter) does not regress the size or magic of the canonical
/// CTFS container.  The bundled fixture has a no-arg `compute()` and
/// a no-arg `main()`, so this is primarily a structural-smoke test
/// asserting that the staging branch still produces a valid file
/// even when there are no params to stage.
///
/// When the Tolk parser is extended to accept arg-passing call sites
/// (open follow-up in AUDIT-CTFS-2026-05.md), this test should be
/// upgraded to also assert that staged arg values appear on the
/// `CallRecord.args` slice once the read-side helper lands.
#[test]
fn call_arg_staging_does_not_empty_trace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    codetracer_ton_recorder::recorder::record(
        &fixture_path(),
        &out_dir,
        TraceEventsFileFormat::Ctfs,
    )
    .expect("record should succeed");

    let ct_path = locate_ct_file(&out_dir);
    let bytes = std::fs::read(&ct_path).expect("read .ct");
    assert_eq!(&bytes[..5], &CTFS_MAGIC);
    assert!(
        bytes.len() >= 64,
        ".ct file should remain materially populated post-arg-staging; got {} bytes",
        bytes.len()
    );
}

/// Audit (d): sandbox action-list out-message trailers should surface as
/// canonical EvmEvent special events in the CTFS event stream.
#[test]
fn ctfs_reader_sees_sandbox_out_message_event() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let log_path = tmp.path().join("vm_logs_full.txt");
    std::fs::write(
        &log_path,
        "\
execute PUSHINT 1
gas: 26 -> 18
stack: [1]
exit code: 0
action: SENDRAWMSG mode=3 dst=EQDabc value=100 body=0xdeadbeef
",
    )
    .expect("write sandbox log");

    let source_path = tmp.path().join("contract.tolk");
    std::fs::write(&source_path, "fun main(): int {\n    return 1;\n}\n").expect("write source");

    let out_dir = tmp.path().join("traces");
    codetracer_ton_recorder::sandbox::trace_sandbox(
        &log_path,
        &source_path,
        &out_dir,
        TraceEventsFileFormat::Ctfs,
    )
    .expect("trace sandbox log");

    let events = read_events(&out_dir);
    let event = events
        .iter()
        .find(|event| event["kind"].as_str() == Some("stderr"))
        .unwrap_or_else(|| panic!("missing CTFS EvmEvent entry: {events:#?}"));
    let content = string_from_json_byte_array(&event["data"]);
    assert!(
        content.contains("SENDRAWMSG")
            && content.contains("EQDabc")
            && content.contains("0xdeadbeef"),
        "unexpected sandbox out-message content: {content}"
    );
}
