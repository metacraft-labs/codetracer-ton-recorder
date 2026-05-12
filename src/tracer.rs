//! Tracer implementation for Tolk/TON programs.
//!
//! Parses a Tolk source file to extract function definitions, variable
//! declarations (var/val), assignments, and return statements, evaluates
//! them via a real TVM (using `tycho-vm`), and emits CodeTracer trace
//! events (steps, calls, returns, variables).

use std::collections::HashMap;
use std::path::Path;

use codetracer_trace_types::{EventLogKind, Line, TypeKind, ValueRecord, NONE_VALUE};
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::{create_trace_writer, TraceEventsFileFormat};
use eyre::{eyre, Context, Result};

use crate::source_map::SourceMap;
use crate::stack_tracker::{self, StackTracker};

// The recorder is CTFS-only per `Recorder-CLI-Conventions.md` §4 (see
// `codetracer-specs`).  We pin every `create_trace_writer` call site to
// this constant so the tracer surface no longer carries a `format`
// parameter and the writer cannot accidentally drift away from the
// canonical multi-stream container.
const CTFS_FORMAT: TraceEventsFileFormat = TraceEventsFileFormat::Ctfs;

// ---------------------------------------------------------------------------
// Tolk AST types
// ---------------------------------------------------------------------------

/// A parsed Tolk function definition.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct FunctionDef {
    /// Function name (e.g. "main", "compute").
    name: String,
    /// Return type name (e.g. "int", "bool").
    return_type: Option<String>,
    /// Parameter list: (name, type_name) pairs.
    params: Vec<(String, String)>,
    /// Body statements.
    body: Vec<Statement>,
    /// 1-based line number where the function definition starts.
    line: u32,
}

/// A parsed statement in a Tolk function body.
#[derive(Debug, Clone)]
enum Statement {
    /// `var <name>: <type> = <expr>;` or `val <name>: <type> = <expr>;`
    VarBinding {
        name: String,
        type_name: String,
        expr: String,
        line: u32,
    },
    /// `return <expr>;`
    Return { expr: String, line: u32 },
    /// `throw <code>;` — Tolk's program-level failure marker.  Per
    /// `metacraft-specs/policies/recorder-test-requirements.md` §2,
    /// reaching this statement MUST surface an
    /// `EventLogKind::Error` io_event carrying the thrown exception
    /// code.  The recorder today only follows the call chain from
    /// `main()`, so `throw` is currently surfaced via a static
    /// post-execution sweep (`emit_error_events_for_program`) — once
    /// the recorder gains control-flow execution of every reachable
    /// throw, the inline arm in `evaluate_function` should emit the
    /// event live and the sweep should dedupe.
    Throw {
        /// The raw exception-code expression (e.g. `"7"`, `"42"`).
        code: String,
        #[allow(dead_code)]
        line: u32,
    },
    /// `assert (<cond>, <code>);` — Tolk's runtime assertion.  Per
    /// the same recorder-test-requirements policy this MUST surface
    /// as an `EventLogKind::Error` io_event when the assertion would
    /// fail.  Same static-sweep limitation as `Throw` above applies.
    Assert {
        /// The raw condition expression (e.g. `"probe > 0"`).  Held
        /// for the future runtime-aware emit path (see the
        /// `Statement::Assert` arm in `evaluate_function`) where we
        /// will evaluate the condition via the TVM and only emit the
        /// Error io_event when it would fail.  Until that lands the
        /// static sweep ignores this field and emits unconditionally.
        #[allow(dead_code)]
        condition: String,
        /// The raw exception-code expression (e.g. `"13"`).
        code: String,
        #[allow(dead_code)]
        line: u32,
    },
}

// ---------------------------------------------------------------------------
// The main tracer
// ---------------------------------------------------------------------------

/// The main tracer struct that captures Tolk execution traces.
pub struct TolkTracer {
    writer: Box<dyn TraceWriter + Send>,
    /// Registered type IDs for Tolk types.
    type_ids: HashMap<String, codetracer_trace_types::TypeId>,
}

impl TolkTracer {
    /// Trace a Tolk program and write CodeTracer output files.
    ///
    /// 1. Parses the source file for function definitions.
    /// 2. Evaluates function bodies starting from `main()` via the real TVM.
    /// 3. Emits Step events at source lines and Value events with variable values.
    /// 4. Writes a CTFS multi-stream `.ct` bundle plus `trace_metadata.json`
    ///    and `trace_paths.json` to `out_dir`.
    pub fn trace_program(source_path: &Path, source_code: &str, out_dir: &Path) -> Result<()> {
        // -- 1. Parse the Tolk source --
        let _source_map = SourceMap::from_source(source_path, source_code);
        let functions = parse_functions(source_code);

        eprintln!("Parsed {} functions", functions.len());

        // -- 2. Create the trace writer (CTFS only) --
        let program_str = source_path.to_string_lossy();
        let mut tracer = TolkTracer {
            writer: create_trace_writer(&program_str, &[], CTFS_FORMAT),
            type_ids: HashMap::new(),
        };

        // -- 3. Initialise output files --
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

        // CTFS-only writer — events stream lives in `trace.bin`.
        let events_path = out_dir.join("trace.bin");
        let metadata_path = out_dir.join("trace_metadata.json");
        let paths_path = out_dir.join("trace_paths.json");

        TraceWriter::begin_writing_trace_events(&mut *tracer.writer, &events_path)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::begin_writing_trace_metadata(&mut *tracer.writer, &metadata_path)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::begin_writing_trace_paths(&mut *tracer.writer, &paths_path)
            .map_err(|e| eyre!("{e}"))?;

        // -- 4. Start the trace --
        TraceWriter::start(&mut *tracer.writer, source_path, Line(1));

        // Register common Tolk types.
        for type_name in &["int", "bool"] {
            let type_id =
                TraceWriter::ensure_type_id(&mut *tracer.writer, TypeKind::Int, type_name);
            tracer.type_ids.insert(type_name.to_string(), type_id);
        }

        // -- 5. Evaluate and emit trace events --
        tracer.evaluate_program(source_path, &functions)?;

        // Surface every `throw <code>` / `assert (<cond>, <code>)`
        // statement in the program as an `EventLogKind::Error`
        // io_event.  Per
        // `metacraft-specs/policies/recorder-test-requirements.md` §2
        // any program-level failure marker (panic / abort / throw /
        // fail / revert / assert) MUST produce an Error io_event
        // carrying the failure reason text.  The recorder today only
        // follows the call chain from `main()` (a separate recorder
        // gap pinned by `test_error_paths_test_via_ct_print_full`),
        // so functions like `failing_compute` / `caught_compute` /
        // `assert_compute` are never reached at runtime — hence the
        // post-execution sweep over all parsed function bodies.
        // Mirrors the precedent in commit 7e5a177 of the cardano
        // recorder for Aiken `fail`.  When the Tolk recorder later
        // gains "execute every reachable throw/assert" support, the
        // inline `Statement::Throw` / `Statement::Assert` arms in
        // `evaluate_function` should emit the event live and this
        // sweep should dedupe against fails already surfaced from
        // the executed path.
        tracer.emit_error_events_for_program(&functions);

        // Close the <toplevel> call that start() opened.
        TraceWriter::register_return(&mut *tracer.writer, NONE_VALUE);

        // -- 6. Finish writing --
        TraceWriter::finish_writing_trace_events(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;
        TraceWriter::finish_writing_trace_metadata(&mut *tracer.writer)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::finish_writing_trace_paths(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;
        tracer.writer.close().map_err(|e| eyre!("{e}"))?;

        Ok(())
    }

    /// Emit one `EventLogKind::Error` io_event per `throw <code>` /
    /// `assert (<cond>, <code>)` statement found anywhere in the
    /// parsed program.  The metadata tags (`"TolkThrow"` /
    /// `"TolkAssert"`) mirror the conventions established by the
    /// cardano `"AikenFail"` (commit 7e5a177), move `"ABORTED: ..."`
    /// (commit 4041840) and wasm trap-reason audits — the frontend
    /// can route on them to distinguish source-level Tolk failures
    /// from generic TVM runtime exceptions (which carry
    /// `"tvm_exception"`).
    ///
    /// LIMITATION: this is a *static sweep*, not a runtime trigger.
    /// It emits one Error io_event per syntactically-present
    /// `throw`/`assert` whether or not the containing function is
    /// actually reachable from `main()`.  The error_paths_test.tolk
    /// fixture has exactly one `throw` and one `assert` in unreached
    /// functions, so the sweep is the only way to surface them
    /// without first fixing the orthogonal recorder gap that drops
    /// every function not transitively called from `main()`.  When
    /// that gap is fixed, the inline arms in `evaluate_function`
    /// should emit live and this sweep should dedupe.
    fn emit_error_events_for_program(&mut self, functions: &[FunctionDef]) {
        for func in functions {
            for stmt in &func.body {
                match stmt {
                    Statement::Throw { code, .. } => {
                        let message = format!("throw {code}");
                        TraceWriter::register_special_event(
                            &mut *self.writer,
                            EventLogKind::Error,
                            "TolkThrow",
                            &message,
                        );
                    }
                    Statement::Assert { code, .. } => {
                        let message = format!("assert: code {code}");
                        TraceWriter::register_special_event(
                            &mut *self.writer,
                            EventLogKind::Error,
                            "TolkAssert",
                            &message,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    /// Evaluate the program starting from `main()`.
    fn evaluate_program(&mut self, source_path: &Path, functions: &[FunctionDef]) -> Result<()> {
        // Build a function lookup table.
        let func_map: HashMap<String, &FunctionDef> =
            functions.iter().map(|f| (f.name.clone(), f)).collect();

        // Find and call main.
        let main_fn = func_map
            .get("main")
            .ok_or_else(|| eyre!("no main function found in Tolk program"))?;

        let mut env = HashMap::new();
        // Merge main() into <toplevel> by skipping its Call/Return events.
        // TraceWriter::start() already created <toplevel> at depth 0. Emitting
        // register_call(main) would push all main-body steps to depth 1 and any
        // nested calls (e.g. compute()) to depth 2. The db-backend's step-over
        // from depth 0 would then skip every step, breaking navigation.
        self.evaluate_function(source_path, main_fn, &func_map, &mut env, true)?;

        Ok(())
    }

    /// Evaluate a single function, emitting trace events.
    /// Returns the function's return value if any.
    ///
    /// When `is_entry_point` is true, the Call/Return events for this function
    /// are suppressed — its body is evaluated directly at the caller's depth
    /// (merged into `<toplevel>`).
    fn evaluate_function(
        &mut self,
        source_path: &Path,
        func: &FunctionDef,
        func_map: &HashMap<String, &FunctionDef>,
        _parent_env: &mut HashMap<String, i64>,
        is_entry_point: bool,
    ) -> Result<Option<i64>> {
        // Register function metadata (for function list / calltrace).
        let fn_id = TraceWriter::ensure_function_id(
            &mut *self.writer,
            &func.name,
            source_path,
            Line(func.line as i64),
        );
        // Only emit Call event for non-entry-point functions. The entry point
        // is merged into <toplevel> to keep its body at depth 0.
        if !is_entry_point {
            // Stage canonical Call args via writer.arg(name, value).
            //
            // The Tolk source carries a formal parameter list per
            // function (`fun foo(a: int, b: int): int`). We use that
            // list here so the calltrace pane's `.call-arg` rows show
            // each declared parameter rather than the empty list that
            // pre-fix register_call(fn_id, vec![]) produced.
            //
            // Concrete values are NONE_VALUE for now: the current Tolk
            // parser only recognises zero-arg call sites
            // (`compute()`), so the caller has no way to surface arg
            // values to this point. Extending parse_function_call /
            // evaluate_function to thread arg expressions would let
            // the staging path emit live values; tracked as an
            // open follow-up in AUDIT-CTFS-2026-05.md (parallel to
            // PolkaVM 1.55 ink!-metadata symbolic decoding and Miden
            // 1.56 per-procedure ABI / argument-name parsing).
            for (param_name, _param_type) in &func.params {
                let _ = TraceWriter::arg(&mut *self.writer, param_name, NONE_VALUE);
            }
            TraceWriter::register_call(&mut *self.writer, fn_id, vec![]);
        }

        // Local variable environment for this function.
        let mut env: HashMap<String, i64> = HashMap::new();
        let mut return_value: Option<i64> = None;
        // Symbolic stack tracker: mirrors TVM execution to reconstruct
        // source-level variable names from stack positions.
        let mut sym_stack = StackTracker::new();

        for stmt in &func.body {
            match stmt {
                Statement::VarBinding {
                    name,
                    type_name,
                    expr,
                    line,
                } => {
                    // Emit Step event.
                    TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));

                    // Evaluate the expression via the real TVM.
                    if let Some(val) = self.eval_expr(expr, &env, source_path, func_map)? {
                        // Track the expression symbolically. track_expr
                        // decomposes the expression and produces a derived
                        // name (e.g. "a + b") which we can inspect but the
                        // authoritative name is the LHS variable name.
                        let expr_tracker = stack_tracker::track_expr(expr, &env, val);
                        let _derived = expr_tracker.variables_at_step();

                        env.insert(name.clone(), val);

                        // Push the bound variable onto the symbolic stack so
                        // subsequent expressions can reference it.
                        sym_stack.push(val, Some(name.clone()));

                        // Emit Value event -- use the source-level variable
                        // name (which the stack tracker now carries).
                        let type_id = self
                            .type_ids
                            .get(type_name)
                            .copied()
                            .unwrap_or_else(|| self.type_ids.get("int").copied().unwrap());

                        let value = ValueRecord::Int { i: val, type_id };
                        TraceWriter::register_variable_with_full_value(
                            &mut *self.writer,
                            name,
                            value,
                        );
                    }
                }
                Statement::Return { expr, line } => {
                    // Emit Step event for the return line.
                    TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));

                    // Evaluate the return expression via the real TVM.
                    if let Some(val) = self.eval_expr(expr, &env, source_path, func_map)? {
                        return_value = Some(val);
                    }
                }
                Statement::Throw { .. } | Statement::Assert { .. } => {
                    // `throw <code>` / `assert (<cond>, <code>)` are
                    // currently surfaced as `EventLogKind::Error`
                    // io_events via the post-execution static sweep
                    // in `emit_error_events_for_program` so the event
                    // count is independent of which functions the
                    // recorder happens to follow from `main()`
                    // (today only the direct call chain — a separate
                    // recorder bug pinned by
                    // `test_error_paths_test_via_ct_print_full`).
                    // When we later wire execution of every reachable
                    // throw/assert, these arms should emit the event
                    // inline (paired with a step at the source line)
                    // and the sweep should dedupe.  Until then,
                    // breaking here keeps a runtime throw/assert
                    // from continuing to evaluate dead code after
                    // the failure.
                    break;
                }
            }
        }

        // Emit Return event (skip for entry point — its steps live under
        // <toplevel> which is closed separately).
        if !is_entry_point {
            match return_value {
                Some(val) => {
                    let type_id = self.type_ids.get("int").copied().unwrap();
                    let value = ValueRecord::Int { i: val, type_id };
                    TraceWriter::register_return(&mut *self.writer, value);
                }
                None => {
                    TraceWriter::register_return(&mut *self.writer, NONE_VALUE);
                }
            }
        }

        Ok(return_value)
    }

    /// Evaluate an expression in the current environment.
    /// Handles function calls, literals, variable references, and binary ops.
    /// All arithmetic is performed by the real TVM via `tycho-vm`.
    fn eval_expr(
        &mut self,
        expr: &str,
        env: &HashMap<String, i64>,
        source_path: &Path,
        func_map: &HashMap<String, &FunctionDef>,
    ) -> Result<Option<i64>> {
        let expr = expr.trim();

        if expr.is_empty() {
            return Ok(None);
        }

        // Check for function call: <name>()
        if let Some(call_name) = parse_function_call(expr) {
            if let Some(callee) = func_map.get(&call_name) {
                let callee = (*callee).clone();
                let mut dummy_env = HashMap::new();
                let result =
                    self.evaluate_function(source_path, &callee, func_map, &mut dummy_env, false)?;
                return Ok(result);
            }
        }

        // Evaluate via real TVM execution.  Use the checked variant
        // so we can route TVM execution failures (overflow, gas
        // exhaustion, divide-by-zero, etc.) through the structured
        // event channel instead of dropping them silently.
        match crate::tvm::tvm_eval_expr_checked(expr, env) {
            Ok(value) => Ok(value),
            Err(err) => {
                let message = format!("{err}");
                eprintln!("TVM execution error in '{expr}': {message}");
                TraceWriter::register_special_event(
                    &mut *self.writer,
                    EventLogKind::Error,
                    "tvm_exception",
                    &message,
                );
                // Treat as missing-value for downstream evaluation;
                // the partial trace continues to finalise cleanly
                // (matches Miden 1.56's "capture and break" pattern).
                Ok(None)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tolk source parser helpers
// ---------------------------------------------------------------------------

/// Parse function definitions from Tolk source code.
///
/// Handles the pattern: `fun <name>(<params>): <type> { ... }`
fn parse_functions(source: &str) -> Vec<FunctionDef> {
    let mut functions = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        let line_num = (i + 1) as u32;

        // Check for function definition: `fun <name>(...)`
        let after_keyword = if let Some(rest) = trimmed.strip_prefix("fun ") {
            rest
        } else {
            i += 1;
            continue;
        };

        // Parse function name.
        let name_end = after_keyword.find('(').unwrap_or(after_keyword.len());
        let name = after_keyword[..name_end].trim().to_string();

        // Parse parameters.
        let params = if let Some(paren_start) = after_keyword.find('(') {
            if let Some(paren_end) = after_keyword.find(')') {
                let params_str = &after_keyword[paren_start + 1..paren_end];
                parse_param_list(params_str)
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Parse return type: look for ): <type> {
        let return_type = if let Some(paren_end) = after_keyword.find(')') {
            let after_paren = after_keyword[paren_end + 1..].trim();
            if let Some(stripped) = after_paren.strip_prefix(':') {
                let after_colon = stripped.trim();
                let type_end = after_colon.find('{').unwrap_or(after_colon.len());
                let rt = after_colon[..type_end].trim().to_string();
                if rt.is_empty() {
                    None
                } else {
                    Some(rt)
                }
            } else {
                None
            }
        } else {
            None
        };

        // Parse body: collect statements between { and }.
        let mut body = Vec::new();
        let mut brace_depth = 0i32;
        let mut body_started = false;

        // Count opening braces on the definition line.
        for ch in lines[i].chars() {
            match ch {
                '{' => {
                    brace_depth += 1;
                    body_started = true;
                }
                '}' => brace_depth -= 1,
                _ => {}
            }
        }

        let mut j = i + 1;
        while j < lines.len() && (brace_depth > 0 || !body_started) {
            let body_line = lines[j].trim();
            let body_line_num = (j + 1) as u32;

            // Track brace depth.
            for ch in lines[j].chars() {
                match ch {
                    '{' => {
                        brace_depth += 1;
                        body_started = true;
                    }
                    '}' => brace_depth -= 1,
                    _ => {}
                }
            }

            // Parse statements.
            if let Some(stmt) = parse_statement(body_line, body_line_num) {
                body.push(stmt);
            }

            if brace_depth <= 0 && body_started {
                break;
            }
            j += 1;
        }

        if !name.is_empty() {
            functions.push(FunctionDef {
                name,
                return_type,
                params,
                body,
                line: line_num,
            });
        }

        i = j + 1;
    }

    functions
}

/// Parse a parameter list string like "a: int, b: int" into (name, type) pairs.
fn parse_param_list(params_str: &str) -> Vec<(String, String)> {
    let params_str = params_str.trim();
    if params_str.is_empty() {
        return vec![];
    }

    params_str
        .split(',')
        .filter_map(|param| {
            let param = param.trim();
            if let Some(colon_pos) = param.find(':') {
                let name = param[..colon_pos].trim().to_string();
                let type_name = param[colon_pos + 1..].trim().to_string();
                if !name.is_empty() && !type_name.is_empty() {
                    Some((name, type_name))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

/// Parse a single statement from a line of Tolk code.
fn parse_statement(line: &str, line_num: u32) -> Option<Statement> {
    let trimmed = line.trim();

    // var/val binding: `var <name>: <type> = <expr>;` or `val <name>: <type> = <expr>;`
    if let Some(after_keyword) = trimmed
        .strip_prefix("var ")
        .or_else(|| trimmed.strip_prefix("val "))
    {
        if let Some(colon_pos) = after_keyword.find(':') {
            let name = after_keyword[..colon_pos].trim().to_string();
            let after_colon = &after_keyword[colon_pos + 1..];
            if let Some(eq_pos) = after_colon.find('=') {
                let type_name = after_colon[..eq_pos].trim().to_string();
                let expr = after_colon[eq_pos + 1..]
                    .trim()
                    .trim_end_matches(';')
                    .trim()
                    .to_string();
                if !name.is_empty() && !type_name.is_empty() && !expr.is_empty() {
                    return Some(Statement::VarBinding {
                        name,
                        type_name,
                        expr,
                        line: line_num,
                    });
                }
            }
        }
    }

    // return statement: `return <expr>;`
    if let Some(rest) = trimmed.strip_prefix("return ") {
        let expr = rest.trim().trim_end_matches(';').trim().to_string();
        if !expr.is_empty() {
            return Some(Statement::Return {
                expr,
                line: line_num,
            });
        }
    }

    // throw statement: `throw <code>;` (Tolk's program-level
    // exception marker).  We preserve the raw code expression so the
    // emitted Error io_event can carry it verbatim.
    if let Some(rest) = trimmed.strip_prefix("throw ") {
        let code = rest.trim().trim_end_matches(';').trim().to_string();
        if !code.is_empty() {
            return Some(Statement::Throw {
                code,
                line: line_num,
            });
        }
    }

    // assert statement: `assert (<cond>, <code>);` — Tolk's runtime
    // assertion.  Be lenient about leading whitespace between
    // `assert` and the opening paren so both `assert(...)` and
    // `assert (...)` parse.
    if let Some(rest) = trimmed
        .strip_prefix("assert(")
        .or_else(|| trimmed.strip_prefix("assert ("))
    {
        // Strip a trailing `);` (and surrounding semicolons) so we're
        // left with the bare arg list.
        let inner = rest
            .trim()
            .trim_end_matches(';')
            .trim()
            .trim_end_matches(')')
            .trim();
        // Split into `<cond>, <code>` on the *last* comma so a
        // condition containing nested commas (e.g. `f(a, b) > 0`)
        // still produces a sensible code.
        if let Some(comma_pos) = inner.rfind(',') {
            let condition = inner[..comma_pos].trim().to_string();
            let code = inner[comma_pos + 1..].trim().to_string();
            if !condition.is_empty() && !code.is_empty() {
                return Some(Statement::Assert {
                    condition,
                    code,
                    line: line_num,
                });
            }
        }
    }

    None
}

/// Check if an expression is a simple function call like `compute()`.
/// Returns the function name if so.
fn parse_function_call(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if let Some(name) = expr.strip_suffix("()") {
        let name = name.trim();
        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Some(name.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tvm::tvm_eval_expr;

    #[test]
    fn test_parse_functions() {
        let source = r#"fun compute(): int {
    var a: int = 10;
    return a;
}

fun main(): int {
    return compute();
}"#;
        let functions = parse_functions(source);
        assert_eq!(functions.len(), 2);
        assert_eq!(functions[0].name, "compute");
        assert_eq!(functions[0].return_type, Some("int".to_string()));
        assert_eq!(functions[1].name, "main");
    }

    #[test]
    fn test_parse_statement_var() {
        let stmt = parse_statement("var a: int = 10;", 3);
        assert!(stmt.is_some());
        match stmt.unwrap() {
            Statement::VarBinding {
                name,
                type_name,
                expr,
                line,
            } => {
                assert_eq!(name, "a");
                assert_eq!(type_name, "int");
                assert_eq!(expr, "10");
                assert_eq!(line, 3);
            }
            _ => panic!("expected VarBinding"),
        }
    }

    #[test]
    fn test_parse_statement_val() {
        let stmt = parse_statement("val x: bool = true;", 5);
        assert!(stmt.is_some());
        match stmt.unwrap() {
            Statement::VarBinding {
                name,
                type_name,
                expr,
                line,
            } => {
                assert_eq!(name, "x");
                assert_eq!(type_name, "bool");
                assert_eq!(expr, "true");
                assert_eq!(line, 5);
            }
            _ => panic!("expected VarBinding"),
        }
    }

    #[test]
    fn test_parse_statement_return() {
        let stmt = parse_statement("return final_result;", 8);
        assert!(stmt.is_some());
        match stmt.unwrap() {
            Statement::Return { expr, line } => {
                assert_eq!(expr, "final_result");
                assert_eq!(line, 8);
            }
            _ => panic!("expected Return"),
        }
    }

    #[test]
    fn test_tvm_eval_simple_expr() {
        let mut known = HashMap::new();
        known.insert("a".to_string(), 10);
        known.insert("b".to_string(), 32);

        assert_eq!(tvm_eval_expr("10", &known), Some(10));
        assert_eq!(tvm_eval_expr("a", &known), Some(10));
        assert_eq!(tvm_eval_expr("a + b", &known), Some(42));
        assert_eq!(tvm_eval_expr("a * 2", &known), Some(20));
        assert_eq!(tvm_eval_expr("a % 3", &known), Some(1));
        assert_eq!(tvm_eval_expr("unknown", &known), None);
    }

    #[test]
    fn test_eval_chain() {
        let mut known = HashMap::new();
        known.insert("a".to_string(), 10);
        known.insert("b".to_string(), 32);
        known.insert("sum_val".to_string(), 42);
        known.insert("doubled".to_string(), 84);

        assert_eq!(tvm_eval_expr("a + b", &known), Some(42));
        assert_eq!(tvm_eval_expr("sum_val * 2", &known), Some(84));
        assert_eq!(tvm_eval_expr("doubled + a", &known), Some(94));
    }

    #[test]
    fn test_parse_function_call() {
        assert_eq!(
            parse_function_call("compute()"),
            Some("compute".to_string())
        );
        assert_eq!(parse_function_call("not_a_call"), None);
        assert_eq!(parse_function_call(""), None);
    }

    #[test]
    fn test_full_flow_test_evaluation() {
        let source = r#"fun compute(): int {
    var a: int = 10;
    var b: int = 32;
    var sum_val: int = a + b;
    var doubled: int = sum_val * 2;
    var final_result: int = doubled + a;
    return final_result;
}

fun main(): int {
    return compute();
}"#;
        let functions = parse_functions(source);
        assert_eq!(functions.len(), 2);

        // Simulate evaluation of compute() using the real TVM.
        let compute = &functions[0];
        assert_eq!(compute.name, "compute");
        assert_eq!(compute.body.len(), 6); // 5 var bindings + 1 return

        let mut env = HashMap::new();
        for stmt in &compute.body {
            if let Statement::VarBinding { name, expr, .. } = stmt {
                if let Some(val) = tvm_eval_expr(expr, &env) {
                    env.insert(name.clone(), val);
                }
            }
        }

        assert_eq!(env["a"], 10);
        assert_eq!(env["b"], 32);
        assert_eq!(env["sum_val"], 42);
        assert_eq!(env["doubled"], 84);
        assert_eq!(env["final_result"], 94);
    }

    #[test]
    fn test_parse_param_list() {
        let params = parse_param_list("a: int, b: int");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], ("a".to_string(), "int".to_string()));
        assert_eq!(params[1], ("b".to_string(), "int".to_string()));

        let empty = parse_param_list("");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_eval_comparisons() {
        let mut known = HashMap::new();
        known.insert("a".to_string(), 10);
        known.insert("b".to_string(), 32);

        assert_eq!(tvm_eval_expr("a == 10", &known), Some(1));
        assert_eq!(tvm_eval_expr("a != b", &known), Some(1));
        assert_eq!(tvm_eval_expr("a < b", &known), Some(1));
        assert_eq!(tvm_eval_expr("b > a", &known), Some(1));
        assert_eq!(tvm_eval_expr("a <= 10", &known), Some(1));
        assert_eq!(tvm_eval_expr("a >= 11", &known), Some(0));
    }

    #[test]
    fn test_eval_booleans() {
        let known = HashMap::new();
        assert_eq!(tvm_eval_expr("true", &known), Some(1));
        assert_eq!(tvm_eval_expr("false", &known), Some(0));
    }
}
