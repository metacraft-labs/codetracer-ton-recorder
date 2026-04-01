//! Tracer implementation for Tolk/TON programs.
//!
//! Parses a Tolk source file to extract function definitions, variable
//! declarations (var/val), assignments, and return statements, evaluates
//! them via a real TVM (using `tycho-vm`), and emits CodeTracer trace
//! events (steps, calls, returns, variables).

use std::collections::HashMap;
use std::path::Path;

use codetracer_trace_types::{Line, TypeKind, ValueRecord, NONE_VALUE};
use codetracer_trace_writer::trace_writer::TraceWriter;
use codetracer_trace_writer::{create_trace_writer, TraceEventsFileFormat};
use eyre::{eyre, Context, Result};

use crate::source_map::SourceMap;
use crate::stack_tracker::{self, StackTracker};

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
    /// 4. Writes trace.bin, trace_metadata.json, trace_paths.json.
    pub fn trace_program(
        source_path: &Path,
        source_code: &str,
        out_dir: &Path,
        format: TraceEventsFileFormat,
    ) -> Result<()> {
        // -- 1. Parse the Tolk source --
        let _source_map = SourceMap::from_source(source_path, source_code);
        let functions = parse_functions(source_code);

        eprintln!("Parsed {} functions", functions.len());

        // -- 2. Create the trace writer --
        let program_str = source_path.to_string_lossy();
        let mut tracer = TolkTracer {
            writer: create_trace_writer(&program_str, &[], format),
            type_ids: HashMap::new(),
        };

        // -- 3. Initialise output files --
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

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

        // -- 6. Finish writing --
        TraceWriter::finish_writing_trace_events(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;
        TraceWriter::finish_writing_trace_metadata(&mut *tracer.writer)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::finish_writing_trace_paths(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;

        Ok(())
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
        self.evaluate_function(source_path, main_fn, &func_map, &mut env)?;

        Ok(())
    }

    /// Evaluate a single function, emitting trace events.
    /// Returns the function's return value if any.
    fn evaluate_function(
        &mut self,
        source_path: &Path,
        func: &FunctionDef,
        func_map: &HashMap<String, &FunctionDef>,
        _parent_env: &mut HashMap<String, i64>,
    ) -> Result<Option<i64>> {
        // Emit Call event.
        let fn_id = TraceWriter::ensure_function_id(
            &mut *self.writer,
            &func.name,
            source_path,
            Line(func.line as i64),
        );
        TraceWriter::register_call(&mut *self.writer, fn_id, vec![]);

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
            }
        }

        // Emit Return event.
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
                    self.evaluate_function(source_path, &callee, func_map, &mut dummy_env)?;
                return Ok(result);
            }
        }

        // Evaluate via real TVM execution.
        Ok(crate::tvm::tvm_eval_expr(expr, env))
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
        if !trimmed.starts_with("fun ") {
            i += 1;
            continue;
        }

        let after_keyword = &trimmed[4..];

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
            if after_paren.starts_with(':') {
                let after_colon = after_paren[1..].trim();
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
    if trimmed.starts_with("var ") || trimmed.starts_with("val ") {
        let after_keyword = &trimmed[4..];
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
    if trimmed.starts_with("return ") {
        let expr = trimmed[7..].trim().trim_end_matches(';').trim().to_string();
        if !expr.is_empty() {
            return Some(Statement::Return {
                expr,
                line: line_num,
            });
        }
    }

    None
}

/// Check if an expression is a simple function call like `compute()`.
/// Returns the function name if so.
fn parse_function_call(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if expr.ends_with("()") {
        let name = expr[..expr.len() - 2].trim();
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
