//! Symbolic stack tracker for TVM execution.
//!
//! The TVM is a stack machine, so source-level variable names are lost during
//! compilation. This module maintains a *symbolic stack* that mirrors the
//! real TVM stack but augments each entry with an optional variable name.
//! By processing the same sequence of opcodes (PUSH, ADD, SWAP, DUP, etc.)
//! symbolically, we can reconstruct which source-level variable (or derived
//! expression) each stack slot corresponds to.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single entry on the symbolic stack.
#[derive(Debug, Clone)]
pub struct StackEntry {
    /// The concrete integer value (mirrors the real TVM stack).
    pub value: i64,
    /// An optional symbolic name. `None` for anonymous intermediates.
    pub name: Option<String>,
}

/// Tracks symbolic names alongside a TVM stack.
#[derive(Debug, Clone)]
pub struct StackTracker {
    stack: Vec<StackEntry>,
}

/// Arithmetic / comparison operations that the tracker understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

impl ArithOp {
    /// Return the infix symbol used when deriving expression names.
    fn symbol(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
            ArithOp::Mod => "%",
        }
    }
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl StackTracker {
    /// Create a new, empty stack tracker.
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a value with an optional variable name.
    pub fn push(&mut self, value: i64, name: Option<String>) {
        self.stack.push(StackEntry { value, name });
    }

    /// Pop the top entry, returning `(value, name)`.
    ///
    /// Returns `None` if the stack is empty.
    pub fn pop(&mut self) -> Option<(i64, Option<String>)> {
        self.stack.pop().map(|e| (e.value, e.name))
    }

    /// Apply a binary arithmetic operation.
    ///
    /// Pops two operands (right first, then left -- matching TVM convention),
    /// computes the result, and pushes it with a derived name such as `"a + b"`.
    ///
    /// The `result_value` is the concrete result (typically obtained from the
    /// real TVM execution so we stay in sync).
    pub fn apply_arithmetic(&mut self, op: ArithOp, result_value: i64) {
        let right = self.pop();
        let left = self.pop();

        let derived_name = match (&left, &right) {
            (Some((left_val, left_name)), Some((right_val, right_name))) => {
                let left_str = left_name.clone().unwrap_or_else(|| left_val.to_string());
                let right_str = right_name.clone().unwrap_or_else(|| right_val.to_string());
                Some(format!("{} {} {}", left_str, op.symbol(), right_str))
            }
            _ => None,
        };

        self.push(result_value, derived_name);
    }

    /// Swap the entries at positions `i` and `j` (0-indexed from the top).
    ///
    /// This mirrors TVM's `XCHG s(i), s(j)` / `SWAP` instructions.
    pub fn apply_swap(&mut self, i: usize, j: usize) {
        let len = self.stack.len();
        if i >= len || j >= len {
            return;
        }
        // Positions are from the top: 0 = top, 1 = second-from-top, etc.
        let idx_i = len - 1 - i;
        let idx_j = len - 1 - j;
        self.stack.swap(idx_i, idx_j);
    }

    /// Duplicate the entry at position `i` (0-indexed from the top) and push
    /// the copy onto the stack.
    ///
    /// This mirrors TVM's `PUSH s(i)` (duplicate) instruction.
    pub fn apply_dup(&mut self, i: usize) {
        let len = self.stack.len();
        if i >= len {
            return;
        }
        let idx = len - 1 - i;
        let entry = self.stack[idx].clone();
        self.stack.push(entry);
    }

    /// Return all named entries currently on the stack as `(name, value)` pairs.
    ///
    /// Anonymous (unnamed) entries are omitted.
    pub fn variables_at_step(&self) -> Vec<(String, i64)> {
        self.stack
            .iter()
            .filter_map(|e| e.name.as_ref().map(|n| (n.clone(), e.value)))
            .collect()
    }

    /// Assign a name to the top-of-stack entry.
    ///
    /// This is used when the tracer knows that the result of an expression
    /// is being bound to a source-level variable (e.g. `var a: int = 10;`).
    pub fn name_top(&mut self, name: String) {
        if let Some(top) = self.stack.last_mut() {
            top.name = Some(name);
        }
    }

    /// Return the current stack depth.
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Look up a named variable on the stack.
    ///
    /// Returns the most-recently-pushed entry with that name (searching from
    /// the top), or `None` if no entry has that name.
    pub fn lookup(&self, name: &str) -> Option<i64> {
        self.stack
            .iter()
            .rev()
            .find(|e| e.name.as_deref() == Some(name))
            .map(|e| e.value)
    }
}

impl Default for StackTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Expression-level symbolic tracking
// ---------------------------------------------------------------------------

/// Build a `StackTracker` that reflects evaluating a simple arithmetic
/// expression, using `known` for variable resolution.
///
/// This is a convenience wrapper used by the tracer to get symbolic names
/// for the result of an expression evaluation.
pub fn track_expr(
    expr: &str,
    known_vars: &HashMap<String, i64>,
    result_value: i64,
) -> StackTracker {
    let expr = expr.trim();
    let mut tracker = StackTracker::new();

    // Try to decompose the expression into a symbolic result.
    // For a simple literal or variable reference, push it directly.
    if let Ok(_val) = expr.parse::<i64>() {
        tracker.push(result_value, None);
        return tracker;
    }

    if expr == "true" || expr == "false" {
        tracker.push(result_value, None);
        return tracker;
    }

    if known_vars.contains_key(expr) {
        tracker.push(result_value, Some(expr.to_string()));
        return tracker;
    }

    // For binary expressions like "a + b", try to decompose and track.
    if let Some((left, op, right)) = split_binary_expr(expr) {
        let left = left.trim();
        let right = right.trim();

        let left_name = if known_vars.contains_key(left) {
            Some(left.to_string())
        } else {
            None
        };
        let right_name = if known_vars.contains_key(right) {
            Some(right.to_string())
        } else {
            None
        };

        let left_val = resolve_operand(left, known_vars);
        let right_val = resolve_operand(right, known_vars);

        tracker.push(left_val, left_name);
        tracker.push(right_val, right_name);
        tracker.apply_arithmetic(op, result_value);
        return tracker;
    }

    // Fallback: just push the result with no name.
    tracker.push(result_value, None);
    tracker
}

/// Try to split a simple binary expression like "a + b" or "sum_val * 2".
fn split_binary_expr(expr: &str) -> Option<(&str, ArithOp, &str)> {
    // Scan right-to-left for +/- first (lower precedence).
    for (op_char, op) in &[('+', ArithOp::Add), ('-', ArithOp::Sub)] {
        let mut depth = 0i32;
        let chars: Vec<char> = expr.chars().collect();
        for i in (1..chars.len()).rev() {
            match chars[i] {
                ')' => depth += 1,
                '(' => depth -= 1,
                c if c == *op_char && depth == 0 => {
                    let left = &expr[..i];
                    let right = &expr[i + 1..];
                    if !left.trim().is_empty() && !right.trim().is_empty() {
                        return Some((left.trim(), *op, right.trim()));
                    }
                }
                _ => {}
            }
        }
    }
    // Then */% (higher precedence).
    for (op_char, op) in &[
        ('*', ArithOp::Mul),
        ('/', ArithOp::Div),
        ('%', ArithOp::Mod),
    ] {
        let mut depth = 0i32;
        let chars: Vec<char> = expr.chars().collect();
        for i in (1..chars.len()).rev() {
            match chars[i] {
                ')' => depth += 1,
                '(' => depth -= 1,
                c if c == *op_char && depth == 0 => {
                    let left = &expr[..i];
                    let right = &expr[i + 1..];
                    if !left.trim().is_empty() && !right.trim().is_empty() {
                        return Some((left.trim(), *op, right.trim()));
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Resolve an operand to its integer value.
fn resolve_operand(operand: &str, known: &HashMap<String, i64>) -> i64 {
    let operand = operand.trim();
    if let Ok(v) = operand.parse::<i64>() {
        return v;
    }
    if let Some(&v) = known.get(operand) {
        return v;
    }
    0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_pop() {
        let mut tracker = StackTracker::new();
        tracker.push(10, Some("a".to_string()));
        tracker.push(32, Some("b".to_string()));

        assert_eq!(tracker.depth(), 2);

        let (val, name) = tracker.pop().unwrap();
        assert_eq!(val, 32);
        assert_eq!(name.as_deref(), Some("b"));

        let (val, name) = tracker.pop().unwrap();
        assert_eq!(val, 10);
        assert_eq!(name.as_deref(), Some("a"));

        assert!(tracker.pop().is_none());
    }

    #[test]
    fn test_arithmetic_derive_name() {
        let mut tracker = StackTracker::new();
        tracker.push(10, Some("a".to_string()));
        tracker.push(32, Some("b".to_string()));
        tracker.apply_arithmetic(ArithOp::Add, 42);

        assert_eq!(tracker.depth(), 1);
        let vars = tracker.variables_at_step();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].0, "a + b");
        assert_eq!(vars[0].1, 42);
    }

    #[test]
    fn test_arithmetic_with_literal() {
        let mut tracker = StackTracker::new();
        tracker.push(42, Some("sum_val".to_string()));
        tracker.push(2, None);
        tracker.apply_arithmetic(ArithOp::Mul, 84);

        let vars = tracker.variables_at_step();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].0, "sum_val * 2");
        assert_eq!(vars[0].1, 84);
    }

    #[test]
    fn test_swap() {
        let mut tracker = StackTracker::new();
        tracker.push(10, Some("a".to_string()));
        tracker.push(32, Some("b".to_string()));

        // Swap top two entries (positions 0 and 1).
        tracker.apply_swap(0, 1);

        let (val, name) = tracker.pop().unwrap();
        assert_eq!(val, 10);
        assert_eq!(name.as_deref(), Some("a"));

        let (val, name) = tracker.pop().unwrap();
        assert_eq!(val, 32);
        assert_eq!(name.as_deref(), Some("b"));
    }

    #[test]
    fn test_dup() {
        let mut tracker = StackTracker::new();
        tracker.push(10, Some("a".to_string()));
        tracker.push(32, Some("b".to_string()));

        // Duplicate the second-from-top (position 1 = "a").
        tracker.apply_dup(1);

        assert_eq!(tracker.depth(), 3);
        let (val, name) = tracker.pop().unwrap();
        assert_eq!(val, 10);
        assert_eq!(name.as_deref(), Some("a"));
    }

    #[test]
    fn test_variables_at_step() {
        let mut tracker = StackTracker::new();
        tracker.push(10, Some("a".to_string()));
        tracker.push(32, Some("b".to_string()));
        tracker.push(99, None); // anonymous

        let vars = tracker.variables_at_step();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0], ("a".to_string(), 10));
        assert_eq!(vars[1], ("b".to_string(), 32));
    }

    #[test]
    fn test_name_top() {
        let mut tracker = StackTracker::new();
        tracker.push(42, None);
        tracker.name_top("sum_val".to_string());

        let vars = tracker.variables_at_step();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0], ("sum_val".to_string(), 42));
    }

    #[test]
    fn test_lookup() {
        let mut tracker = StackTracker::new();
        tracker.push(10, Some("a".to_string()));
        tracker.push(32, Some("b".to_string()));

        assert_eq!(tracker.lookup("a"), Some(10));
        assert_eq!(tracker.lookup("b"), Some(32));
        assert_eq!(tracker.lookup("c"), None);
    }

    #[test]
    fn test_track_expr_literal() {
        let known = HashMap::new();
        let tracker = track_expr("10", &known, 10);
        // Literal push: no symbolic name.
        assert_eq!(tracker.depth(), 1);
        let vars = tracker.variables_at_step();
        assert!(vars.is_empty()); // literals have no name
    }

    #[test]
    fn test_track_expr_variable() {
        let mut known = HashMap::new();
        known.insert("a".to_string(), 10);
        let tracker = track_expr("a", &known, 10);
        let vars = tracker.variables_at_step();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0], ("a".to_string(), 10));
    }

    #[test]
    fn test_track_expr_binary() {
        let mut known = HashMap::new();
        known.insert("a".to_string(), 10);
        known.insert("b".to_string(), 32);

        let tracker = track_expr("a + b", &known, 42);
        let vars = tracker.variables_at_step();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].0, "a + b");
        assert_eq!(vars[0].1, 42);
    }

    #[test]
    fn test_track_expr_mul_with_literal() {
        let mut known = HashMap::new();
        known.insert("sum_val".to_string(), 42);

        let tracker = track_expr("sum_val * 2", &known, 84);
        let vars = tracker.variables_at_step();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].0, "sum_val * 2");
        assert_eq!(vars[0].1, 84);
    }

    #[test]
    fn test_full_flow_test_symbolic_tracking() {
        // Simulate the flow_test.tolk program:
        //   var a: int = 10;
        //   var b: int = 32;
        //   var sum_val: int = a + b;
        //   var doubled: int = sum_val * 2;
        //   var final_result: int = doubled + a;
        //   return final_result;

        let mut env: HashMap<String, i64> = HashMap::new();
        let mut all_vars: Vec<(String, i64)> = Vec::new();

        // var a = 10
        let val = 10i64;
        env.insert("a".to_string(), val);
        all_vars.push(("a".to_string(), val));

        // var b = 32
        let val = 32i64;
        env.insert("b".to_string(), val);
        all_vars.push(("b".to_string(), val));

        // var sum_val = a + b
        let tracker = track_expr("a + b", &env, 42);
        let vars = tracker.variables_at_step();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].0, "a + b");
        assert_eq!(vars[0].1, 42);
        env.insert("sum_val".to_string(), 42);
        all_vars.push(("sum_val".to_string(), 42));

        // var doubled = sum_val * 2
        let tracker = track_expr("sum_val * 2", &env, 84);
        let vars = tracker.variables_at_step();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].0, "sum_val * 2");
        assert_eq!(vars[0].1, 84);
        env.insert("doubled".to_string(), 84);
        all_vars.push(("doubled".to_string(), 84));

        // var final_result = doubled + a
        let tracker = track_expr("doubled + a", &env, 94);
        let vars = tracker.variables_at_step();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].0, "doubled + a");
        assert_eq!(vars[0].1, 94);
        env.insert("final_result".to_string(), 94);
        all_vars.push(("final_result".to_string(), 94));

        // Verify all expected variable values.
        assert_eq!(all_vars.len(), 5);
        assert_eq!(all_vars[0], ("a".to_string(), 10));
        assert_eq!(all_vars[1], ("b".to_string(), 32));
        assert_eq!(all_vars[2], ("sum_val".to_string(), 42));
        assert_eq!(all_vars[3], ("doubled".to_string(), 84));
        assert_eq!(all_vars[4], ("final_result".to_string(), 94));
    }
}
