//! @ton/sandbox integration for parsing `vm_logs_full` output.
//!
//! The `@ton/sandbox` test framework emits detailed TVM execution logs
//! (`vm_logs_full`) that record every instruction executed, gas consumption,
//! and stack state.  This module parses those logs and converts them into
//! CodeTracer trace events, enabling developers to debug TON smart contracts
//! by replaying sandbox test runs inside CodeTracer.

use std::path::Path;

use codetracer_trace_types::{EventLogKind, Line, TypeKind, ValueRecord};
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::{create_trace_writer, TraceEventsFileFormat};
use eyre::{eyre, Context, Result};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single parsed entry from @ton/sandbox `vm_logs_full` output.
///
/// Each entry corresponds to one TVM instruction execution and captures
/// the instruction name, gas accounting, and the stack snapshot after
/// the instruction has executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmLog {
    /// The TVM opcode name (e.g. "PUSHINT 10", "ADD").
    pub instruction: String,
    /// Gas consumed so far (the "before" value in `gas: before -> after`).
    pub gas_used: u64,
    /// Remaining gas after this instruction (the "after" value).
    pub gas_remaining: u64,
    /// Stack contents after the instruction, represented as strings.
    pub stack: Vec<String>,
    /// Exit code, present only on the final log entry.
    pub exit_code: Option<i32>,
}

/// Configuration for sandbox trace ingestion.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Path to the `vm_logs_full` output file.
    pub vm_log_path: std::path::PathBuf,
    /// Optional path to the source file for source mapping.
    pub source_path: Option<std::path::PathBuf>,
}

// ---------------------------------------------------------------------------
// A lightweight "trace event" for the conversion layer
// ---------------------------------------------------------------------------

/// A CodeTracer-compatible trace event produced from sandbox logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEvent {
    /// 1-based step number (instruction index in the log).
    pub step: u32,
    /// The TVM instruction that was executed.
    pub instruction: String,
    /// Gas remaining after this instruction.
    pub gas_remaining: u64,
    /// Stack snapshot after the instruction.
    pub stack: Vec<String>,
    /// Exit code (only on the last event if the log contains one).
    pub exit_code: Option<i32>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse @ton/sandbox `vm_logs_full` text into a sequence of [`VmLog`]s.
///
/// The expected format is a repeating group of lines:
///
/// ```text
/// execute <INSTRUCTION>
/// gas: <before> -> <after>
/// stack: [<values...>]
/// ```
///
/// An optional trailing `exit code: <N>` line is attached to the last
/// instruction entry.
pub fn parse_vm_logs(log_text: &str) -> Result<Vec<VmLog>> {
    let mut logs: Vec<VmLog> = Vec::new();
    let lines: Vec<&str> = log_text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        // Look for "execute <INSTRUCTION>"
        if let Some(instruction) = trimmed.strip_prefix("execute ") {
            let instruction = instruction.trim().to_string();

            // Next line should be "gas: <before> -> <after>"
            i += 1;
            let (gas_used, gas_remaining) = if i < lines.len() {
                parse_gas_line(lines[i].trim())?
            } else {
                return Err(eyre!(
                    "unexpected end of log: expected gas line after instruction '{instruction}'"
                ));
            };

            // Next line should be "stack: [...]"
            i += 1;
            let stack = if i < lines.len() {
                parse_stack_line(lines[i].trim())?
            } else {
                return Err(eyre!(
                    "unexpected end of log: expected stack line after instruction '{instruction}'"
                ));
            };

            logs.push(VmLog {
                instruction,
                gas_used,
                gas_remaining,
                stack,
                exit_code: None,
            });

            i += 1;
            continue;
        }

        // Look for "exit code: <N>"
        if let Some(code_str) = trimmed.strip_prefix("exit code:") {
            let code: i32 = code_str
                .trim()
                .parse()
                .with_context(|| format!("invalid exit code: '{}'", code_str.trim()))?;
            // Attach exit code to the last log entry.
            if let Some(last) = logs.last_mut() {
                last.exit_code = Some(code);
            }
            i += 1;
            continue;
        }

        // Skip blank or unrecognised lines.
        i += 1;
    }

    Ok(logs)
}

/// Parse a `gas: <before> -> <after>` line.
///
/// Gas values may be negative (when credit is exhausted), so we parse
/// as i64 and convert.  The `gas_used` field is the absolute "before"
/// value and `gas_remaining` the absolute "after" value; negative
/// values are clamped to 0 for the u64 representation.
fn parse_gas_line(line: &str) -> Result<(u64, u64)> {
    let rest = line
        .strip_prefix("gas:")
        .ok_or_else(|| eyre!("expected gas line, got: '{line}'"))?
        .trim();

    let parts: Vec<&str> = rest.split("->").collect();
    if parts.len() != 2 {
        return Err(eyre!("malformed gas line: '{line}'"));
    }

    let before: i64 = parts[0]
        .trim()
        .parse()
        .with_context(|| format!("invalid gas-before value in '{line}'"))?;
    let after: i64 = parts[1]
        .trim()
        .parse()
        .with_context(|| format!("invalid gas-after value in '{line}'"))?;

    Ok((before.unsigned_abs(), after.unsigned_abs()))
}

/// Parse a `stack: [<val1> <val2> ...]` line.
fn parse_stack_line(line: &str) -> Result<Vec<String>> {
    let rest = line
        .strip_prefix("stack:")
        .ok_or_else(|| eyre!("expected stack line, got: '{line}'"))?
        .trim();

    // Strip brackets.
    let inner = rest
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| eyre!("malformed stack line (missing brackets): '{line}'"))?
        .trim();

    if inner.is_empty() {
        return Ok(Vec::new());
    }

    Ok(inner.split_whitespace().map(|s| s.to_string()).collect())
}

// ---------------------------------------------------------------------------
// Conversion to CodeTracer trace events
// ---------------------------------------------------------------------------

/// Convert parsed [`VmLog`] entries into [`TraceEvent`]s.
///
/// Each instruction becomes a numbered step.  The `source_path` is carried
/// through so that downstream writers can associate events with a file.
pub fn convert_vm_logs_to_trace(logs: &[VmLog], _source_path: &Path) -> Result<Vec<TraceEvent>> {
    let mut events = Vec::with_capacity(logs.len());

    for (idx, log) in logs.iter().enumerate() {
        events.push(TraceEvent {
            step: (idx + 1) as u32,
            instruction: log.instruction.clone(),
            gas_remaining: log.gas_remaining,
            stack: log.stack.clone(),
            exit_code: log.exit_code,
        });
    }

    Ok(events)
}

// ---------------------------------------------------------------------------
// Full trace-writing pipeline (used by the CLI `trace-sandbox` command)
// ---------------------------------------------------------------------------

/// Parse a `vm_logs_full` file and write CodeTracer trace output.
pub fn trace_sandbox(
    vm_log_path: &Path,
    source_path: &Path,
    out_dir: &Path,
    format: TraceEventsFileFormat,
) -> Result<()> {
    let log_text = std::fs::read_to_string(vm_log_path)
        .with_context(|| format!("failed to read vm_log file: {}", vm_log_path.display()))?;

    let logs = parse_vm_logs(&log_text)?;
    let events = convert_vm_logs_to_trace(&logs, source_path)?;

    // -- Set up the trace writer --
    let program_str = source_path.to_string_lossy();
    let mut writer = create_trace_writer(&program_str, &[], format);

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    let events_filename = match format {
        TraceEventsFileFormat::Json => "trace.json",
        TraceEventsFileFormat::Binary
        | TraceEventsFileFormat::BinaryV0
        | TraceEventsFileFormat::Ctfs => "trace.bin",
    };
    let events_path = out_dir.join(events_filename);
    let metadata_path = out_dir.join("trace_metadata.json");
    let paths_path = out_dir.join("trace_paths.json");

    TraceWriter::begin_writing_trace_events(&mut *writer, &events_path)
        .map_err(|e| eyre!("{e}"))?;
    TraceWriter::begin_writing_trace_metadata(&mut *writer, &metadata_path)
        .map_err(|e| eyre!("{e}"))?;
    TraceWriter::begin_writing_trace_paths(&mut *writer, &paths_path).map_err(|e| eyre!("{e}"))?;

    TraceWriter::start(&mut *writer, source_path, Line(1));

    // Register a type for stack integers.
    let int_type_id = TraceWriter::ensure_type_id(&mut *writer, TypeKind::Int, "int");

    // Emit a step + variable events for each instruction.
    let fn_id = TraceWriter::ensure_function_id(&mut *writer, "<sandbox>", source_path, Line(1));
    TraceWriter::register_call(&mut *writer, fn_id, vec![]);

    for event in &events {
        TraceWriter::register_step(&mut *writer, source_path, Line(event.step as i64));

        // Emit the top-of-stack as a variable named "tos" when the stack
        // is non-empty.
        if let Some(top) = event.stack.last() {
            if let Ok(val) = top.parse::<i64>() {
                let value = ValueRecord::Int {
                    i: val,
                    type_id: int_type_id,
                };
                TraceWriter::register_variable_with_full_value(&mut *writer, "tos", value);
            }
        }

        // Route any non-success TVM exit code through the structured
        // event channel as an Error special event.  TVM's `THROW`
        // family of opcodes terminates the VM with a non-zero exit
        // code (see TVM Spec §4.5 "Exception primitives" -
        // https://docs.ton.org/tvm.pdf).  Pre-fix the recorder
        // dropped that signal entirely; post-fix the frontend's
        // error stream surfaces it (mirrors Miden 1.56
        // `miden_vm_error` and Cairo 1.50 `CairoPanic` routing).
        if let Some(exit_code) = event.exit_code {
            if exit_code != 0 {
                let message = format!(
                    "TVM exception at instruction '{}' (exit code {})",
                    event.instruction, exit_code
                );
                TraceWriter::register_special_event(
                    &mut *writer,
                    EventLogKind::Error,
                    "tvm_exception",
                    &message,
                );
            }
        }
    }

    // Finish.
    TraceWriter::finish_writing_trace_events(&mut *writer).map_err(|e| eyre!("{e}"))?;
    TraceWriter::finish_writing_trace_metadata(&mut *writer).map_err(|e| eyre!("{e}"))?;
    TraceWriter::finish_writing_trace_paths(&mut *writer).map_err(|e| eyre!("{e}"))?;
    writer.close().map_err(|e| eyre!("{e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Minimal vm_logs_full snippet used across tests.
    const SAMPLE_LOG: &str = "\
execute PUSHINT 10
gas: 26 -> 18
stack: [10]
execute PUSHINT 32
gas: 18 -> 10
stack: [10 32]
execute ADD
gas: 10 -> 2
stack: [42]
exit code: 0
";

    // -- Parsing tests -------------------------------------------------------

    #[test]
    fn test_parse_vm_logs_instruction_count() {
        let logs = parse_vm_logs(SAMPLE_LOG).unwrap();
        assert_eq!(logs.len(), 3);
    }

    #[test]
    fn test_parse_vm_logs_instructions() {
        let logs = parse_vm_logs(SAMPLE_LOG).unwrap();
        assert_eq!(logs[0].instruction, "PUSHINT 10");
        assert_eq!(logs[1].instruction, "PUSHINT 32");
        assert_eq!(logs[2].instruction, "ADD");
    }

    #[test]
    fn test_parse_vm_logs_gas() {
        let logs = parse_vm_logs(SAMPLE_LOG).unwrap();
        // First instruction: gas 26 -> 18
        assert_eq!(logs[0].gas_used, 26);
        assert_eq!(logs[0].gas_remaining, 18);
        // Second instruction: gas 18 -> 10
        assert_eq!(logs[1].gas_used, 18);
        assert_eq!(logs[1].gas_remaining, 10);
        // Third instruction: gas 10 -> 2
        assert_eq!(logs[2].gas_used, 10);
        assert_eq!(logs[2].gas_remaining, 2);
    }

    #[test]
    fn test_parse_vm_logs_stack() {
        let logs = parse_vm_logs(SAMPLE_LOG).unwrap();
        assert_eq!(logs[0].stack, vec!["10"]);
        assert_eq!(logs[1].stack, vec!["10", "32"]);
        assert_eq!(logs[2].stack, vec!["42"]);
    }

    #[test]
    fn test_parse_vm_logs_exit_code() {
        let logs = parse_vm_logs(SAMPLE_LOG).unwrap();
        // Only the last entry should have an exit code.
        assert_eq!(logs[0].exit_code, None);
        assert_eq!(logs[1].exit_code, None);
        assert_eq!(logs[2].exit_code, Some(0));
    }

    #[test]
    fn test_parse_vm_logs_negative_gas() {
        let log = "\
execute MUL
gas: -6 -> -14
stack: [84]
";
        let logs = parse_vm_logs(log).unwrap();
        assert_eq!(logs[0].gas_used, 6);
        assert_eq!(logs[0].gas_remaining, 14);
    }

    #[test]
    fn test_parse_vm_logs_empty_stack() {
        let log = "\
execute NOP
gas: 10 -> 9
stack: []
";
        let logs = parse_vm_logs(log).unwrap();
        assert!(logs[0].stack.is_empty());
    }

    #[test]
    fn test_parse_vm_logs_empty_input() {
        let logs = parse_vm_logs("").unwrap();
        assert!(logs.is_empty());
    }

    // -- Conversion tests ----------------------------------------------------

    #[test]
    fn test_convert_vm_logs_to_trace_step_numbers() {
        let logs = parse_vm_logs(SAMPLE_LOG).unwrap();
        let source = PathBuf::from("test.tolk");
        let events = convert_vm_logs_to_trace(&logs, &source).unwrap();

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].step, 1);
        assert_eq!(events[1].step, 2);
        assert_eq!(events[2].step, 3);
    }

    #[test]
    fn test_convert_vm_logs_to_trace_instructions() {
        let logs = parse_vm_logs(SAMPLE_LOG).unwrap();
        let source = PathBuf::from("test.tolk");
        let events = convert_vm_logs_to_trace(&logs, &source).unwrap();

        assert_eq!(events[0].instruction, "PUSHINT 10");
        assert_eq!(events[1].instruction, "PUSHINT 32");
        assert_eq!(events[2].instruction, "ADD");
    }

    #[test]
    fn test_convert_vm_logs_to_trace_gas() {
        let logs = parse_vm_logs(SAMPLE_LOG).unwrap();
        let source = PathBuf::from("test.tolk");
        let events = convert_vm_logs_to_trace(&logs, &source).unwrap();

        assert_eq!(events[0].gas_remaining, 18);
        assert_eq!(events[1].gas_remaining, 10);
        assert_eq!(events[2].gas_remaining, 2);
    }

    #[test]
    fn test_convert_vm_logs_to_trace_stack() {
        let logs = parse_vm_logs(SAMPLE_LOG).unwrap();
        let source = PathBuf::from("test.tolk");
        let events = convert_vm_logs_to_trace(&logs, &source).unwrap();

        assert_eq!(events[0].stack, vec!["10"]);
        assert_eq!(events[1].stack, vec!["10", "32"]);
        assert_eq!(events[2].stack, vec!["42"]);
    }

    #[test]
    fn test_convert_vm_logs_to_trace_exit_code() {
        let logs = parse_vm_logs(SAMPLE_LOG).unwrap();
        let source = PathBuf::from("test.tolk");
        let events = convert_vm_logs_to_trace(&logs, &source).unwrap();

        assert_eq!(events[0].exit_code, None);
        assert_eq!(events[2].exit_code, Some(0));
    }

    // -- Fixture file test ---------------------------------------------------

    #[test]
    fn test_parse_mock_vm_log_fixture() {
        let fixture_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-programs/tolk/mock_vm_log.txt");
        let log_text =
            std::fs::read_to_string(&fixture_path).expect("mock_vm_log.txt fixture should exist");

        let logs = parse_vm_logs(&log_text).unwrap();
        // The fixture has 7 instructions.
        assert_eq!(logs.len(), 7);

        // Verify first instruction.
        assert_eq!(logs[0].instruction, "PUSHINT 10");
        assert_eq!(logs[0].gas_used, 26);
        assert_eq!(logs[0].gas_remaining, 18);
        assert_eq!(logs[0].stack, vec!["10"]);

        // Verify ADD result.
        assert_eq!(logs[2].instruction, "ADD");
        assert_eq!(logs[2].stack, vec!["42"]);

        // Verify MUL result.
        assert_eq!(logs[4].instruction, "MUL");
        assert_eq!(logs[4].stack, vec!["84"]);

        // Verify final ADD result (94) with exit code.
        assert_eq!(logs[6].instruction, "ADD");
        assert_eq!(logs[6].stack, vec!["94"]);
        assert_eq!(logs[6].exit_code, Some(0));
    }

    #[test]
    fn test_fixture_convert_to_trace_events() {
        let fixture_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-programs/tolk/mock_vm_log.txt");
        let log_text = std::fs::read_to_string(&fixture_path).unwrap();
        let logs = parse_vm_logs(&log_text).unwrap();
        let source = PathBuf::from("flow_test.tolk");
        let events = convert_vm_logs_to_trace(&logs, &source).unwrap();

        assert_eq!(events.len(), 7);
        // Steps should be sequential 1..=7.
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.step, (i + 1) as u32);
        }
        // Final event should carry exit code and show 94 on the stack.
        let last = events.last().unwrap();
        assert_eq!(last.exit_code, Some(0));
        assert_eq!(last.stack, vec!["94"]);
    }
}
