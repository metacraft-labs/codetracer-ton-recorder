//! Real TVM execution via the `tycho-vm` crate.
//!
//! This module compiles simple arithmetic expressions into TVM bytecode,
//! executes them on a real TON Virtual Machine, and returns the computed
//! results. Every value produced by this module comes from genuine TVM
//! stack operations -- nothing is hand-rolled.

use std::collections::HashMap;

use eyre::{eyre, Result};
use tycho_vm::{GasParams, NoLibraries, RcStackValue, VmState};

// ---------------------------------------------------------------------------
// TVM bytecode builder
// ---------------------------------------------------------------------------

/// Accumulates TVM opcodes and produces a Cell suitable for `VmState`.
struct TvmProgram {
    bytes: Vec<u8>,
}

impl TvmProgram {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// `PUSHINT n` -- push a 257-bit integer onto the stack.
    ///
    /// Encoding:
    /// - tiny (-5..=10): 0x7x where x = (n+5)&0xf
    /// - 8-bit signed:   0x80, i8
    /// - 16-bit signed:  0x81, i16 big-endian
    fn push_int(&mut self, n: i64) {
        if (-5..=10).contains(&n) {
            self.bytes.push(0x70 | (n as u8 & 0x0f));
        } else if (-128..=127).contains(&n) {
            self.bytes.push(0x80);
            self.bytes.push(n as i8 as u8);
        } else if (-32768..=32767).contains(&n) {
            self.bytes.push(0x81);
            let val = n as i16;
            self.bytes.push((val >> 8) as u8);
            self.bytes.push(val as u8);
        } else {
            // For values outside i16 range, use ADDINT/MULINT sequences
            // to build the value from smaller pieces.
            // This handles the full i64 range without needing bit-level
            // encoding of the long PUSHINT format.
            //
            // Strategy: push (n / 127), multiply by 127, add (n % 127).
            // This recurses until values fit in i16.
            let divisor = 127i64;
            let quotient = n / divisor;
            let remainder = n % divisor;
            self.push_int(quotient);
            self.push_int(divisor);
            self.bytes.push(0xa8); // MUL
            if remainder != 0 {
                self.push_int(remainder);
                self.bytes.push(0xa0); // ADD
            }
        }
    }

    /// `ADD` -- pop two integers, push their sum.
    fn add(&mut self) {
        self.bytes.push(0xa0);
    }

    /// `SUB` -- pop two integers, push first - second.
    fn sub(&mut self) {
        self.bytes.push(0xa1);
    }

    /// `MUL` -- pop two integers, push their product.
    fn mul(&mut self) {
        self.bytes.push(0xa8);
    }

    /// Build the program into a Cell for execution.
    fn build_cell(&self) -> Result<tycho_vm::__export::tycho_types::cell::Cell> {
        use tycho_vm::__export::tycho_types::cell::CellBuilder;

        let mut builder = CellBuilder::new();
        for &byte in &self.bytes {
            builder
                .store_u8(byte)
                .map_err(|e| eyre!("failed to store opcode byte: {e}"))?;
        }
        builder
            .build()
            .map_err(|e| eyre!("failed to build TVM code cell: {e}"))
    }
}

// ---------------------------------------------------------------------------
// TVM execution
// ---------------------------------------------------------------------------

/// Execute a TVM program (a Cell) and return the top-of-stack integer.
fn run_tvm_program(cell: tycho_vm::__export::tycho_types::cell::Cell) -> Result<i64> {
    let mut output = String::new();
    let mut vm = VmState::builder()
        .with_code(cell)
        .with_debug(&mut output)
        .with_gas(GasParams {
            max: 1_000_000,
            limit: 1_000_000,
            credit: 0,
            ..GasParams::getter()
        })
        .with_libraries(&NoLibraries)
        .build();

    let raw_result = vm.run();
    // vm.run() returns the bitwise-negated exit code.
    // Normal exit: QuitCont(0) sets exit_code = !0 = -1, so vm.run() = -1.
    // We check that !result == 0 (normal exit).
    let exit_code = !raw_result;
    if exit_code != 0 {
        return Err(eyre!(
            "TVM execution failed with exit code {exit_code} (raw: {raw_result}), debug: {output}"
        ));
    }

    // Read the top of stack.
    if vm.stack.items.is_empty() {
        return Err(eyre!("TVM stack is empty after execution"));
    }

    let top = &vm.stack.items[vm.stack.items.len() - 1];
    extract_int_from_stack_value(top)
}

/// Extract an i64 from a TVM stack value.
///
/// TVM stack values are trait objects wrapping BigInt. We format via
/// `display_list()` (which calls `Display` on the BigInt) and parse
/// the result back to i64.
fn extract_int_from_stack_value(value: &RcStackValue) -> Result<i64> {
    let display = format!("{}", value.display_list());
    display
        .trim()
        .parse::<i64>()
        .map_err(|e| eyre!("failed to parse TVM stack value '{}' as i64: {e}", display))
}

// ---------------------------------------------------------------------------
// Expression compilation and evaluation
// ---------------------------------------------------------------------------

/// Compile a simple arithmetic expression to TVM bytecode and execute it.
///
/// This replaces the hand-rolled `eval_simple_expr` with real TVM execution.
/// The expression is parsed into an AST and compiled to TVM instructions
/// (PUSHINT, ADD, MUL, SUB), then executed on the tycho-vm TVM.
///
/// Supports:
/// - Integer literals
/// - Boolean literals (true=1, false=0)
/// - Variable references (looked up in `known`)
/// - Binary operations: +, -, *, /, %
/// - Comparison operators: ==, !=, <, >, <=, >=
/// - Parenthesized sub-expressions
pub fn tvm_eval_expr(expr: &str, known: &HashMap<String, i64>) -> Option<i64> {
    tvm_eval_expr_checked(expr, known).ok().flatten()
}

/// Like [`tvm_eval_expr`], but distinguishes "expression could not be
/// parsed / used variables are not bound" (returns `Ok(None)`) from
/// "TVM execution failed" (returns `Err(msg)`).
///
/// The recorder uses this checked entry point to route real TVM
/// exceptions (e.g. divide-by-zero, integer overflow, gas exhaustion)
/// through `register_special_event(EventLogKind::Error,
/// "tvm_exception", &msg)` so they surface on the structured event
/// channel rather than being silently dropped by `.ok()`. Same shape
/// as Cairo 1.50 CairoPanic and Miden 1.56 miden_vm_error routing.
pub fn tvm_eval_expr_checked(expr: &str, known: &HashMap<String, i64>) -> Result<Option<i64>> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Ok(None);
    }

    // Parse the expression into an AST.  An unparseable expression is
    // a structural problem (no point routing it through TVM error);
    // return Ok(None) so the caller continues without an event.
    let ast = match parse_expr(expr, known) {
        Some(ast) => ast,
        None => return Ok(None),
    };

    // Compile AST to TVM bytecode.
    let mut program = TvmProgram::new();
    compile_ast(&ast, &mut program);

    // Build the cell and run.  These two paths CAN surface real TVM
    // errors (cell-builder overflow + VM exit-code != 0).  Propagate
    // the error string through Err so the recorder can register it
    // as a structured event.
    let cell = program.build_cell()?;
    let value = run_tvm_program(cell)?;
    Ok(Some(value))
}

/// Simple expression AST node.
#[derive(Debug, Clone)]
enum Expr {
    Literal(i64),
    BinOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Unary bitwise NOT (`~x`).  TVM `NOT` (0xb3) computes
    /// `-x - 1`, matching Tolk's signed-integer convention for
    /// bitwise negation.
    Not(Box<Expr>),
}

#[derive(Debug, Clone, Copy)]
enum BinOp {
    Add,
    Sub,
    Mul,
    // Division and modulo are handled via TVM's DIV/MOD opcodes
    // but for simplicity we'll handle them as well.
    Div,
    Mod,
    // Comparison operators return 0 or -1 (TVM convention) but we
    // convert to 0/1 for compatibility with the existing tests.
    Eq,
    Neq,
    Lt,
    Gt,
    Leq,
    Geq,
    // Bitwise operators (TVM logicops): AND/OR/XOR + LSHIFT/RSHIFT.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

/// Parse a simple expression string into an AST.
/// Variables are resolved from `known` at parse time (since TVM operates on
/// raw integers, not named variables).
fn parse_expr(expr: &str, known: &HashMap<String, i64>) -> Option<Expr> {
    let expr = expr.trim();
    if expr.is_empty() {
        return None;
    }

    // Handle parenthesized expression.
    if expr.starts_with('(') && expr.ends_with(')') {
        let inner = &expr[1..expr.len() - 1];
        let mut depth = 0i32;
        let mut balanced = true;
        for ch in inner.chars() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        balanced = false;
                        break;
                    }
                }
                _ => {}
            }
        }
        if balanced && depth == 0 {
            return parse_expr(inner, known);
        }
    }

    // Boolean literals.
    if expr == "true" {
        return Some(Expr::Literal(1));
    }
    if expr == "false" {
        return Some(Expr::Literal(0));
    }

    // Unary bitwise NOT: `~<expr>`.  Recognised here (after the
    // paren-balance pre-pass and literal lookups, before binary-op
    // dispatch) so a top-level `~a` parses as a unary node and never
    // reaches the binary-op scanners as a single-char operator.
    if let Some(rest) = expr.strip_prefix('~') {
        let inner = rest.trim();
        if !inner.is_empty() {
            if let Some(inner_ast) = parse_expr(inner, known) {
                return Some(Expr::Not(Box::new(inner_ast)));
            }
        }
    }

    // Try as integer literal.
    if let Ok(val) = expr.parse::<i64>() {
        return Some(Expr::Literal(val));
    }

    // Try as variable.
    if let Some(&val) = known.get(expr) {
        return Some(Expr::Literal(val));
    }

    // Bitwise OR (`|`) — lowest precedence so it binds last.
    // Right-to-left scan: the rightmost top-level `|` is the
    // top-level split point.  We avoid swallowing `||` (logical OR)
    // by checking the neighbouring char; Tolk doesn't currently use
    // `||` but the explicit guard keeps us forward-compatible.
    if let Some(pos) = find_top_level_single_bitwise(expr, '|') {
        let left = expr[..pos].trim();
        let right = expr[pos + 1..].trim();
        if !left.is_empty() && !right.is_empty() {
            let left_ast = parse_expr(left, known)?;
            let right_ast = parse_expr(right, known)?;
            return Some(Expr::BinOp {
                op: BinOp::BitOr,
                left: Box::new(left_ast),
                right: Box::new(right_ast),
            });
        }
    }

    // Bitwise XOR (`^`).
    if let Some(pos) = find_top_level_single_bitwise(expr, '^') {
        let left = expr[..pos].trim();
        let right = expr[pos + 1..].trim();
        if !left.is_empty() && !right.is_empty() {
            let left_ast = parse_expr(left, known)?;
            let right_ast = parse_expr(right, known)?;
            return Some(Expr::BinOp {
                op: BinOp::BitXor,
                left: Box::new(left_ast),
                right: Box::new(right_ast),
            });
        }
    }

    // Bitwise AND (`&`).  Same logical-AND-style guard against `&&`.
    if let Some(pos) = find_top_level_single_bitwise(expr, '&') {
        let left = expr[..pos].trim();
        let right = expr[pos + 1..].trim();
        if !left.is_empty() && !right.is_empty() {
            let left_ast = parse_expr(left, known)?;
            let right_ast = parse_expr(right, known)?;
            return Some(Expr::BinOp {
                op: BinOp::BitAnd,
                left: Box::new(left_ast),
                right: Box::new(right_ast),
            });
        }
    }

    // Try comparison operators (lower precedence than shifts): ==, !=, <=, >=
    for (op_str, op) in &[
        ("==", BinOp::Eq),
        ("!=", BinOp::Neq),
        ("<=", BinOp::Leq),
        (">=", BinOp::Geq),
    ] {
        if let Some(pos) = find_top_level_op(expr, op_str) {
            let left = expr[..pos].trim();
            let right = expr[pos + op_str.len()..].trim();
            if !left.is_empty() && !right.is_empty() {
                let left_ast = parse_expr(left, known)?;
                let right_ast = parse_expr(right, known)?;
                return Some(Expr::BinOp {
                    op: *op,
                    left: Box::new(left_ast),
                    right: Box::new(right_ast),
                });
            }
        }
    }

    // Bitwise shifts (`<<`, `>>`) — checked BEFORE single-char `<` /
    // `>` so a top-level `<<` doesn't get split as a comparison.
    for (op_str, op) in &[("<<", BinOp::Shl), (">>", BinOp::Shr)] {
        if let Some(pos) = find_top_level_op(expr, op_str) {
            let left = expr[..pos].trim();
            let right = expr[pos + op_str.len()..].trim();
            if !left.is_empty() && !right.is_empty() {
                let left_ast = parse_expr(left, known)?;
                let right_ast = parse_expr(right, known)?;
                return Some(Expr::BinOp {
                    op: *op,
                    left: Box::new(left_ast),
                    right: Box::new(right_ast),
                });
            }
        }
    }

    // Single-char < and > (after checking <=, >=, <<, >>).
    for (op_str, op) in &[("<", BinOp::Lt), (">", BinOp::Gt)] {
        if let Some(pos) = find_top_level_single_comparison(expr, op_str) {
            let left = expr[..pos].trim();
            let right = expr[pos + 1..].trim();
            if !left.is_empty() && !right.is_empty() {
                let left_ast = parse_expr(left, known)?;
                let right_ast = parse_expr(right, known)?;
                return Some(Expr::BinOp {
                    op: *op,
                    left: Box::new(left_ast),
                    right: Box::new(right_ast),
                });
            }
        }
    }

    // Addition and subtraction (lowest arithmetic precedence, right-to-left scan).
    {
        let mut depth = 0i32;
        let chars: Vec<char> = expr.chars().collect();
        for i in (0..chars.len()).rev() {
            match chars[i] {
                ')' => depth += 1,
                '(' => depth -= 1,
                '+' | '-' if depth == 0 && i > 0 => {
                    let left = expr[..i].trim();
                    let right = expr[i + 1..].trim();
                    if !left.is_empty() && !right.is_empty() {
                        let left_ast = parse_expr(left, known)?;
                        let right_ast = parse_expr(right, known)?;
                        let op = if chars[i] == '+' {
                            BinOp::Add
                        } else {
                            BinOp::Sub
                        };
                        return Some(Expr::BinOp {
                            op,
                            left: Box::new(left_ast),
                            right: Box::new(right_ast),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    // Multiplication, division, modulo (right-to-left scan).
    {
        let mut depth = 0i32;
        let chars: Vec<char> = expr.chars().collect();
        for i in (0..chars.len()).rev() {
            match chars[i] {
                ')' => depth += 1,
                '(' => depth -= 1,
                '*' | '/' | '%' if depth == 0 && i > 0 => {
                    let left = expr[..i].trim();
                    let right = expr[i + 1..].trim();
                    if !left.is_empty() && !right.is_empty() {
                        let left_ast = parse_expr(left, known)?;
                        let right_ast = parse_expr(right, known)?;
                        let op = match chars[i] {
                            '*' => BinOp::Mul,
                            '/' => BinOp::Div,
                            '%' => BinOp::Mod,
                            _ => unreachable!(),
                        };
                        return Some(Expr::BinOp {
                            op,
                            left: Box::new(left_ast),
                            right: Box::new(right_ast),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    None
}

/// Compile an AST node into TVM opcodes (post-order traversal).
fn compile_ast(expr: &Expr, program: &mut TvmProgram) {
    match expr {
        Expr::Literal(n) => {
            program.push_int(*n);
        }
        Expr::Not(inner) => {
            // TVM NOT (`b3`): pops x, pushes -x - 1 (the canonical
            // signed-integer bitwise complement).  Matches Tolk's
            // semantics for `~a` on an `int` operand.
            compile_ast(inner, program);
            program.bytes.push(0xb3);
        }
        Expr::BinOp { op, left, right } => {
            // Push operands in order (left first, then right).
            compile_ast(left, program);
            compile_ast(right, program);
            match op {
                BinOp::Add => program.add(),
                BinOp::Sub => program.sub(),
                BinOp::Mul => program.mul(),
                BinOp::Div => {
                    // TVM DIV opcode: 0xa904
                    program.bytes.push(0xa9);
                    program.bytes.push(0x04);
                }
                BinOp::Mod => {
                    // TVM MOD opcode: 0xa908
                    program.bytes.push(0xa9);
                    program.bytes.push(0x08);
                }
                BinOp::Eq => {
                    // EQUAL: 0xba
                    program.bytes.push(0xba);
                }
                BinOp::Neq => {
                    // NEQ: 0xbd
                    program.bytes.push(0xbd);
                }
                BinOp::Lt => {
                    // LESS: 0xb9
                    program.bytes.push(0xb9);
                }
                BinOp::Gt => {
                    // GREATER: 0xbc
                    program.bytes.push(0xbc);
                }
                BinOp::Leq => {
                    // LEQ: 0xbb
                    program.bytes.push(0xbb);
                }
                BinOp::Geq => {
                    // GEQ: 0xbe
                    program.bytes.push(0xbe);
                }
                // Bitwise binary ops (TVM `logicops`): AND/OR/XOR
                // are single-byte opcodes; the variable-shift forms
                // pop `count` then `x` and push the shifted result.
                BinOp::BitAnd => {
                    // AND: 0xb0
                    program.bytes.push(0xb0);
                }
                BinOp::BitOr => {
                    // OR: 0xb1
                    program.bytes.push(0xb1);
                }
                BinOp::BitXor => {
                    // XOR: 0xb2
                    program.bytes.push(0xb2);
                }
                BinOp::Shl => {
                    // LSHIFT (var): 0xac — pops y (count) and x,
                    // pushes x << y.
                    program.bytes.push(0xac);
                }
                BinOp::Shr => {
                    // RSHIFT (var): 0xad — pops y (count) and x,
                    // pushes x >> y (arithmetic shift).
                    program.bytes.push(0xad);
                }
            }
            // For comparison operators, TVM returns -1 (true) or 0 (false).
            // Our tests expect 1 for true. We need to negate the result.
            // TVM true = -1 = 0xFFFF...FF. We want 1.
            // NEGATE (-1 -> 1, 0 -> 0): use NEGATE opcode.
            match op {
                BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Leq | BinOp::Geq => {
                    // NEGATE: 0xa3
                    program.bytes.push(0xa3);
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parser helpers (reused from the original tracer.rs with minor changes)
// ---------------------------------------------------------------------------

/// Find a multi-character operator at the top level (depth 0) scanning right to left.
fn find_top_level_op(expr: &str, op: &str) -> Option<usize> {
    let chars: Vec<char> = expr.chars().collect();
    let op_chars: Vec<char> = op.chars().collect();
    let mut depth = 0i32;

    if chars.len() < op_chars.len() {
        return None;
    }

    for i in (0..=chars.len() - op_chars.len()).rev() {
        match chars[i] {
            ')' => depth += 1,
            '(' => depth -= 1,
            _ => {}
        }
        if depth == 0 && i > 0 && chars[i..i + op_chars.len()] == op_chars[..] {
            return Some(i);
        }
    }
    None
}

/// Find a single-character bitwise operator (`&`, `|`, `^`) at top
/// level, scanning right-to-left for the lowest-precedence binding.
/// Skips occurrences that are part of doubled operators (`&&`, `||`)
/// — Tolk doesn't currently use those, but the explicit guard keeps
/// the bitwise scanners forward-compatible with logical-operator
/// support arriving via a parser fast-path.
fn find_top_level_single_bitwise(expr: &str, op_char: char) -> Option<usize> {
    let chars: Vec<char> = expr.chars().collect();
    let mut depth = 0i32;
    for i in (0..chars.len()).rev() {
        match chars[i] {
            ')' => depth += 1,
            '(' => depth -= 1,
            _ => {}
        }
        if depth == 0 && i > 0 && chars[i] == op_char {
            // Skip doubled (`&&` / `||`) and the carat-doubled (`^^`)
            // shapes — none of these are real Tolk operators today,
            // but the guard is cheap and keeps the scanner robust.
            if i + 1 < chars.len() && chars[i + 1] == op_char {
                continue;
            }
            if chars[i - 1] == op_char {
                continue;
            }
            return Some(i);
        }
    }
    None
}

/// Find a single-char comparison operator (<, >) at top level,
/// ensuring it is not part of <=, >=, ==, or !=.
fn find_top_level_single_comparison(expr: &str, op: &str) -> Option<usize> {
    let chars: Vec<char> = expr.chars().collect();
    let op_char = op.chars().next()?;
    let mut depth = 0i32;

    for i in (0..chars.len()).rev() {
        match chars[i] {
            ')' => depth += 1,
            '(' => depth -= 1,
            _ => {}
        }
        if depth == 0 && i > 0 && chars[i] == op_char {
            // Make sure it is not part of <=, >=, ==, !=.
            if i + 1 < chars.len() && chars[i + 1] == '=' {
                continue;
            }
            if i > 0 && (chars[i - 1] == '!' || chars[i - 1] == '<' || chars[i - 1] == '>') {
                continue;
            }
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tvm_push_int() {
        // Small value: push 5 onto stack.
        let mut prog = TvmProgram::new();
        prog.push_int(5);
        let cell = prog.build_cell().unwrap();
        let result = run_tvm_program(cell).unwrap();
        assert_eq!(result, 5);
    }

    #[test]
    fn test_tvm_push_int_negative() {
        let mut prog = TvmProgram::new();
        prog.push_int(-3);
        let cell = prog.build_cell().unwrap();
        let result = run_tvm_program(cell).unwrap();
        assert_eq!(result, -3);
    }

    #[test]
    fn test_tvm_push_int_medium() {
        let mut prog = TvmProgram::new();
        prog.push_int(100);
        let cell = prog.build_cell().unwrap();
        let result = run_tvm_program(cell).unwrap();
        assert_eq!(result, 100);
    }

    #[test]
    fn test_tvm_stack_operations() {
        // Push 10 and 32, then ADD. Should get 42.
        let mut prog = TvmProgram::new();
        prog.push_int(10);
        prog.push_int(32);
        prog.add();
        let cell = prog.build_cell().unwrap();
        let result = run_tvm_program(cell).unwrap();
        assert_eq!(result, 42, "TVM ADD: 10 + 32 should be 42");
    }

    #[test]
    fn test_tvm_multiplication() {
        // Push 42 and 2, then MUL. Should get 84.
        let mut prog = TvmProgram::new();
        prog.push_int(42);
        prog.push_int(2);
        prog.mul();
        let cell = prog.build_cell().unwrap();
        let result = run_tvm_program(cell).unwrap();
        assert_eq!(result, 84, "TVM MUL: 42 * 2 should be 84");
    }

    #[test]
    fn test_tvm_full_computation() {
        // Compute the full chain: a=10, b=32, sum=a+b=42, doubled=sum*2=84,
        // final=doubled+a=94.
        // TVM program: PUSH 10, PUSH 32, ADD, PUSH 2, MUL, PUSH 10, ADD
        let known = HashMap::new();

        // Step 1: a + b = 10 + 32 = 42
        let sum = tvm_eval_expr("10 + 32", &known).unwrap();
        assert_eq!(sum, 42);

        // Step 2: sum * 2 = 42 * 2 = 84
        let mut known2 = HashMap::new();
        known2.insert("sum_val".to_string(), sum);
        let doubled = tvm_eval_expr("sum_val * 2", &known2).unwrap();
        assert_eq!(doubled, 84);

        // Step 3: doubled + a = 84 + 10 = 94
        let mut known3 = HashMap::new();
        known3.insert("doubled".to_string(), doubled);
        known3.insert("a".to_string(), 10);
        let final_result = tvm_eval_expr("doubled + a", &known3).unwrap();
        assert_eq!(final_result, 94);
    }

    #[test]
    fn test_tvm_eval_literal() {
        let known = HashMap::new();
        assert_eq!(tvm_eval_expr("10", &known), Some(10));
        assert_eq!(tvm_eval_expr("0", &known), Some(0));
        assert_eq!(tvm_eval_expr("-5", &known), Some(-5));
    }

    #[test]
    fn test_tvm_eval_variable() {
        let mut known = HashMap::new();
        known.insert("x".to_string(), 42);
        assert_eq!(tvm_eval_expr("x", &known), Some(42));
    }

    #[test]
    fn test_tvm_eval_binary_ops() {
        let mut known = HashMap::new();
        known.insert("a".to_string(), 10);
        known.insert("b".to_string(), 32);

        assert_eq!(tvm_eval_expr("a + b", &known), Some(42));
        assert_eq!(tvm_eval_expr("a * 2", &known), Some(20));
        assert_eq!(tvm_eval_expr("a % 3", &known), Some(1));
        assert_eq!(tvm_eval_expr("unknown", &known), None);
    }

    #[test]
    fn test_tvm_eval_comparisons() {
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
    fn test_tvm_eval_booleans() {
        let known = HashMap::new();
        assert_eq!(tvm_eval_expr("true", &known), Some(1));
        assert_eq!(tvm_eval_expr("false", &known), Some(0));
    }

    #[test]
    fn test_tvm_eval_chain() {
        let mut known = HashMap::new();
        known.insert("a".to_string(), 10);
        known.insert("b".to_string(), 32);
        known.insert("sum_val".to_string(), 42);
        known.insert("doubled".to_string(), 84);

        assert_eq!(tvm_eval_expr("a + b", &known), Some(42));
        assert_eq!(tvm_eval_expr("sum_val * 2", &known), Some(84));
        assert_eq!(tvm_eval_expr("doubled + a", &known), Some(94));
    }
}
