//! Recording logic for Tolk execution traces.
//!
//! This module provides the top-level `record` function that reads a Tolk
//! source file, evaluates function bodies, captures the trace, and writes
//! CodeTracer output.

use std::path::Path;

use codetracer_trace_writer::TraceEventsFileFormat;
use eyre::{Context, Result};

use crate::tracer::TolkTracer;

/// Record a Tolk execution trace.
///
/// Reads the Tolk source file at `source_path`, parses function definitions,
/// variable declarations, and return statements, evaluates them, captures
/// the trace, and writes CodeTracer trace files to `out_dir`.
pub fn record(
    source_path: &Path,
    out_dir: &Path,
    format: TraceEventsFileFormat,
) -> Result<()> {
    let source_code = std::fs::read_to_string(source_path)
        .with_context(|| format!("failed to read source file: {}", source_path.display()))?;

    TolkTracer::trace_program(source_path, &source_code, out_dir, format)
}
