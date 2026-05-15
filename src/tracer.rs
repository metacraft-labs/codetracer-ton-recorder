//! Tracer implementation for Tolk/TON programs.
//!
//! Parses a Tolk source file to extract function definitions, variable
//! declarations (var/val), assignments, and return statements, evaluates
//! them via a real TVM (using `tycho-vm`), and emits CodeTracer trace
//! events (steps, calls, returns, variables).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use codetracer_trace_types::{EventLogKind, Line, TypeId, TypeKind, ValueRecord, NONE_VALUE};
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
    /// `@method_id(N)` value when the function carries one, otherwise
    /// `None`.  Real-world Tolk contracts attach `@method_id` to
    /// every getter so the on-chain dispatch table can route requests
    /// by method id; the recorder preserves it on the function entry
    /// so future channels (calltrace metadata, frontend filters) can
    /// surface it without re-parsing the source.
    method_id: Option<i64>,
    /// Absolute path to the source file this function was parsed
    /// from.  Cross-file fixtures (`import "helpers.tolk";`) carry
    /// the imported file's path here so step events emitted from the
    /// imported helper land under the correct source path on the
    /// frontend's source pane.  See `imports_test.tolk` for the
    /// canonical fixture.
    source_path: PathBuf,
}

/// A parsed statement in a Tolk function body.
#[derive(Debug, Clone)]
enum Statement {
    /// `var <name>[: <type>] = <expr>;` or `val <name>[: <type>] = <expr>;`
    ///
    /// `type_name` is `None` for the typeless `var b = beginCell();`
    /// shape that Tolk allows for Cell / Slice / Builder bindings —
    /// the recorder defaults the on-trace type id to `int` so these
    /// continue to round-trip through `register_variable_with_full_value`
    /// even when no annotation is present.
    VarBinding {
        name: String,
        type_name: Option<String>,
        expr: String,
        line: u32,
    },
    /// `<name> = <expr>;` — bare assignment to an already-bound
    /// variable.  Required for loop bodies like `total = total + i;`
    /// and branch-arm rebindings like `sign = -1;`.
    Assign {
        name: String,
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
    /// `if (cond) { ... } [else if (cond) { ... }]* [else { ... }]?`
    ///
    /// The recorder's brace-tracking parser collapses
    /// `else if (cond) { ... }` into a chained `If` whose `else_block`
    /// is a single-element `Vec<Statement>` containing another `If`.
    /// `then_block` always exists; `else_block` is empty when no
    /// `else` arm is present.
    If {
        cond: String,
        then_block: Vec<Statement>,
        else_block: Vec<Statement>,
        line: u32,
    },
    /// `while (cond) { ... }` — top-tested loop.
    While {
        cond: String,
        body: Vec<Statement>,
        line: u32,
    },
    /// `repeat (count) { ... }` — fixed-count loop.  `count_expr` is
    /// evaluated once before the loop starts.
    Repeat {
        count_expr: String,
        body: Vec<Statement>,
        line: u32,
    },
    /// `do { ... } until (cond);` — bottom-tested loop.  The body is
    /// always executed at least once; `cond` is checked after each
    /// iteration and the loop exits when it evaluates truthy.
    DoUntil {
        body: Vec<Statement>,
        cond: String,
        line: u32,
    },
    /// `<callee>(...);` invoked as a statement (return value
    /// discarded).  Used for storage-mutation calls like
    /// `set_data(c);`.
    ExprStatement { expr: String, line: u32 },
    /// `throwIf(<code>, <cond>);` / `throwUnless(<code>, <cond>);`
    /// — Tolk's pervasive guard idiom.  Unlike the unconditional
    /// `Throw` / static-sweep `Assert` variants, these MUST evaluate
    /// the condition at runtime and only fire the Error io_event
    /// when the gate actually trips (the `mode` field selects which
    /// truth value trips it).  Implemented inline in
    /// `execute_statement`; deliberately NOT walked by
    /// `emit_error_events_for_program` so a non-tripping gate does
    /// not produce a spurious io_event the way the static `throw` /
    /// `assert` sweep does for unreachable failure markers.
    GatedThrow {
        /// Raw exception-code expression (e.g. `"40"`, `"36"`).
        code: String,
        /// Raw condition expression (e.g. `"value == 0"`,
        /// `"probe >= 7"`).
        condition: String,
        /// Selects whether the gate trips on a true or false
        /// condition.  `Mode::If` = throw when cond is truthy
        /// (TON's `throwIf`); `Mode::Unless` = throw when cond is
        /// falsy (TON's `throwUnless`).
        mode: GateMode,
        line: u32,
    },
    /// `match (scrutinee) { Constructor => { ... }; Constructor(payload)
    /// => { ... }; ... }` — Tolk's pattern-matching dispatch over a
    /// user-defined sum type.  At runtime we evaluate the scrutinee
    /// to a `Value::Struct` (whose `type_name` is the variant's
    /// constructor name), pick the arm whose `constructor` matches
    /// (NOT a lexical last-arm-wins fallback), bind any
    /// `payload_binding` to the variant's first field, and execute
    /// that arm's body in the surrounding `env`.  See
    /// `match_test.tolk` for the canonical fixture.
    Match {
        scrutinee: String,
        arms: Vec<MatchArm>,
        line: u32,
    },
}

/// Selector for `Statement::GatedThrow` — see its doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateMode {
    /// `throwIf(<code>, <cond>);` — trip when cond is truthy.
    If,
    /// `throwUnless(<code>, <cond>);` — trip when cond is falsy.
    Unless,
}

/// A single arm of a `match` statement.  `constructor` is the
/// variant tag (e.g. `Pending`, `Active`); `payload_binding` is the
/// optional local-name binder for the variant's first field
/// (`Active(n)` binds `n` to the variant's first field for the arm
/// body); `body` is the block that runs when this arm fires.
#[derive(Debug, Clone)]
struct MatchArm {
    constructor: String,
    payload_binding: Option<String>,
    body: Vec<Statement>,
    #[allow(dead_code)]
    line: u32,
}

/// A run-time Tolk value flowing through the hand-rolled evaluator.
///
/// The recorder grew out of an int-only proof-of-concept where the
/// evaluation env was `HashMap<String, i64>`.  That was enough for
/// the pure-arithmetic fixtures (`flow_test.tolk`, `nested_calls_test.tolk`,
/// `control_flow_test.tolk`), but `tuples_structs_test.tolk` exercises
/// two structured shapes — tuple literal `(10, 20)` and struct literal
/// `Point { x: 3, y: 4 }` — that the trace MUST surface as
/// `ValueRecord::Tuple` / `ValueRecord::Struct` per
/// `metacraft-specs/policies/recorder-test-requirements.md`.
///
/// `Value` is the smallest superset that lets the same env carry both
/// scalars (for the existing TVM arithmetic pipeline) and the new
/// structured shapes.  Conversion to `ValueRecord` happens at the
/// `register_variable_with_full_value` boundary; conversion back to
/// `i64` (for `tvm_eval_expr_checked`) happens via `Value::as_i64`
/// (used to project the env down to the int sub-env).
#[derive(Debug, Clone)]
enum Value {
    Int(i64),
    /// Tolk tuple literal — emitted as `ValueRecord::Tuple`.
    Tuple(Vec<Value>),
    /// Tolk struct literal — emitted as `ValueRecord::Struct`.  The
    /// type name is needed so we can `ensure_type_id` the right
    /// `TypeKind::Struct`; field names are kept for `p.field` access.
    Struct {
        type_name: String,
        fields: Vec<(String, Value)>,
    },
    /// In-progress TON Builder.  `payload` records the integer
    /// arguments handed to `storeInt` / `storeUint` / `storeAddress`
    /// calls in the order they were appended; the Slice produced by
    /// `cell.beginParse()` consumes them via `loadInt` /
    /// `loadAddress` in the same order.  `refs` is a parallel queue
    /// of sub-cells appended by `storeRef` / `storeMaybeRef`;
    /// `loadRef` / `loadMaybeRef` pop from the front via a separate
    /// cursor maintained on the Slice side.  This is a recorder-side
    /// abstraction — no real cell bits are constructed — but it's
    /// sufficient to round-trip the values used in
    /// `cell_ops_test.tolk` and `builder_refs_test.tolk`.
    Builder {
        payload: Vec<i64>,
        refs: Vec<Vec<i64>>,
    },
    /// Finalised cell, produced by `Builder::endCell` or a chained
    /// `beginCell().storeInt(...).endCell()` expression.
    Cell {
        payload: Vec<i64>,
        refs: Vec<Vec<i64>>,
    },
    /// Read-cursor slice produced by `Cell::beginParse`.  `payload`
    /// is the same data the originating cell carried; `pos` advances
    /// as `loadInt(N)` calls consume entries.  `refs` is the
    /// parallel ref queue inherited from the cell; `ref_pos`
    /// advances as `loadRef` / `loadMaybeRef` consume sub-cells.
    Slice {
        payload: Vec<i64>,
        pos: usize,
        refs: Vec<Vec<i64>>,
        ref_pos: usize,
    },
    /// A `slice` produced from a Tolk source-level string literal —
    /// either UTF-8 text (`"abc"`) or hex bytes (`0x"abcd"`).  The
    /// recorder keeps the encoding tag so the resulting
    /// `ValueRecord::Raw` payload can render `Slice<text> ...`
    /// vs `Slice<hex> ...` and downstream consumers can tell the
    /// two literal flavours apart.  See `string_literals_test.tolk`
    /// for the canonical fixture.
    SliceLit {
        encoding: SliceEncoding,
        bytes: Vec<u8>,
    },
    /// Tolk's `coin` domain type — a tagged Int whose semantic
    /// distinction from a plain `int` is preserved at the
    /// `ValueRecord` boundary via a dedicated `coin` `type_id`.
    /// See `address_coins_test.tolk` and `tvm_primitives_test.tolk`
    /// for the canonical fixtures: `coin` arises both from formal
    /// parameter declarations (`fun ...(myBalance: coin, ...)`) and
    /// from the TVM context primitive `myBalance()`.  The underlying
    /// `i64` flows through `as_i64` so coin participates in the
    /// arithmetic pipeline transparently.
    Coin(i64),
    /// Tolk's `address` domain type — TON's account address modelled
    /// as a (workchain, hash) pair.  Surfaces as a structured
    /// `ValueRecord::Struct` with two named fields so frontend /
    /// address-decoder consumers can render the pair as
    /// `<wc>:<hex-hash>`.  Arises from formal parameter declarations
    /// (`fun ...(sender: address)`), from the TVM context primitives
    /// `msgSender()` / `myAddress()`, and from `var x: address =
    /// slice.loadAddress();` annotations that drive the recorder's
    /// int-to-Address lift.
    Address {
        workchain: i64,
        hash: i64,
    },
}

/// Encoding selector for `Value::SliceLit`.  Tolk's source-level
/// surface is `"abc"` (text) and `0x"abcd"` (hex); both lift to the
/// same wire-level `slice` value but the recorder must surface the
/// distinction so the trace pins each literal flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SliceEncoding {
    /// `"abc"` — UTF-8 codepoints, byte-for-byte.
    Text,
    /// `0x"abcd"` — hex nibbles parsed into the raw byte stream.
    Hex,
}

impl Value {
    /// Project to `i64` for the TVM arithmetic pipeline.  Only the
    /// `Int` variant has a meaningful answer; structured values can't
    /// be substituted into a TVM PUSHINT/ADD/MUL/... program.
    fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            // `coin` is a tagged Int; it round-trips through the TVM
            // arithmetic pipeline transparently so `myBalance +
            // msgValue` (both `coin`) computes a meaningful sum.
            Value::Coin(i) => Some(*i),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// The main tracer
// ---------------------------------------------------------------------------

/// The main tracer struct that captures Tolk execution traces.
pub struct TolkTracer {
    writer: Box<dyn TraceWriter + Send>,
    /// Registered type IDs for Tolk types.
    type_ids: HashMap<String, codetracer_trace_types::TypeId>,
    /// In-memory shadow of the on-chain persistent storage cell, set
    /// via `set_data(c)` and re-fetched via `get_data()`.  This is a
    /// recorder-side abstraction — no real TVM persistent storage is
    /// touched — but it is sufficient to round-trip the values used
    /// in `cell_ops_test.tolk`.  A `None` value means storage hasn't
    /// been written yet (the recorder still synthesises an empty
    /// `Cell` on read so downstream code doesn't blow up).
    storage_data: Option<Value>,
    /// Set of struct names that participate in a `type X = A | B | C;`
    /// union declaration.  When one of these names appears as a
    /// constructor (`Pending { ... }`, `Active { n: 7 }`), the value
    /// is emitted as `ValueRecord::Variant` with the name as the
    /// discriminator instead of as a plain `ValueRecord::Struct`.
    /// Pre-populated by `parse_variant_constructors` at trace start.
    variant_constructors: std::collections::HashSet<String>,
    /// Verbatim entry-file source text.  Retained so the recorder
    /// can scan for source-level directives (currently only the
    /// `// @recorder:messages ...` multi-message dispatcher hint
    /// consumed by `evaluate_program`).  See
    /// `parse_multi_message_directive` for the parser.
    source_text: Option<String>,
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
        // -- 1. Parse the Tolk source.  Cross-file fixtures use
        // `import "<path>";` to pull in helper modules; those are
        // loaded recursively (each imported file is parsed once,
        // tagged with its own source path, and merged into the
        // function pool).  See `imports_test.tolk` for the canonical
        // multi-file fixture.
        let _source_map = SourceMap::from_source(source_path, source_code);
        let mut functions = parse_functions(source_code, source_path);
        let mut already_imported: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::new();
        already_imported.insert(source_path.to_path_buf());
        let mut imported_sources: Vec<String> = Vec::new();
        load_imports_recursive(
            source_path,
            source_code,
            &mut functions,
            &mut already_imported,
            &mut imported_sources,
        );

        eprintln!("Parsed {} functions", functions.len());

        // -- 2. Create the trace writer (CTFS only) --
        let program_str = source_path.to_string_lossy();
        // Collect variant-constructor names from BOTH the entry file
        // and every imported file.  This keeps the
        // `Pending`/`Active`/`Failed` lift to `ValueRecord::Variant`
        // working when the union declaration lives in an imported
        // helper module.
        let mut variant_ctors = parse_variant_constructors(source_code);
        for src in &imported_sources {
            for name in parse_variant_constructors(src) {
                variant_ctors.insert(name);
            }
        }
        let mut tracer = TolkTracer {
            writer: create_trace_writer(&program_str, &[], CTFS_FORMAT),
            type_ids: HashMap::new(),
            storage_data: None,
            variant_constructors: variant_ctors,
            source_text: Some(source_code.to_string()),
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
        // Pre-register the generic structured-value type names used by
        // the literal-emitting paths.  Per-struct-type names (e.g.
        // "Point") are registered lazily in `value_to_record` the
        // first time a literal of that shape lands in the trace.
        let tuple_type_id =
            TraceWriter::ensure_type_id(&mut *tracer.writer, TypeKind::Seq, "Tuple");
        tracer.type_ids.insert("Tuple".to_string(), tuple_type_id);

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

    /// Evaluate the program starting from its entry point.
    ///
    /// Tolk's actual on-chain entry points are
    /// `onInternalMessage(...)` (called when the contract receives an
    /// internal message from another contract) and
    /// `onExternalMessage(...)` (called when the contract receives an
    /// external message from outside the chain).  Real-world contracts
    /// don't declare a `main()` — the TVM invokes whichever hook is
    /// appropriate for the incoming message.  The legacy hand-rolled
    /// fixtures here still use `main()` as a synthetic dispatch point,
    /// so we keep that resolver and only fall back to the TON
    /// entry-point hooks when no `main` is declared.  When BOTH
    /// hooks are present we evaluate them in the canonical order
    /// (internal-before-external) so the trace shape is deterministic.
    fn evaluate_program(&mut self, source_path: &Path, functions: &[FunctionDef]) -> Result<()> {
        // Build a function lookup table.
        let func_map: HashMap<String, &FunctionDef> =
            functions.iter().map(|f| (f.name.clone(), f)).collect();

        let mut env: HashMap<String, Value> = HashMap::new();

        // Preferred entry: `main()`, used by the legacy linear
        // fixtures.  Merged into <toplevel> (is_entry_point=true) so
        // its body runs at depth 0 — see the comment in the original
        // arm for the navigation rationale.
        if let Some(main_fn) = func_map.get("main") {
            self.evaluate_function(source_path, main_fn, &func_map, &mut env, true, &[])?;
            return Ok(());
        }

        // No main() declared: fall back to TON's actual entry points.
        // Tolk contracts may declare either or both of these hooks;
        // we drive each one in canonical order with synthetic int
        // arguments (0 for every formal) so the body executes and
        // surfaces step + var events.  Bigger fixtures with rich
        // entry-point signatures can extend this stub once the
        // recorder learns to synthesise representative message
        // payloads.
        let entry_names = ["onInternalMessage", "onExternalMessage"];
        let present: Vec<&str> = entry_names
            .iter()
            .copied()
            .filter(|n| func_map.contains_key(*n))
            .collect();

        if present.is_empty() {
            return Err(eyre!(
                "no entry point found in Tolk program: declare \
                 `main()`, `onInternalMessage(...)`, or \
                 `onExternalMessage(...)`"
            ));
        }

        for (idx, name) in present.iter().enumerate() {
            let entry_fn = func_map.get(*name).expect("present-filter");
            // Honour the declared parameter type when synthesising
            // each actual: `coin` / `address` / `cell` / `slice` lift
            // to representative TVM-context values so the body's
            // `loadAddress()` / `loadInt()` / `myBalance + msgValue`
            // arithmetic can compute meaningful results.  See
            // `synthesize_entry_arg` for the per-type schedule (and
            // the per-message override used by the multi-message
            // dispatcher below).
            let synthetic_args: Vec<Value> = entry_fn
                .params
                .iter()
                .map(|(_, ty)| synthesize_entry_arg(ty, None))
                .collect();
            // First entry merges into <toplevel> to keep its body at
            // depth 0 (same constraint as `main()`); any subsequent
            // entry runs as a non-entry-point call so its body opens
            // a fresh call_entry / call_exit pair.
            let is_first = idx == 0;

            // Multi-message dispatcher: opt-in via a source-level
            // `// @recorder:messages 1 2 3` directive.  When present
            // (and only on the `onInternalMessage` arm), the recorder
            // re-invokes the handler once per op_code, threading each
            // op_code into the synthesised `msgBody: slice` payload
            // so the body's `msgBody.loadInt(32)` dispatch routes
            // through a different branch on each call.  Each repeat
            // runs as a non-entry-point call so its body opens a
            // fresh call_entry / call_exit pair, producing one trace
            // segment per op_code.  Without the directive (the
            // common case) the handler runs once with the default
            // synthetic args, exactly as before.
            if *name == "onInternalMessage" {
                if let Some(op_codes) = parse_multi_message_directive(self.source_text.as_deref()) {
                    if !op_codes.is_empty() {
                        for (msg_idx, op) in op_codes.iter().enumerate() {
                            let args: Vec<Value> = entry_fn
                                .params
                                .iter()
                                .map(|(_, ty)| synthesize_entry_arg(ty, Some(*op)))
                                .collect();
                            // First message merges into <toplevel>
                            // (same constraint as the single-shot
                            // path); subsequent messages run as
                            // regular non-entry calls.
                            let merge_into_toplevel = is_first && msg_idx == 0;
                            self.evaluate_function(
                                source_path,
                                entry_fn,
                                &func_map,
                                &mut env,
                                merge_into_toplevel,
                                &args,
                            )?;
                        }
                        continue;
                    }
                }
            }

            self.evaluate_function(
                source_path,
                entry_fn,
                &func_map,
                &mut env,
                is_first,
                &synthetic_args,
            )?;
        }

        Ok(())
    }

    /// Convert a structured `Value` to its on-trace `ValueRecord`
    /// shape and register the necessary type ids on first use.
    ///
    /// `Int` → `ValueRecord::Int { type_id: type_ids["int"] }`.
    /// `Tuple` → `ValueRecord::Tuple { type_id: type_ids["Tuple"] }`.
    /// `Struct { type_name }` → `ValueRecord::Struct { type_id: type_ids[type_name] }`,
    ///   lazily registering `type_name` as `TypeKind::Struct` the first
    ///   time it's seen.  Field names are dropped at the
    ///   `ValueRecord::Struct` boundary (the wire format only carries
    ///   `field_values: Vec<ValueRecord>`); the per-type
    ///   `TypeSpecificInfo::Struct { fields }` registration that
    ///   carries the names is handled inside the Nim writer.
    fn value_to_record(&mut self, val: &Value) -> ValueRecord {
        match val {
            Value::Int(i) => {
                let type_id = self.type_ids.get("int").copied().unwrap_or(TypeId(0));
                ValueRecord::Int { i: *i, type_id }
            }
            Value::Tuple(elements) => {
                let elements: Vec<ValueRecord> =
                    elements.iter().map(|v| self.value_to_record(v)).collect();
                let type_id = self.type_ids.get("Tuple").copied().unwrap_or(TypeId(0));
                ValueRecord::Tuple { elements, type_id }
            }
            Value::Struct { type_name, fields } => {
                let field_values: Vec<ValueRecord> = fields
                    .iter()
                    .map(|(_n, v)| self.value_to_record(v))
                    .collect();
                let type_id = if let Some(id) = self.type_ids.get(type_name).copied() {
                    id
                } else {
                    let id =
                        TraceWriter::ensure_type_id(&mut *self.writer, TypeKind::Struct, type_name);
                    self.type_ids.insert(type_name.clone(), id);
                    id
                };
                // If `type_name` was registered as a variant
                // constructor (via a `type X = A | B | C;` union
                // declaration scanned at trace start), wrap the
                // struct contents in a `ValueRecord::Variant` whose
                // discriminator carries the constructor name.  The
                // contents `Struct { ... }` round-trips the field
                // tuple unchanged so the downstream consumer sees
                // both the variant tag AND the per-constructor data.
                let struct_record = ValueRecord::Struct {
                    field_values,
                    type_id,
                };
                if self.variant_constructors.contains(type_name) {
                    ValueRecord::Variant {
                        discriminator: type_name.clone(),
                        contents: Box::new(struct_record),
                        type_id,
                    }
                } else {
                    struct_record
                }
            }
            // TON Builder / Cell / Slice surface as `ValueRecord::Raw`
            // with a human-readable payload summary.  Each of the
            // three names is registered lazily as `TypeKind::Raw` so
            // the trace carries a stable `type_id` per shape and the
            // ct-print decode produces `kind: "Raw"` rows that the
            // strict test in `tests/test_tracer.rs` asserts on.  We
            // keep the payload surface minimal (the ordered int list
            // round-tripped through `storeInt` / `loadInt`) — the
            // recorder doesn't model the bit-level cell wire format,
            // but the int round-trip is what `cell_ops_test.tolk`
            // exercises and the only payload downstream tools render
            // today.
            Value::Builder { payload, refs } => {
                let type_id = self.ensure_raw_type_id("TolkBuilder");
                ValueRecord::Raw {
                    r: format_payload_with_refs("Builder", payload, refs),
                    type_id,
                }
            }
            Value::Cell { payload, refs } => {
                let type_id = self.ensure_raw_type_id("TolkCell");
                ValueRecord::Raw {
                    r: format_payload_with_refs("Cell", payload, refs),
                    type_id,
                }
            }
            Value::Slice {
                payload,
                pos,
                refs,
                ref_pos,
            } => {
                let type_id = self.ensure_raw_type_id("TolkSlice");
                let mut text = format_payload_with_refs("Slice", payload, refs);
                text.push_str(&format!(" @{pos}/r{ref_pos}"));
                ValueRecord::Raw { r: text, type_id }
            }
            // Tolk's `coin` domain type — surfaces as a tagged Int.
            // The integer value flows through unchanged; what changes
            // vs the plain `Int` arm is the `type_id`, which resolves
            // to the lazily-registered `coin` slot (`TypeKind::Int`,
            // separate type_id) so downstream consumers can route on
            // the type before formatting the value as nanoton / TON.
            Value::Coin(i) => {
                let type_id = self.ensure_int_alias_type_id("coin");
                ValueRecord::Int { i: *i, type_id }
            }
            // Tolk's `address` domain type — surfaces as a structured
            // `ValueRecord::Struct` with two fields (`workchain: int`,
            // `hash: int`).  The struct type registers lazily under
            // the canonical name `TolkAddress`; field values use the
            // generic `int` type_id so they decode as plain integers
            // on the consumer side.
            Value::Address { workchain, hash } => {
                let type_id = if let Some(id) = self.type_ids.get("TolkAddress").copied() {
                    id
                } else {
                    let id = TraceWriter::ensure_type_id(
                        &mut *self.writer,
                        TypeKind::Struct,
                        "TolkAddress",
                    );
                    self.type_ids.insert("TolkAddress".to_string(), id);
                    id
                };
                let int_type_id = self.type_ids.get("int").copied().unwrap_or(TypeId(0));
                let field_values = vec![
                    ValueRecord::Int {
                        i: *workchain,
                        type_id: int_type_id,
                    },
                    ValueRecord::Int {
                        i: *hash,
                        type_id: int_type_id,
                    },
                ];
                ValueRecord::Struct {
                    field_values,
                    type_id,
                }
            }
            Value::SliceLit { encoding, bytes } => {
                // Source-level literal slices register under their
                // encoding-specific type name so the trace's `type_id`
                // is stable per flavour and a downstream consumer can
                // route on the type before even looking at the
                // payload string.  The payload string is rendered as
                // `Slice<text> "abc"` (text) or `Slice<hex> 0x"abcd"`
                // (hex) — both shapes round-trip the original source
                // form so the test corpus pins them exactly.
                let (type_name, payload_text) = match encoding {
                    SliceEncoding::Text => {
                        let body = String::from_utf8(bytes.clone()).unwrap_or_else(|_| {
                            // Fall back to a byte-oriented dump if
                            // the literal isn't valid UTF-8 (Tolk
                            // does allow `"\\xNN"` escapes that
                            // could break this; we don't see any
                            // in the current fixtures).
                            bytes
                                .iter()
                                .map(|b| format!("{:02x}", b))
                                .collect::<String>()
                        });
                        ("TolkSliceText", format!("Slice<text> \"{body}\""))
                    }
                    SliceEncoding::Hex => {
                        let nibbles: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
                        ("TolkSliceHex", format!("Slice<hex> 0x\"{nibbles}\""))
                    }
                };
                let type_id = self.ensure_raw_type_id(type_name);
                ValueRecord::Raw {
                    r: payload_text,
                    type_id,
                }
            }
        }
    }

    /// Lazily register a `TypeKind::Raw` type for the given Tolk-
    /// specific opaque name (`TolkBuilder`, `TolkCell`, `TolkSlice`)
    /// and return the registered TypeId.
    fn ensure_raw_type_id(&mut self, name: &str) -> TypeId {
        if let Some(id) = self.type_ids.get(name).copied() {
            return id;
        }
        let id = TraceWriter::ensure_type_id(&mut *self.writer, TypeKind::Raw, name);
        self.type_ids.insert(name.to_string(), id);
        id
    }

    /// Lazily register a `TypeKind::Int` slot under a Tolk-specific
    /// alias name (`coin`, etc.) and return the registered TypeId.
    /// Distinct from `ensure_raw_type_id` because the underlying
    /// `TypeKind` differs — alias types carry the same wire shape as
    /// plain `int` but a different `type_id`, so consumers can route
    /// on the type before formatting the value.
    fn ensure_int_alias_type_id(&mut self, name: &str) -> TypeId {
        if let Some(id) = self.type_ids.get(name).copied() {
            return id;
        }
        let id = TraceWriter::ensure_type_id(&mut *self.writer, TypeKind::Int, name);
        self.type_ids.insert(name.to_string(), id);
        id
    }

    /// Evaluate a single function, emitting trace events.
    /// Returns the function's return value if any.
    ///
    /// When `is_entry_point` is true, the Call/Return events for this function
    /// are suppressed — its body is evaluated directly at the caller's depth
    /// (merged into `<toplevel>`).
    ///
    /// `args` carries the resolved `Value`s for the function's formal
    /// parameters (in source order).  They are bound into the callee's
    /// local env BEFORE the body executes, so the body can reference
    /// each parameter by name.  Mirrors the cardano `a393608` arg-
    /// passing pattern.  Extra args (more than `func.params.len()`)
    /// are ignored; missing args leave the corresponding param unbound
    /// (the body falls through any reference to that name as an
    /// unknown identifier rather than panicking).
    fn evaluate_function(
        &mut self,
        _caller_source_path: &Path,
        func: &FunctionDef,
        func_map: &HashMap<String, &FunctionDef>,
        _parent_env: &mut HashMap<String, Value>,
        is_entry_point: bool,
        args: &[Value],
    ) -> Result<Option<Value>> {
        // Per-function source path: cross-file fixtures
        // (`import "helpers.tolk";`) tag each `FunctionDef` with the
        // file it was parsed from, so step events emitted from the
        // imported helper land under the correct source path on the
        // frontend.  Pre-this-fixture every step inherited the entry
        // file's path regardless of where the function actually
        // lived; see `imports_test.tolk` for the canonical fixture.
        let source_path: &Path = func.source_path.as_path();
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
            // function (`fun foo(a: int, b: int): int`).  Now that
            // `parse_call_with_args` threads the resolved actuals
            // through to the callee (see `eval_expr_to_value` /
            // `eval_expr`), we surface each one as a real
            // `ValueRecord` instead of the historical `NONE_VALUE`
            // placeholder.  Formals without a matching actual still
            // surface as `NONE_VALUE` so the calltrace row remains
            // present (matches the "best-effort, never panic"
            // discipline used elsewhere in the recorder).
            for (idx, (param_name, _param_type)) in func.params.iter().enumerate() {
                let arg_value = match args.get(idx) {
                    Some(v) => self.value_to_record(v),
                    None => NONE_VALUE,
                };
                let _ = TraceWriter::arg(&mut *self.writer, param_name, arg_value);
            }
            TraceWriter::register_call(&mut *self.writer, fn_id, vec![]);
        }

        // Local variable environment for this function.
        let mut env: HashMap<String, Value> = HashMap::new();
        // Bind formal params to actual arg values.  Zip on shorter so
        // calls with too-few actuals still produce a (degraded but
        // consistent) trace rather than panicking.  This is the
        // recorder-side counterpart to the cardano `a393608` pattern.
        for ((param_name, _param_type), arg_val) in func.params.iter().zip(args.iter()) {
            env.insert(param_name.clone(), arg_val.clone());
        }
        // Symbolic stack tracker: mirrors TVM execution to reconstruct
        // source-level variable names from stack positions.  Currently
        // only consulted by the int-arithmetic path — control-flow
        // arms don't update it because the historical bookkeeping was
        // only meaningful for the linear var-binding stream.
        let mut sym_stack = StackTracker::new();

        let exit =
            self.execute_block(source_path, &func.body, func_map, &mut env, &mut sym_stack)?;
        let return_value = match exit {
            BlockExit::Returned(v) => v,
            _ => None,
        };

        // Emit Return event (skip for entry point — its steps live under
        // <toplevel> which is closed separately).
        if !is_entry_point {
            match &return_value {
                Some(val) => {
                    let value = self.value_to_record(val);
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
    ///
    /// This is the int-only fallback path; structured shapes (tuple /
    /// struct literals, field accesses) are intercepted in
    /// `eval_expr_to_value` BEFORE reaching here.  Field accesses that
    /// resolve to scalar Ints are pre-substituted by
    /// `resolve_field_accesses` so the TVM compiler (which only knows
    /// about identifiers and integer literals) sees an int-only
    /// expression.
    fn eval_expr(
        &mut self,
        expr: &str,
        env: &mut HashMap<String, Value>,
        source_path: &Path,
        func_map: &HashMap<String, &FunctionDef>,
    ) -> Result<Option<i64>> {
        // Strip the value-level `as <T>` suffix — see the matching
        // comment in `eval_expr_to_value`.
        let stripped_owned = strip_as_casts(expr);
        let expr = stripped_owned.trim();

        if expr.is_empty() {
            return Ok(None);
        }

        // Field-access pre-pass: rewrite every `<ident>.<field>`
        // (where `<ident>` is bound in `env` to a `Value::Struct` or
        // `Value::Tuple`) to the resolved scalar literal, so the
        // downstream TVM compiler sees an int-only expression.
        // Without this, `p.x * p.x + p.y * p.y` would be unresolvable
        // — `parse_expr` doesn't understand dotted names — and
        // `point_distance_sq` would silently return `None`.
        let resolved = resolve_field_accesses(expr, env);
        let expr_str: &str = &resolved;

        // Check for function call: `<name>(<args>)` — zero-or-more
        // args.  Each actual is evaluated in the caller's env BEFORE
        // recursing into the callee, mirroring the cardano `a393608`
        // arg-passing pattern.  An actual that fails to evaluate
        // aborts the call (the body falls through to the TVM compile
        // path so callees referencing the param surface as unknown
        // identifiers — same best-effort discipline used everywhere
        // else in the recorder).
        if let Some((call_name, arg_exprs)) = parse_call_with_args(expr_str) {
            if let Some(callee) = func_map.get(call_name) {
                let callee = (*callee).clone();
                let mut arg_vals: Vec<Value> = Vec::with_capacity(arg_exprs.len());
                let mut all_args_ok = true;
                for arg_expr in &arg_exprs {
                    match self.eval_expr_to_value(arg_expr, env, source_path, func_map)? {
                        Some(v) => arg_vals.push(v),
                        None => {
                            all_args_ok = false;
                            break;
                        }
                    }
                }
                if all_args_ok {
                    let mut dummy_env: HashMap<String, Value> = HashMap::new();
                    let result = self.evaluate_function(
                        source_path,
                        &callee,
                        func_map,
                        &mut dummy_env,
                        false,
                        &arg_vals,
                    )?;
                    // Only Int return values feed back into the TVM
                    // arithmetic pipeline; structured returns surface
                    // via `eval_expr_to_value` directly.
                    return Ok(result.and_then(|v| v.as_i64()));
                }
            }
        }

        // Build the int-only sub-env that `tvm_eval_expr_checked`
        // expects, projecting structured `Value`s through `as_i64`
        // (which returns `None` for non-`Int` shapes — they're
        // simply absent from the TVM substitution map, matching the
        // behaviour of any other unknown identifier).
        let int_env = value_env_to_i64_map(env);

        // Evaluate via real TVM execution.  Use the checked variant
        // so we can route TVM execution failures (overflow, gas
        // exhaustion, divide-by-zero, etc.) through the structured
        // event channel instead of dropping them silently.
        match crate::tvm::tvm_eval_expr_checked(expr_str, &int_env) {
            Ok(value) => Ok(value),
            Err(err) => {
                let message = format!("{err}");
                eprintln!("TVM execution error in '{expr_str}': {message}");
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

    /// Evaluate a Tolk expression to a `Value`, supporting both
    /// structured shapes (tuple / struct literals, field accesses,
    /// calls returning structured values) and the existing int-only
    /// TVM arithmetic pipeline.
    ///
    /// Resolution order (first match wins):
    /// 1. Struct literal `Type { f: v, g: w }` → `Value::Struct { ... }`.
    /// 2. Tuple literal `(a, b[, c...])` (paren-wrapped, 2+ comma-
    ///    separated elements) → `Value::Tuple(...)`.
    /// 3. Bare variable reference — read from `env` (preserves
    ///    structured shape, no TVM round-trip).
    /// 4. Field access `<lhs>.<field>` — `Value::Struct` (named
    ///    field) or `Value::Tuple` (numeric index) projection.
    /// 5. Function call `f()` — recurse into `evaluate_function`,
    ///    return the callee's `Value` result.
    /// 6. Fallback — delegate to `eval_expr` (the int-only TVM
    ///    arithmetic pipeline) and lift the `i64` result back into
    ///    a `Value::Int`.
    fn eval_expr_to_value(
        &mut self,
        expr: &str,
        env: &mut HashMap<String, Value>,
        source_path: &Path,
        func_map: &HashMap<String, &FunctionDef>,
    ) -> Result<Option<Value>> {
        // Strip Tolk's value-level `as <T>` cast suffix at the top
        // level — Tolk treats it as a no-op at the value level (the
        // alias `type Coins = int;` doesn't introduce a new wire
        // shape, just a friendlier source-level name), so we recover
        // the bare expression for the downstream evaluator.  See
        // `type_aliases_casting_test.tolk` for the canonical fixture.
        let stripped_owned = strip_as_casts(expr);
        let expr = stripped_owned.trim();
        if expr.is_empty() {
            return Ok(None);
        }

        // 0a. String slice literals — Tolk's `"abc"` (UTF-8 text) and
        // `0x"abcd"` (hex bytes).  Both lift to `Value::SliceLit`
        // with the encoding tag preserved so the resulting
        // `ValueRecord::Raw` payload renders distinctly per flavour
        // (see `value_to_record`).  Recognised before the call /
        // struct / tuple arms because the literal text starts with a
        // `"` (or `0x"`) which none of those arms accept anyway, but
        // the explicit arm makes the resolution order obvious.
        if let Some(lit) = parse_slice_literal(expr) {
            return Ok(Some(lit));
        }

        // 0. Method-chain calls (`<lhs>.<method>(<args>)`) and TON
        // global helpers (`beginCell()`, `set_data(...)`,
        // `get_data()`).  These must be intercepted BEFORE the struct-
        // literal / tuple-literal / field-access arms because they
        // share syntax (a top-level `.` for method chains, parenthesised
        // arg lists, etc.) and would otherwise be misclassified.
        if let Some(v) = self.try_eval_ton_call(expr, env, source_path, func_map)? {
            return Ok(Some(v));
        }

        // 1. Struct literal: `Type { f: v, g: w }`.
        if let Some((type_name, fields)) = parse_struct_literal(expr) {
            let mut out_fields = Vec::with_capacity(fields.len());
            for (fname, fexpr) in fields {
                match self.eval_expr_to_value(&fexpr, env, source_path, func_map)? {
                    Some(v) => out_fields.push((fname, v)),
                    None => return Ok(None),
                }
            }
            return Ok(Some(Value::Struct {
                type_name,
                fields: out_fields,
            }));
        }

        // 2. Tuple literal: `(a, b[, c...])` — must be paren-wrapped
        // and contain at least one top-level comma at depth 1.
        if let Some(elems) = parse_tuple_literal(expr) {
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                match self.eval_expr_to_value(&e, env, source_path, func_map)? {
                    Some(v) => out.push(v),
                    None => return Ok(None),
                }
            }
            return Ok(Some(Value::Tuple(out)));
        }

        // 3. Bare variable reference — preserves structured shape.
        if is_simple_identifier(expr) {
            if let Some(v) = env.get(expr) {
                return Ok(Some(v.clone()));
            }
            // Fall through to TVM for unknown identifiers (will
            // surface as an evaluation error rather than panicking).
        }

        // 4. Field access: `<lhs>.<field>` — top-level dot, where
        // `<lhs>` resolves to a `Value::Struct` or `Value::Tuple`
        // and `<field>` is either a field name (Struct) or numeric
        // index (Tuple).
        if let Some((lhs, field)) = split_top_level_dot(expr) {
            if let Some(base) = self.eval_expr_to_value(lhs, env, source_path, func_map)? {
                match (&base, field) {
                    (Value::Struct { fields, .. }, fname) => {
                        if let Some((_, v)) = fields.iter().find(|(n, _)| n == fname) {
                            return Ok(Some(v.clone()));
                        }
                    }
                    (Value::Tuple(elements), idx_str) => {
                        if let Ok(idx) = idx_str.parse::<usize>() {
                            if let Some(v) = elements.get(idx) {
                                return Ok(Some(v.clone()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // 5. Function call.  Once `parse_call_with_args` identifies
        // `<name>(<args>)` AND `<name>` resolves to a known
        // user-defined function in `func_map`, we own the evaluation
        // entirely — the result (Some or None) is returned directly
        // so we don't accidentally re-invoke the callee from the
        // int-only `eval_expr` fallback below.  This matters for
        // calls that abort via a runtime-tripping
        // `Statement::GatedThrow` (the callee's `BlockExit::Aborted`
        // becomes `Ok(None)` here): without the explicit `return`,
        // the fallback would re-invoke the callee and double-emit
        // the Error io_event.
        if let Some((call_name, arg_exprs)) = parse_call_with_args(expr) {
            if let Some(callee) = func_map.get(call_name) {
                let callee = (*callee).clone();
                let mut arg_vals: Vec<Value> = Vec::with_capacity(arg_exprs.len());
                let mut all_args_ok = true;
                for arg_expr in &arg_exprs {
                    match self.eval_expr_to_value(arg_expr, env, source_path, func_map)? {
                        Some(v) => arg_vals.push(v),
                        None => {
                            all_args_ok = false;
                            break;
                        }
                    }
                }
                if all_args_ok {
                    let mut dummy_env: HashMap<String, Value> = HashMap::new();
                    let result = self.evaluate_function(
                        source_path,
                        &callee,
                        func_map,
                        &mut dummy_env,
                        false,
                        &arg_vals,
                    )?;
                    return Ok(result);
                }
            }
        }

        // 6. Fallback — int-only TVM arithmetic.  Lift the resulting
        // `i64` (if any) back into a `Value::Int`.
        let result = self.eval_expr(expr, env, source_path, func_map)?;
        Ok(result.map(Value::Int))
    }

    /// Execute a sequence of statements (function body, if-arm, loop
    /// body, ...) in order.  Returns a `BlockExit` that tells the
    /// caller whether the block fell through, hit a `return`, or
    /// hit a `throw`/`assert` (which short-circuits enclosing loops
    /// and branches as well).
    fn execute_block(
        &mut self,
        source_path: &Path,
        stmts: &[Statement],
        func_map: &HashMap<String, &FunctionDef>,
        env: &mut HashMap<String, Value>,
        sym_stack: &mut StackTracker,
    ) -> Result<BlockExit> {
        for stmt in stmts {
            match self.execute_statement(source_path, stmt, func_map, env, sym_stack)? {
                BlockExit::Fallthrough => continue,
                other => return Ok(other),
            }
        }
        Ok(BlockExit::Fallthrough)
    }

    /// Execute a single statement.  See `execute_block` for the
    /// `BlockExit` semantics.
    fn execute_statement(
        &mut self,
        source_path: &Path,
        stmt: &Statement,
        func_map: &HashMap<String, &FunctionDef>,
        env: &mut HashMap<String, Value>,
        sym_stack: &mut StackTracker,
    ) -> Result<BlockExit> {
        match stmt {
            Statement::VarBinding {
                name,
                type_name,
                expr,
                line,
            } => {
                // Emit Step event.
                TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));

                if let Some(raw_val) = self.eval_expr_to_value(expr, env, source_path, func_map)? {
                    // Type-driven lift: when the LHS declares a Tolk
                    // domain type (`coin`, `address`) and the
                    // expression evaluated to a plain `Int`, repackage
                    // the value in the matching `Value::Coin` /
                    // `Value::Address` variant so the resulting
                    // `ValueRecord` carries the right type tag (and
                    // shape, in the address case — `Struct{wc, hash}`).
                    // The source-level construction surface stays
                    // unchanged: `var sender: address =
                    // sender_slice.loadAddress();` just declares the
                    // intended domain type and the recorder honours it
                    // here.  See `address_coins_test.tolk`.
                    let val = match (type_name.as_deref(), &raw_val) {
                        (Some("address"), Value::Int(i)) => Value::Address {
                            workchain: 0,
                            hash: *i,
                        },
                        (Some("coin"), Value::Int(i)) => Value::Coin(*i),
                        _ => raw_val,
                    };
                    if let Some(int_val) = val.as_i64() {
                        let int_env = value_env_to_i64_map(env);
                        let expr_tracker = stack_tracker::track_expr(expr, &int_env, int_val);
                        let _derived = expr_tracker.variables_at_step();
                        sym_stack.push(int_val, Some(name.clone()));
                    }

                    let value = match &val {
                        Value::Int(i) => {
                            let type_id = type_name
                                .as_deref()
                                .and_then(|n| self.type_ids.get(n).copied())
                                .unwrap_or_else(|| self.type_ids.get("int").copied().unwrap());
                            ValueRecord::Int { i: *i, type_id }
                        }
                        _ => self.value_to_record(&val),
                    };
                    env.insert(name.clone(), val);
                    TraceWriter::register_variable_with_full_value(&mut *self.writer, name, value);
                }
                Ok(BlockExit::Fallthrough)
            }
            Statement::Assign { name, expr, line } => {
                // Emit Step event for the assignment line.
                TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));

                if let Some(val) = self.eval_expr_to_value(expr, env, source_path, func_map)? {
                    let value = self.value_to_record(&val);
                    env.insert(name.clone(), val);
                    TraceWriter::register_variable_with_full_value(&mut *self.writer, name, value);
                }
                Ok(BlockExit::Fallthrough)
            }
            Statement::Return { expr, line } => {
                TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));

                let val = self.eval_expr_to_value(expr, env, source_path, func_map)?;
                Ok(BlockExit::Returned(val))
            }
            Statement::Throw { .. } | Statement::Assert { .. } => {
                // See the original-arm comment: today these are
                // surfaced as Error io_events via the post-execution
                // static sweep.  We propagate `Aborted` so enclosing
                // loops / branches stop executing instead of running
                // dead code past the failure marker.
                Ok(BlockExit::Aborted)
            }
            Statement::If {
                cond,
                then_block,
                else_block,
                line,
            } => {
                // Emit a Step event for the if-header line so the
                // branch shows up in the calltrace pane.
                TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));
                let cond_val = self.eval_cond(cond, env, source_path, func_map)?;
                let arm = if cond_val { then_block } else { else_block };
                self.execute_block(source_path, arm, func_map, env, sym_stack)
            }
            Statement::While { cond, body, line } => {
                // Emit a Step event for the loop header so the loop
                // construct itself shows up in the trace.
                TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));
                let mut iters = 0u32;
                loop {
                    if iters >= LOOP_ITERATION_BOUND {
                        eprintln!(
                            "while loop at line {line} exceeded {LOOP_ITERATION_BOUND} \
                             iterations; aborting recorder-side evaluation"
                        );
                        break;
                    }
                    if !self.eval_cond(cond, env, source_path, func_map)? {
                        break;
                    }
                    match self.execute_block(source_path, body, func_map, env, sym_stack)? {
                        BlockExit::Fallthrough => {}
                        other => return Ok(other),
                    }
                    iters += 1;
                }
                Ok(BlockExit::Fallthrough)
            }
            Statement::Repeat {
                count_expr,
                body,
                line,
            } => {
                TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));
                let count = self
                    .eval_expr_to_value(count_expr, env, source_path, func_map)?
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let count = count.max(0) as u64;
                let bound = LOOP_ITERATION_BOUND as u64;
                let actual = count.min(bound);
                if count > bound {
                    eprintln!(
                        "repeat loop at line {line} requested {count} iterations; \
                         clamping to recorder-side bound {bound}"
                    );
                }
                for _ in 0..actual {
                    match self.execute_block(source_path, body, func_map, env, sym_stack)? {
                        BlockExit::Fallthrough => {}
                        other => return Ok(other),
                    }
                }
                Ok(BlockExit::Fallthrough)
            }
            Statement::DoUntil { body, cond, line } => {
                TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));
                let mut iters = 0u32;
                loop {
                    if iters >= LOOP_ITERATION_BOUND {
                        eprintln!(
                            "do/until loop at line {line} exceeded \
                             {LOOP_ITERATION_BOUND} iterations; aborting \
                             recorder-side evaluation"
                        );
                        break;
                    }
                    match self.execute_block(source_path, body, func_map, env, sym_stack)? {
                        BlockExit::Fallthrough => {}
                        other => return Ok(other),
                    }
                    iters += 1;
                    if self.eval_cond(cond, env, source_path, func_map)? {
                        break;
                    }
                }
                Ok(BlockExit::Fallthrough)
            }
            Statement::ExprStatement { expr, line } => {
                // Emit a Step event for the call line and run the
                // expression for its side effects (`set_data(c);`,
                // method-chain mutations on a builder, etc.).  The
                // returned value is intentionally discarded.
                TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));
                let _ = self.eval_expr_to_value(expr, env, source_path, func_map)?;
                Ok(BlockExit::Fallthrough)
            }
            Statement::GatedThrow {
                code,
                condition,
                mode,
                line,
            } => {
                // Emit a Step event for the guard line so the branch
                // shows up in the calltrace pane regardless of whether
                // the gate trips.
                TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));
                // Evaluate the condition.  `eval_cond` already
                // normalises any TVM-side `-1` ("true") and missing
                // values to a Rust `bool`; we then route that against
                // the gate selector.
                let cond_val = self.eval_cond(condition, env, source_path, func_map)?;
                let trips = match mode {
                    GateMode::If => cond_val,
                    GateMode::Unless => !cond_val,
                };
                if trips {
                    let label = match mode {
                        GateMode::If => "throwIf",
                        GateMode::Unless => "throwUnless",
                    };
                    let message = format!("{label}: code {code}");
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::Error,
                        "TolkThrow",
                        &message,
                    );
                    // Propagate Aborted so enclosing loops / branches
                    // stop executing past the failure marker.  Unlike
                    // the unconditional `Throw` case, the caller can
                    // recover by short-circuiting the current function
                    // (Tolk has no try/catch in the recorder's view).
                    Ok(BlockExit::Aborted)
                } else {
                    Ok(BlockExit::Fallthrough)
                }
            }
            Statement::Match {
                scrutinee,
                arms,
                line,
            } => {
                // Emit a Step event for the match-header line so the
                // dispatch shows up in the calltrace pane.
                TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));
                // Resolve the scrutinee to a `Value::Struct` so we
                // can read its variant tag (the `type_name`) and any
                // payload field.  A non-struct scrutinee produces no
                // arm dispatch — we simply fall through (mirroring
                // the recorder's "best-effort, never panic"
                // discipline).
                let scrutinee_val =
                    self.eval_expr_to_value(scrutinee, env, source_path, func_map)?;
                let (ctor_tag, first_field): (Option<String>, Option<Value>) = match &scrutinee_val
                {
                    Some(Value::Struct { type_name, fields }) => (
                        Some(type_name.clone()),
                        fields.first().map(|(_n, v)| v.clone()),
                    ),
                    _ => (None, None),
                };
                let Some(tag) = ctor_tag else {
                    return Ok(BlockExit::Fallthrough);
                };
                // Pick the spec-correct arm: the one whose
                // constructor name matches the scrutinee's tag.
                // First match wins; this is NOT a lexical
                // last-arm-wins fallback.
                let arm = arms.iter().find(|a| a.constructor == tag);
                let Some(arm) = arm else {
                    // Unmatched scrutinee — no arm fires.  Mirrors
                    // the cardano recorder's "missing arm =
                    // fallthrough" behaviour.
                    return Ok(BlockExit::Fallthrough);
                };
                // Bind the variant payload (if the arm names one)
                // before executing the body.  Tolk's `Active(n)`
                // form binds `n` to the variant's first field for
                // the duration of the arm.
                if let (Some(binder), Some(payload)) = (&arm.payload_binding, &first_field) {
                    env.insert(binder.clone(), payload.clone());
                }
                self.execute_block(source_path, &arm.body, func_map, env, sym_stack)
            }
        }
    }

    /// Evaluate a condition expression to a Rust `bool`.  Anything
    /// non-zero (including the TVM convention `-1` for "true" and the
    /// recorder's normalised `1`) counts as truthy; missing /
    /// unparseable conditions default to `false` so loops terminate
    /// rather than spin forever.
    fn eval_cond(
        &mut self,
        cond: &str,
        env: &mut HashMap<String, Value>,
        source_path: &Path,
        func_map: &HashMap<String, &FunctionDef>,
    ) -> Result<bool> {
        let val = self.eval_expr_to_value(cond, env, source_path, func_map)?;
        Ok(val.and_then(|v| v.as_i64()).unwrap_or(0) != 0)
    }

    /// Recognise and evaluate the small set of TON-specific calls used
    /// by `cell_ops_test.tolk` — global helpers (`beginCell()`,
    /// `set_data(<expr>)`, `get_data()`) and Builder/Slice/Cell method
    /// chains (`<lhs>.storeInt(<v>, <bits>)`, `<lhs>.endCell()`,
    /// `<lhs>.beginParse()`, `<lhs>.loadInt(<bits>)`).  Returns
    /// `Ok(Some(_))` if the expression matched and was handled,
    /// `Ok(None)` otherwise so the caller falls through to the rest
    /// of the resolver chain.
    ///
    /// `set_data(c)` and `get_data()` register an io_event each
    /// (`EventLogKind::Write` / `EventLogKind::Read`, metadata
    /// `"TolkStorage"`) so the canonical-CTFS trace surfaces TON
    /// persistent-storage interactions on the same channel as the
    /// rest of the recorder ecosystem.
    fn try_eval_ton_call(
        &mut self,
        expr: &str,
        env: &mut HashMap<String, Value>,
        source_path: &Path,
        func_map: &HashMap<String, &FunctionDef>,
    ) -> Result<Option<Value>> {
        let expr = expr.trim();

        // Global helpers: `beginCell()`, `get_data()`/`load_data()`,
        // `set_data(<expr>)`/`save_data(<expr>)`.  `load_data` /
        // `save_data` are the canonical Tolk names for TON persistent
        // storage (see the per-contract `Storage` struct idiom in
        // `persistent_storage_test.tolk`); `get_data` / `set_data`
        // are their legacy FunC-era aliases.  Both surface as the
        // same Read / Write io_events tagged `"TolkStorage"` so the
        // frontend's storage panel doesn't need to know which name
        // the source happens to use.
        if let Some((name, args)) = parse_call_with_args(expr) {
            // TVM context primitives backed by continuation register
            // c7.  The TVM whitepaper §5 defines these as
            // contract-context lookups; pre-this-fixture the
            // recorder treated them as unknown calls and silently
            // dropped the binding, so a fixture that exercised
            // `now()` / `myBalance()` / `msgSender()` / `myAddress()`
            // produced a degraded trace.  Spec-compliant recording
            // surfaces each primitive with the matching domain type
            // (`int` for `now()`, `coin` for `myBalance()`,
            // `address` for `msgSender()` / `myAddress()`) and
            // deterministic recorder-side context values so tests
            // pin both the type tag AND a representative payload.
            // See `tvm_primitives_test.tolk` for the canonical
            // fixture.
            if args.is_empty() {
                match name {
                    "now" => return Ok(Some(Value::Int(1_700_000_000))),
                    "myBalance" => return Ok(Some(Value::Coin(2_000_000_000))),
                    "msgSender" => {
                        return Ok(Some(Value::Address {
                            workchain: 0,
                            hash: 0xCAFE,
                        }));
                    }
                    "myAddress" => {
                        return Ok(Some(Value::Address {
                            workchain: 0,
                            hash: 0xBEEF,
                        }));
                    }
                    _ => {}
                }
            }

            if name == "beginCell" && args.is_empty() {
                return Ok(Some(Value::Builder {
                    payload: Vec::new(),
                    refs: Vec::new(),
                }));
            }
            if (name == "get_data" || name == "load_data") && args.is_empty() {
                let (payload, refs) = match &self.storage_data {
                    Some(Value::Cell { payload, refs }) => (payload.clone(), refs.clone()),
                    Some(Value::Builder { payload, refs }) => (payload.clone(), refs.clone()),
                    _ => (Vec::new(), Vec::new()),
                };
                let summary = format_payload_with_refs("Cell", &payload, &refs);
                TraceWriter::register_special_event(
                    &mut *self.writer,
                    EventLogKind::Read,
                    "TolkStorage",
                    &format!("{name}: {summary}"),
                );
                return Ok(Some(Value::Cell { payload, refs }));
            }
            if (name == "set_data" || name == "save_data") && args.len() == 1 {
                if let Some(val) = self.eval_expr_to_value(&args[0], env, source_path, func_map)? {
                    let summary = match &val {
                        Value::Cell { payload, refs } => {
                            format_payload_with_refs("Cell", payload, refs)
                        }
                        Value::Builder { payload, refs } => {
                            format_payload_with_refs("Builder", payload, refs)
                        }
                        _ => "<non-cell>".to_string(),
                    };
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::Write,
                        "TolkStorage",
                        &format!("{name}: {summary}"),
                    );
                    self.storage_data = Some(val);
                }
                // `set_data(c);` / `save_data(c);` are statement-
                // shaped; we still return something concrete so the
                // surrounding expression doesn't accidentally fall
                // through to the int-only TVM path.  An empty `Cell`
                // is harmless because the call doesn't show up on a
                // value-bearing RHS.
                return Ok(Some(Value::Cell {
                    payload: Vec::new(),
                    refs: Vec::new(),
                }));
            }
        }

        // Method chains: `<lhs>.<method>(<args>)`.  Use the rightmost
        // top-level dot as the split point so chained calls
        // (`beginCell().storeInt(42, 32).endCell()`) recurse left-
        // first.
        if let Some((lhs_str, method, arg_strs)) = parse_method_call(expr) {
            // For ref-bearing methods (storeRef / storeMaybeRef /
            // storeSlice) we need the actual argument Value (a Cell
            // or Slice), not an int.  Evaluate the args BEFORE the
            // LHS so the resolution order matches the source-level
            // read: the arg side-effects run first.  Pre-M10 every
            // recognised method here took int args only, so the eager
            // int-projection below remained valid; the new arms
            // consult `arg_value_records` directly for the cell-bearing
            // args.
            let mut arg_value_records: Vec<Option<Value>> = Vec::with_capacity(arg_strs.len());
            for a in &arg_strs {
                let v = self.eval_expr_to_value(a, env, source_path, func_map)?;
                arg_value_records.push(v);
            }
            // Evaluate the LHS after the args (matches the historical
            // order — chained calls like `b.storeInt(...).endCell()`
            // recurse left-first via the dot split).
            let base = match self.eval_expr_to_value(lhs_str, env, source_path, func_map)? {
                Some(v) => v,
                None => return Ok(None),
            };
            // Project the int-bearing args for the legacy int-only
            // shapes (storeInt / storeUint / loadInt / ...).
            let arg_ints: Vec<Option<i64>> = arg_value_records
                .iter()
                .map(|v| v.as_ref().and_then(|x| x.as_i64()))
                .collect();

            match method {
                "storeInt" | "storeUint" => {
                    let (mut payload, refs) = match base {
                        Value::Builder { payload, refs } => (payload, refs),
                        Value::Cell { payload, refs } => (payload, refs),
                        _ => return Ok(None),
                    };
                    if let Some(Some(v)) = arg_ints.first() {
                        payload.push(*v);
                    }
                    return Ok(Some(Value::Builder { payload, refs }));
                }
                // `storeRef(<cell>)`: append the entire ref-cell to
                // the builder's ref queue.  `storeMaybeRef(<cell>)`:
                // same as storeRef when the arg is a real cell;
                // appends an empty marker cell when the arg is
                // missing / unresolvable (TON's null-ref convention).
                // `storeSlice(<slice>)`: flatten the slice's
                // remaining payload onto the builder's int payload
                // (Tolk's bit-level concat); refs from the slice are
                // appended to the builder's ref queue.
                "storeRef" | "storeMaybeRef" => {
                    let (payload, mut refs) = match base {
                        Value::Builder { payload, refs } => (payload, refs),
                        Value::Cell { payload, refs } => (payload, refs),
                        _ => return Ok(None),
                    };
                    let ref_payload = match arg_value_records.first().and_then(|v| v.clone()) {
                        Some(Value::Cell { payload: p, .. }) => p,
                        Some(Value::Builder { payload: p, .. }) => p,
                        Some(Value::Slice {
                            payload: p, pos, ..
                        }) => {
                            // Drain the slice's remaining int payload
                            // (positions >= pos).  This matches the
                            // TON convention that a slice converted
                            // to a ref carries only its unconsumed
                            // tail.
                            p[pos..].to_vec()
                        }
                        Some(Value::Int(v)) => vec![v],
                        _ => Vec::new(),
                    };
                    refs.push(ref_payload);
                    return Ok(Some(Value::Builder { payload, refs }));
                }
                "storeSlice" => {
                    let (mut payload, mut refs) = match base {
                        Value::Builder { payload, refs } => (payload, refs),
                        Value::Cell { payload, refs } => (payload, refs),
                        _ => return Ok(None),
                    };
                    if let Some(Some(Value::Slice {
                        payload: sp,
                        pos,
                        refs: sr,
                        ref_pos,
                    })) = arg_value_records.first().map(|v| v.clone())
                    {
                        payload.extend_from_slice(&sp[pos..]);
                        for r in sr.into_iter().skip(ref_pos) {
                            refs.push(r);
                        }
                    }
                    return Ok(Some(Value::Builder { payload, refs }));
                }
                // `storeAddress(<int>)`: TON's wallet/account address
                // surfaces here as a single int (the recorder doesn't
                // model the bit-level address wire format; the int
                // round-trip is what every fixture verifies).  Mirrors
                // `storeInt` semantically but is broken out so the
                // diff-against-source is readable.
                "storeAddress" => {
                    let (mut payload, refs) = match base {
                        Value::Builder { payload, refs } => (payload, refs),
                        Value::Cell { payload, refs } => (payload, refs),
                        _ => return Ok(None),
                    };
                    if let Some(Some(v)) = arg_ints.first() {
                        payload.push(*v);
                    }
                    return Ok(Some(Value::Builder { payload, refs }));
                }
                "endCell" => {
                    let (payload, refs) = match base {
                        Value::Builder { payload, refs } => (payload, refs),
                        Value::Cell { payload, refs } => (payload, refs),
                        _ => return Ok(None),
                    };
                    return Ok(Some(Value::Cell { payload, refs }));
                }
                "beginParse" => {
                    let (payload, refs) = match base {
                        Value::Cell { payload, refs } => (payload, refs),
                        Value::Builder { payload, refs } => (payload, refs),
                        Value::Slice { payload, refs, .. } => (payload, refs),
                        _ => return Ok(None),
                    };
                    return Ok(Some(Value::Slice {
                        payload,
                        pos: 0,
                        refs,
                        ref_pos: 0,
                    }));
                }
                "loadInt" | "loadUint" | "loadAddress" => {
                    let (payload, pos, refs, ref_pos) = match base {
                        Value::Slice {
                            payload,
                            pos,
                            refs,
                            ref_pos,
                        } => (payload, pos, refs, ref_pos),
                        _ => return Ok(None),
                    };
                    let value = payload.get(pos).copied().unwrap_or(0);
                    let new_slice = Value::Slice {
                        payload,
                        pos: pos + 1,
                        refs,
                        ref_pos,
                    };
                    // If the LHS was a simple identifier, mutate the
                    // env so subsequent `loadInt`/`loadAddress` calls
                    // advance the cursor.  Anything more complex
                    // (chained calls, expressions) just yields the
                    // int — the caller can't observe the slice
                    // anyway.
                    let lhs_ident = lhs_str.trim();
                    if is_simple_identifier(lhs_ident) {
                        env.insert(lhs_ident.to_string(), new_slice);
                    }
                    return Ok(Some(Value::Int(value)));
                }
                // `loadRef()` / `loadMaybeRef()`: pop the next sub-
                // cell from the slice's ref queue.  Returns an empty
                // Cell when exhausted (matches the recorder's
                // "best-effort, never panic" discipline).  As with
                // `loadInt`, mutate the env when the LHS is a simple
                // identifier so subsequent loads see the advanced
                // ref cursor.
                "loadRef" | "loadMaybeRef" => {
                    let (payload, pos, refs, ref_pos) = match base {
                        Value::Slice {
                            payload,
                            pos,
                            refs,
                            ref_pos,
                        } => (payload, pos, refs, ref_pos),
                        _ => return Ok(None),
                    };
                    let popped = refs.get(ref_pos).cloned().unwrap_or_default();
                    let new_slice = Value::Slice {
                        payload,
                        pos,
                        refs,
                        ref_pos: ref_pos + 1,
                    };
                    let lhs_ident = lhs_str.trim();
                    if is_simple_identifier(lhs_ident) {
                        env.insert(lhs_ident.to_string(), new_slice);
                    }
                    return Ok(Some(Value::Cell {
                        payload: popped,
                        refs: Vec::new(),
                    }));
                }
                _ => {
                    // Fall through to user-defined method dispatch.  A
                    // method-receiver helper is registered under the
                    // qualified name `<Type>.<method>`; we resolve the
                    // base's struct type and look up the qualified
                    // name in `func_map`.  When the base is not a
                    // struct (or the qualified name isn't registered),
                    // this returns Ok(None) so the caller's resolver
                    // chain continues.
                    if let Value::Struct { type_name, .. } = &base {
                        let qualified = format!("{type_name}.{method}");
                        if let Some(callee) = func_map.get(qualified.as_str()) {
                            let callee = (*callee).clone();
                            // Build the actuals: receiver first, then
                            // the parsed positional args.
                            let mut arg_vals: Vec<Value> = Vec::with_capacity(arg_strs.len() + 1);
                            arg_vals.push(base.clone());
                            let mut all_args_ok = true;
                            for (i, a) in arg_strs.iter().enumerate() {
                                match arg_value_records.get(i).cloned().flatten() {
                                    Some(v) => arg_vals.push(v),
                                    None => {
                                        // Re-evaluate (the first pass
                                        // is best-effort and may have
                                        // returned None for a
                                        // non-int-flat shape).
                                        match self.eval_expr_to_value(
                                            a,
                                            env,
                                            source_path,
                                            func_map,
                                        )? {
                                            Some(v) => arg_vals.push(v),
                                            None => {
                                                all_args_ok = false;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            if all_args_ok {
                                let mut dummy_env: HashMap<String, Value> = HashMap::new();
                                let result = self.evaluate_function(
                                    source_path,
                                    &callee,
                                    func_map,
                                    &mut dummy_env,
                                    false,
                                    &arg_vals,
                                )?;
                                return Ok(result);
                            }
                        }
                    }
                    return Ok(None);
                }
            }
        }

        Ok(None)
    }
}

/// Outcome of executing a block of statements.  Encodes whether the
/// block ran to completion, hit a `return`, or aborted (via
/// `throw`/`assert`).  Bubbles out of nested loops / branches so a
/// `return` inside a `while` body terminates the enclosing function
/// rather than just the loop iteration.
#[derive(Debug)]
enum BlockExit {
    /// Block ran every statement to completion; caller continues with
    /// the next statement at the same nesting level.
    Fallthrough,
    /// Block hit a `return <expr>;`.  `Some(v)` if the return carried
    /// a value, `None` for `return;` or evaluation failures.
    Returned(Option<Value>),
    /// Block hit a `throw`/`assert` (or a nested block did).  Caller
    /// should stop evaluating further statements at every enclosing
    /// level — there's no "catch" arm in the current AST.
    Aborted,
}

/// Maximum number of iterations the recorder-side interpreter will
/// run a `while` / `repeat` / `do/until` loop for before giving up.
/// Acts as a safety net against runaway loops in case the condition
/// expression doesn't evaluate the way the program author intended.
/// Picked at 10_000 to comfortably cover the small fixtures in
/// `test-programs/tolk/` while still keeping the recorder bounded.
const LOOP_ITERATION_BOUND: u32 = 10_000;

/// Format a Builder/Cell/Slice payload as a human-readable string for
/// the on-trace `ValueRecord::Raw.r` field.  The payload is the ordered
/// int list round-tripped through `storeInt` / `loadInt`.
fn format_payload(kind: &'static str, payload: &[i64]) -> String {
    if payload.is_empty() {
        format!("{kind}([])")
    } else {
        let parts: Vec<String> = payload.iter().map(|v| v.to_string()).collect();
        format!("{kind}([{}])", parts.join(", "))
    }
}

/// Format a Builder/Cell/Slice payload that may carry sub-cell refs
/// (`builder_refs_test.tolk`).  When `refs` is empty the output
/// matches `format_payload` exactly so the cell-ops fixture's pinned
/// payload strings continue to round-trip; when refs are present they
/// are appended as `refs=[Cell(...), ...]` after the int payload.
fn format_payload_with_refs(kind: &'static str, payload: &[i64], refs: &[Vec<i64>]) -> String {
    let head = format_payload(kind, payload);
    if refs.is_empty() {
        return head;
    }
    let parts: Vec<String> = refs.iter().map(|r| format_payload("Cell", r)).collect();
    format!("{head} refs=[{}]", parts.join(", "))
}

/// Project a structured `Value` env down to the int-only sub-env that
/// `tvm_eval_expr_checked` consumes for variable substitution.  Non-`Int`
/// values are dropped (they simply won't be found by the TVM compiler,
/// matching the historical "unknown identifier → leave the substitution
/// hole" behaviour).
fn value_env_to_i64_map(env: &HashMap<String, Value>) -> HashMap<String, i64> {
    env.iter()
        .filter_map(|(k, v)| v.as_i64().map(|i| (k.clone(), i)))
        .collect()
}

/// Strip Tolk's value-level `as <T>` cast suffix from an expression.
/// Returns the input unchanged when no matching ` as <ident>` token
/// is present.
///
/// Tolk's `as` keyword is a value-level no-op when it crosses a
/// `type Coins = int;` style alias (and in real-world contracts that
/// is the only shape that actually reaches the recorder — the more
/// adventurous `as` shapes touch storage layout, which the recorder
/// doesn't model).  So the right thing for the int-only TVM
/// evaluator and for the `Value`-aware evaluator is the same: drop
/// the cast and forward the underlying expression unchanged.
///
/// We walk the expression byte-by-byte and recognise ` as ` at any
/// bracket depth (Tolk semantics: an `as` cast is a no-op whether it
/// sits at the top level or nested inside a parenthesised
/// sub-expression).  The pattern repeats so chained casts like
/// `(x as A) as B` collapse fully.
fn strip_as_casts(input: &str) -> String {
    let s = input;
    // Fast path: no `as` substring at all.
    if !s.contains(" as ") {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < bytes.len() {
        // Recognise ` as ` (whitespace-bounded `as` keyword) and skip
        // past the trailing type identifier.  The leading space is
        // consumed (not re-emitted) so `balance as Coins` becomes
        // `balance` rather than `balance ` (no trailing space).
        if i + 4 <= bytes.len() && &bytes[i..i + 4] == b" as " {
            let mut j = i + 4;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            i = j;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Rewrite every `<ident>.<field-or-index>` subterm of `expr` (where
/// `<ident>` is bound in `env` to a `Value::Struct` or `Value::Tuple`)
/// to the resolved scalar literal text.  Used by `eval_expr` to bridge
/// the field-access syntax to the int-only TVM substitution map that
/// `tvm_eval_expr_checked` expects.
///
/// Walks the expression byte-by-byte, identifying maximal runs of
/// `<ident>.<field>` shape at safe positions (i.e. the head of the
/// `<ident>` must be at a word boundary).  Only `Int`-valued field
/// projections are substituted; structured-valued projections stay
/// in place (they'd just hit the same "unknown identifier" wall
/// downstream).
fn resolve_field_accesses(expr: &str, env: &HashMap<String, Value>) -> String {
    let bytes = expr.as_bytes();
    let mut out = String::with_capacity(expr.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let prev = if i == 0 { None } else { Some(bytes[i - 1]) };
        let at_word_boundary = match prev {
            None => true,
            Some(p) => !(p.is_ascii_alphanumeric() || p == b'_' || p == b'.'),
        };
        if at_word_boundary && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
            let ident_start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &expr[ident_start..i];
            if i < bytes.len() && bytes[i] == b'.' {
                // `<ident>.<field>` — gather the field run (alnum / _).
                let field_start = i + 1;
                let mut j = field_start;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if j > field_start {
                    let field = &expr[field_start..j];
                    if let Some(base) = env.get(ident) {
                        let resolved: Option<i64> = match base {
                            Value::Struct { fields, .. } => fields
                                .iter()
                                .find(|(n, _)| n == field)
                                .and_then(|(_, v)| v.as_i64()),
                            Value::Tuple(elements) => field
                                .parse::<usize>()
                                .ok()
                                .and_then(|idx| elements.get(idx))
                                .and_then(|v| v.as_i64()),
                            _ => None,
                        };
                        if let Some(n) = resolved {
                            out.push_str(&n.to_string());
                            i = j;
                            continue;
                        }
                    }
                }
            }
            out.push_str(ident);
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Tolk source parser helpers
// ---------------------------------------------------------------------------

/// Parse function definitions from Tolk source code.
///
/// Handles the pattern: `fun <name>(<params>): <type> { ... }`
/// and recursively parses control-flow blocks (`if`, `else`, `while`,
/// `repeat`, `do`/`until`) inside each body.
/// Recursively load every `import "<path>";` declaration found in
/// `source` (the contents of `from_path`).  Each imported file is
/// resolved relative to `from_path`'s parent directory (Tolk's
/// canonical lookup: imports are sibling-relative, not project-root
/// relative).  Already-loaded files are tracked in `already_imported`
/// so a diamond `A imports B imports A` doesn't re-parse anything.
/// Functions parsed from each imported file are appended to
/// `functions` with their own source-path tag preserved (so step
/// events emit under the correct path).
fn load_imports_recursive(
    from_path: &Path,
    source: &str,
    functions: &mut Vec<FunctionDef>,
    already_imported: &mut std::collections::HashSet<PathBuf>,
    imported_sources: &mut Vec<String>,
) {
    let parent = from_path.parent().unwrap_or_else(|| Path::new("."));
    for raw in source.lines() {
        let line = raw.trim();
        // Tolk's canonical import shape is `import "path";`; tolerate
        // optional trailing semicolons and surrounding whitespace.
        let after = match line.strip_prefix("import") {
            Some(s) => s.trim(),
            None => continue,
        };
        // Strip the open quote, then capture up to the close quote.
        let after = match after.strip_prefix('"') {
            Some(s) => s,
            None => continue,
        };
        let close = match after.find('"') {
            Some(p) => p,
            None => continue,
        };
        let rel_path = &after[..close];
        if rel_path.is_empty() {
            continue;
        }
        let import_path = parent.join(rel_path);
        let canonical = import_path
            .canonicalize()
            .unwrap_or_else(|_| import_path.clone());
        if !already_imported.insert(canonical.clone()) {
            continue;
        }
        let imported_source = match std::fs::read_to_string(&import_path) {
            Ok(s) => s,
            Err(err) => {
                eprintln!(
                    "warn: import \"{}\" from {} failed: {err}",
                    rel_path,
                    from_path.display()
                );
                continue;
            }
        };
        let imported_funcs = parse_functions(&imported_source, &import_path);
        functions.extend(imported_funcs);
        imported_sources.push(imported_source.clone());
        load_imports_recursive(
            &import_path,
            &imported_source,
            functions,
            already_imported,
            imported_sources,
        );
    }
}

fn parse_functions(source: &str, source_path: &Path) -> Vec<FunctionDef> {
    let mut functions = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;
    // Pending attribute decorators that have been seen but not yet
    // attached to a `fun` declaration.  Tolk uses `@method_id(N)`,
    // `@inline`, `@inline_ref`, `@pure` (and a handful of others) on
    // the line(s) preceding a `fun` line.  Today we recognise
    // `@method_id(N)` specifically and fold N onto the next function
    // declaration; every other attribute is skipped silently so the
    // function still parses normally.  Multiple consecutive attribute
    // lines stack — the canonical TON style is one attribute per line.
    let mut pending_method_id: Option<i64> = None;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        let line_num = (i + 1) as u32;

        // Function-attribute lines.  Each `@<name>` (with optional
        // `(<args>)`) is recognised and the parser advances; the
        // attribute may carry a `@method_id(N)` payload that we
        // capture for the next-declared function.  Trailing junk
        // beyond the closing paren (rare) is tolerated — the line is
        // still consumed.
        if trimmed.starts_with('@') {
            if let Some(name_and_args) = trimmed.strip_prefix('@') {
                if let Some(paren_open) = name_and_args.find('(') {
                    let name = name_and_args[..paren_open].trim();
                    if name == "method_id" {
                        if let Some(paren_close) = name_and_args[paren_open + 1..].find(')') {
                            let arg =
                                name_and_args[paren_open + 1..paren_open + 1 + paren_close].trim();
                            if let Ok(n) = arg.parse::<i64>() {
                                pending_method_id = Some(n);
                            }
                        }
                    }
                }
                // Any other `@attr` (including `@inline`, `@pure`,
                // `@inline_ref`) is acknowledged and skipped — the
                // recorder doesn't care about inlining decisions, only
                // that the next `fun` line is still recognised.
            }
            i += 1;
            continue;
        }

        // Check for function definition: `fun <name>(...)`
        let after_keyword = if let Some(rest) = trimmed.strip_prefix("fun ") {
            rest
        } else {
            i += 1;
            continue;
        };

        // Method-receiver syntax: `fun (self <Type>) <name>(<params>): ...`.
        // The leading `(self <Type>)` is split off and the function is
        // registered under the qualified name `<Type>.<name>` so call
        // sites like `bag.length()` resolve via the receiver's type.
        // The `self` identifier is added as the first formal so the
        // body sees `self.field` accesses against the caller's struct.
        let (receiver_type, after_receiver) = parse_receiver_prefix(after_keyword);
        let after_keyword = after_receiver;

        // Parse function name.  Strip any generic-parameter suffix
        // (`fun max<T>(...)` registers under the bare name `max`) so
        // the call-site lookup symmetrically resolves
        // `max<int>(3, 5)` → declared `max`.  See `strip_generic_args`
        // for the semantics.
        let name_end = after_keyword.find('(').unwrap_or(after_keyword.len());
        let bare_name = strip_generic_args(after_keyword[..name_end].trim()).to_string();
        let name = match &receiver_type {
            Some(t) => format!("{t}.{bare_name}"),
            None => bare_name,
        };

        // Parse parameters.
        let mut params = if let Some(paren_start) = after_keyword.find('(') {
            if let Some(paren_end) = after_keyword.find(')') {
                let params_str = &after_keyword[paren_start + 1..paren_end];
                parse_param_list(params_str)
            } else {
                vec![]
            }
        } else {
            vec![]
        };
        // Prepend `self` as the implicit first formal for receiver
        // methods so the call-site resolver can pass the LHS through
        // unchanged.
        if let Some(rt) = &receiver_type {
            params.insert(0, ("self".to_string(), rt.clone()));
        }

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

        // The function definition line carries the opening `{`; the
        // recursive block parser starts on the next source line and
        // returns the index of the matching `}`.
        let body_start = i + 1;
        let (body, body_end) = parse_block(&lines, body_start);

        if !name.is_empty() {
            functions.push(FunctionDef {
                name,
                return_type,
                params,
                body,
                line: line_num,
                method_id: pending_method_id.take(),
                source_path: source_path.to_path_buf(),
            });
        } else {
            // No name resolved — drop any pending attribute so the
            // next valid declaration starts clean.
            pending_method_id = None;
        }

        i = body_end + 1;
    }

    functions
}

/// Parse a `{ ... }` block of statements starting at `start` (the
/// first line *inside* the block).  Returns the parsed statements plus
/// the line index of the closing `}` (so the caller can resume from
/// `closing + 1`).
///
/// Recognises the following block-introducing constructs:
///   * `if (cond) { ... }` plus chained `else if (cond) { ... }` and
///     `else { ... }`.
///   * `while (cond) { ... }`.
///   * `repeat (count) { ... }`.
///   * `do { ... } until (cond);`.
///
/// Other lines inside the block are handed to `parse_statement` so
/// `var` / `val` / `return` / `throw` / `assert` / bare assignment /
/// `<call>(...)` expression statements all surface.
fn parse_block(lines: &[&str], start: usize) -> (Vec<Statement>, usize) {
    let mut body = Vec::new();
    let mut idx = start;
    while idx < lines.len() {
        let raw_line = lines[idx];
        let trimmed = raw_line.trim();
        let line_num = (idx + 1) as u32;

        // Any line that starts with `}` closes the current block.
        // The trailing tokens (e.g. `} else if (cond) {` /
        // `} else {` / `} until (cond);`) are inspected by the
        // higher-level handler that originally called us so it can
        // chain the next block in the same construct.  We do NOT fall
        // through to statement parsing for these lines — that would
        // misclassify them as noise expressions.
        if trimmed.starts_with('}') {
            return (body, idx);
        }

        // `if (cond) {` opens a then-block; we then look for chained
        // `else if (cond) {` / `else {` clauses and fold them into
        // the same If node.
        if let Some(cond) = strip_block_header(trimmed, "if") {
            let (then_block, then_end) = parse_block(lines, idx + 1);
            // Probe for trailing `else` / `else if` clauses.  The
            // `}` line may carry a trailing `else if (...)` /
            // `else {` token (e.g. `} else if (raw == 0) {`); fall
            // back to the next line for the `else { ... }` form.
            let (else_block, advance_to) = parse_else_clauses(lines, then_end);
            body.push(Statement::If {
                cond,
                then_block,
                else_block,
                line: line_num,
            });
            idx = advance_to + 1;
            continue;
        }

        if let Some(cond) = strip_block_header(trimmed, "while") {
            let (loop_body, end) = parse_block(lines, idx + 1);
            body.push(Statement::While {
                cond,
                body: loop_body,
                line: line_num,
            });
            idx = end + 1;
            continue;
        }

        if let Some(count_expr) = strip_block_header(trimmed, "repeat") {
            let (loop_body, end) = parse_block(lines, idx + 1);
            body.push(Statement::Repeat {
                count_expr,
                body: loop_body,
                line: line_num,
            });
            idx = end + 1;
            continue;
        }

        // `match (scrutinee) {` opens a pattern-matching dispatch.
        // Each line inside is one arm of the form
        // `Constructor => { ... };` or `Constructor(payload) => { ... };`.
        // The arm body itself is parsed via `parse_block` recursively.
        if let Some(scrutinee) = strip_block_header(trimmed, "match") {
            let (arms, end) = parse_match_arms(lines, idx + 1);
            body.push(Statement::Match {
                scrutinee,
                arms,
                line: line_num,
            });
            idx = end + 1;
            continue;
        }

        // `do {` opens a loop body that ends with `} until (cond);`.
        // The `}` line itself carries the `until (cond);` suffix that
        // we need to capture as the loop's condition.
        if trimmed == "do" || trimmed == "do {" {
            let (loop_body, end_line) = parse_block(lines, idx + 1);
            // The closing `}` may have a trailing `until (cond);`.
            let close_line = lines[end_line].trim();
            // Strip the leading `}`, then `until`, then the
            // parenthesised condition.
            let after_brace = close_line.strip_prefix('}').unwrap_or(close_line).trim();
            let cond = if let Some(rest) = after_brace.strip_prefix("until") {
                let rest = rest.trim();
                rest.strip_prefix('(')
                    .and_then(|r| r.rfind(')').map(|p| &r[..p]))
                    .map(|c| c.trim().to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            body.push(Statement::DoUntil {
                body: loop_body,
                cond,
                line: line_num,
            });
            idx = end_line + 1;
            continue;
        }

        // Fall back to statement parsing for non-block lines.
        // Multi-line literal support: if the trimmed line opens a
        // bracket pair (`(`, `[`, `{`) that is not closed on the same
        // line, join successive lines until the brackets balance.  This
        // lets `var p: Point = Point {\n    x: 3,\n    y: 4\n};` parse
        // as a single VarBinding whose RHS is the equivalent
        // single-line struct literal.  We only consume continuation
        // lines (no `}`-only lines that would close an enclosing
        // block); the join stops at the line whose net depth returns to
        // zero.
        let (joined, lines_consumed) = gather_logical_line(lines, idx);
        if lines_consumed > 1 {
            if let Some(stmt) = parse_statement(&joined, line_num) {
                body.push(stmt);
            }
            idx += lines_consumed;
            continue;
        }
        if let Some(stmt) = parse_statement(trimmed, line_num) {
            body.push(stmt);
        }
        idx += 1;
    }
    (body, idx)
}

/// Track bracket depth across a starting line and return the joined
/// text plus the number of source lines consumed.  Returns
/// `(<single-line>, 1)` when the starting line's bracket depth is
/// already balanced (the common case); otherwise keeps appending
/// successive lines until the running depth returns to zero (or we
/// hit a line that would CLOSE a higher-level block — `}` at depth 0
/// — in which case we stop early to avoid swallowing the enclosing
/// block).
///
/// Used by `parse_block` to support multi-line struct / tuple
/// literals (`Point {\n    x: 3,\n    y: 4\n};` and
/// `(\n    a,\n    b,\n    c\n)`) that the strict
/// `parse_statement` path would otherwise reject as unclosed
/// expressions.
fn gather_logical_line(lines: &[&str], start: usize) -> (String, usize) {
    let first = lines[start].trim();
    let depth = bracket_depth(first);
    if depth == 0 {
        return (first.to_string(), 1);
    }
    let mut joined = first.to_string();
    let mut running = depth;
    let mut consumed = 1usize;
    let mut i = start + 1;
    while i < lines.len() && running != 0 {
        let next = lines[i].trim();
        // Don't swallow an enclosing block's closing brace.  A standalone
        // `}` at the start of a line drops the running depth by 1; we
        // only follow it if our current literal is still unbalanced
        // (running > 0), which is exactly the multi-line literal case.
        joined.push(' ');
        joined.push_str(next);
        running += bracket_depth(next);
        consumed += 1;
        i += 1;
        if running == 0 {
            break;
        }
    }
    (joined, consumed)
}

/// Net bracket-depth change across a single line, counting `(`, `[`,
/// and `{` as +1 and the matching closers as -1.  Strings and comments
/// are NOT special-cased — the recorder's source corpus is small and
/// hand-written, so the simpler accounting is sufficient.
fn bracket_depth(line: &str) -> i32 {
    let mut d = 0i32;
    for &b in line.as_bytes() {
        match b {
            b'(' | b'[' | b'{' => d += 1,
            b')' | b']' | b'}' => d -= 1,
            _ => {}
        }
    }
    d
}

/// Parse zero or more `else` / `else if` clauses that may follow an
/// `if` block's closing `}`.  Returns the chained `else_block` (which
/// is either empty, a single-element `Vec` containing another `If`,
/// or the body of the terminal plain `else { ... }`) plus the line
/// index where the chain ends (so the caller resumes from
/// `end + 1`).
fn parse_else_clauses(lines: &[&str], close_idx: usize) -> (Vec<Statement>, usize) {
    let close_line = lines[close_idx].trim();
    // Case 1: `} else if (cond) {` on the same line.
    if let Some(rest) = close_line.strip_prefix('}') {
        let rest = rest.trim();
        if let Some(cond) = rest
            .strip_prefix("else if")
            .or_else(|| rest.strip_prefix("else  if"))
        {
            // Recover the parenthesised condition.
            let cond_text = strip_paren_block_header(cond.trim());
            if let Some(cond) = cond_text {
                let (then_block, end) = parse_block(lines, close_idx + 1);
                let line = (close_idx + 1) as u32;
                let (chained_else, end2) = parse_else_clauses(lines, end);
                return (
                    vec![Statement::If {
                        cond,
                        then_block,
                        else_block: chained_else,
                        line,
                    }],
                    end2,
                );
            }
        }
        if rest == "else {" || rest == "else{" {
            let (else_block, end) = parse_block(lines, close_idx + 1);
            return (else_block, end);
        }
    }
    // Case 2: `}` on one line, `else if (...)` / `else {` on the next.
    if close_idx + 1 < lines.len() {
        let next = lines[close_idx + 1].trim();
        if let Some(cond) = next
            .strip_prefix("else if")
            .or_else(|| next.strip_prefix("else  if"))
        {
            let cond_text = strip_paren_block_header(cond.trim());
            if let Some(cond) = cond_text {
                let (then_block, end) = parse_block(lines, close_idx + 2);
                let line = (close_idx + 2) as u32;
                let (chained_else, end2) = parse_else_clauses(lines, end);
                return (
                    vec![Statement::If {
                        cond,
                        then_block,
                        else_block: chained_else,
                        line,
                    }],
                    end2,
                );
            }
        }
        if next == "else {" || next == "else{" {
            let (else_block, end) = parse_block(lines, close_idx + 2);
            return (else_block, end);
        }
    }
    (Vec::new(), close_idx)
}

/// Parse the body of a `match (scrutinee) { ... }` block.  Each arm
/// is one source line of the shape:
///
///   `Constructor => { <stmt>; <stmt>; };`
///   `Constructor(payload) => { <stmt>; ... };`
///
/// Multi-line arm bodies are also accepted (the joiner gathers
/// continuations until the brace pair balances).  Returns the
/// collected `MatchArm`s plus the line index of the closing `}`.
///
/// The recorder only consumes one statement per arm body in
/// practice (the canonical shape is `Active(n) => { result = n; };`),
/// but the parser is happy to take a multi-statement arm — the
/// runtime arm-execution path walks the full statement list.
fn parse_match_arms(lines: &[&str], start: usize) -> (Vec<MatchArm>, usize) {
    let mut arms = Vec::new();
    let mut idx = start;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.starts_with('}') {
            return (arms, idx);
        }
        if trimmed.is_empty() {
            idx += 1;
            continue;
        }
        // Gather any continuation lines so a multi-line arm body
        // (`Active(n) => {\n    result = n;\n};`) joins into a
        // single logical line.
        let (joined, consumed) = gather_logical_line(lines, idx);
        let line_num = (idx + 1) as u32;
        if let Some(arm) = parse_match_arm(&joined, line_num) {
            arms.push(arm);
        }
        idx += consumed;
    }
    (arms, idx)
}

/// Parse a single match arm of the form
/// `Constructor[(<binding>)] => { <body> };` (the trailing `;` is
/// optional).  Returns `None` if the line doesn't match.
fn parse_match_arm(line: &str, line_num: u32) -> Option<MatchArm> {
    let s = line.trim().trim_end_matches(';').trim();
    let arrow = s.find("=>")?;
    let lhs = s[..arrow].trim();
    let rhs = s[arrow + 2..].trim();
    // LHS shape: `Constructor` or `Constructor(<binding>)`.
    let (constructor, payload_binding) = if let Some(paren_open) = lhs.find('(') {
        let ctor = lhs[..paren_open].trim().to_string();
        let inner = lhs[paren_open + 1..].trim_end_matches(')').trim();
        if !is_simple_identifier(inner) {
            return None;
        }
        (ctor, Some(inner.to_string()))
    } else {
        (lhs.to_string(), None)
    };
    if !is_simple_identifier(&constructor) {
        return None;
    }
    // RHS: must be a `{ ... }` brace block.  Strip the outer braces
    // and parse the inner statements via the same per-line driver.
    let inner = rhs.strip_prefix('{')?.trim();
    let inner = inner.strip_suffix('}')?.trim();
    let mut body = Vec::new();
    for stmt_text in inner.split(';') {
        let stmt_text = stmt_text.trim();
        if stmt_text.is_empty() {
            continue;
        }
        // Re-attach the trailing `;` so `parse_statement` recognises
        // the shape (it strips it itself, but several arms only
        // accept the `<...>;` form).
        let with_semi = format!("{stmt_text};");
        if let Some(stmt) = parse_statement(&with_semi, line_num) {
            body.push(stmt);
        }
    }
    Some(MatchArm {
        constructor,
        payload_binding,
        body,
        line: line_num,
    })
}

/// Recognise a block-introducing line of the shape
/// `<keyword> (<expr>) {` and return the parenthesised expression.
/// Returns `None` if the line doesn't match the shape exactly.
fn strip_block_header(line: &str, keyword: &str) -> Option<String> {
    let rest = line.strip_prefix(keyword)?.trim_start();
    let rest = rest.strip_prefix('(')?;
    // Find the matching close paren at depth 0; everything after
    // must be `{` (optionally with whitespace).
    let mut depth = 1i32;
    let mut close: Option<usize> = None;
    let bytes = rest.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let cond = rest[..close].trim().to_string();
    let after = rest[close + 1..].trim();
    if after != "{" {
        return None;
    }
    Some(cond)
}

/// Strip a `(<expr>) {` suffix and return `<expr>`.  Used to recover
/// the condition of an `else if` clause whose preceding tokens have
/// already been consumed.
fn strip_paren_block_header(rest: &str) -> Option<String> {
    let rest = rest.strip_prefix('(')?;
    let mut depth = 1i32;
    let mut close: Option<usize> = None;
    let bytes = rest.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let cond = rest[..close].trim().to_string();
    let after = rest[close + 1..].trim();
    if after != "{" {
        return None;
    }
    Some(cond)
}

/// Scan a Tolk source for `type X = A | B | C;` union declarations
/// and return the set of right-hand-side variant names.  Each name
/// is treated as a variant constructor by the value-emission path:
/// when a constructor with one of these names appears (e.g.
/// `Active { n: 7 }`), the recorder emits `ValueRecord::Variant`
/// instead of `ValueRecord::Struct`.  Whitespace inside the union
/// body is normalised; trailing semicolons are tolerated.  Names
/// must be simple identifiers (Tolk's variant tags are constructor
/// names, not parameterised types).
fn parse_variant_constructors(source: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for raw in source.lines() {
        let line = raw.trim();
        let after_type = match line.strip_prefix("type ") {
            Some(s) => s,
            None => continue,
        };
        let eq_pos = match after_type.find('=') {
            Some(p) => p,
            None => continue,
        };
        let rhs = after_type[eq_pos + 1..].trim().trim_end_matches(';').trim();
        for part in rhs.split('|') {
            let name = part.trim();
            // Tolk allows `Active(int)` style payload signatures in
            // the union; we strip everything past the first `(`.
            let name = name.split('(').next().unwrap_or(name).trim();
            if !name.is_empty() && is_simple_identifier(name) {
                out.insert(name.to_string());
            }
        }
    }
    out
}

/// Recognise an optional `(self <Type>)` receiver prefix on a `fun `
/// declaration tail.  Returns `(<Type>, <rest-after-receiver>)` when
/// the prefix matches, or `(None, <unchanged-tail>)` when the
/// declaration doesn't carry one.  Tolk's exact source-level shape is
/// `fun (self Bag) length(): int { ... }` — the parser consumes the
/// `(self Bag) ` segment so the downstream "find name + params"
/// pipeline sees the conventional `length(): int { ... }` tail.
fn parse_receiver_prefix(rest: &str) -> (Option<String>, &str) {
    let rest = rest.trim_start();
    let inner_start = match rest.strip_prefix('(') {
        Some(s) => s,
        None => return (None, rest),
    };
    let close_pos = match inner_start.find(')') {
        Some(p) => p,
        None => return (None, rest),
    };
    let inner = inner_start[..close_pos].trim();
    let mut parts = inner.split_whitespace();
    let head = match parts.next() {
        Some(h) => h,
        None => return (None, rest),
    };
    if head != "self" {
        return (None, rest);
    }
    let type_name = match parts.next() {
        Some(t) => t.to_string(),
        None => return (None, rest),
    };
    if parts.next().is_some() {
        return (None, rest);
    }
    let after = inner_start[close_pos + 1..].trim_start();
    (Some(type_name), after)
}

/// Synthesise a representative actual for a contract entry-point
/// formal parameter, honouring the declared TVM type so the body's
/// downstream operations (`loadAddress()`, `myBalance + msgValue`,
/// `msgBody.loadInt(32)`) compute meaningful results instead of
/// degenerating to zero-default placeholders.
///
/// `msg_op_code` is the per-message override threaded by the
/// multi-message dispatcher (see `evaluate_program`): when present
/// it becomes the first int payload of the synthesised `msgBody:
/// slice`, so the body's `op = msgBody.loadInt(32)` dispatch routes
/// through a different branch on each call.  When absent (the
/// single-shot path), `slice` synthesises empty.
///
/// The exact integer values are recorder-side conventions chosen for
/// strict-test pinning: `coin = 1_000_000_000` (one TON in nanoton),
/// `address = (0, 0xBEEF)`, `cell = [0xCAFE]` so `cell.beginParse().
/// loadAddress()` returns 51966 = 0xCAFE.
fn synthesize_entry_arg(param_type: &str, msg_op_code: Option<i64>) -> Value {
    match param_type {
        "coin" => Value::Coin(1_000_000_000),
        "address" => Value::Address {
            workchain: 0,
            hash: 0xBEEF,
        },
        "cell" => Value::Cell {
            payload: vec![0xCAFE],
            refs: Vec::new(),
        },
        "slice" => {
            let payload = match msg_op_code {
                Some(op) => vec![op],
                None => Vec::new(),
            };
            Value::Slice {
                payload,
                pos: 0,
                refs: Vec::new(),
                ref_pos: 0,
            }
        }
        _ => Value::Int(0),
    }
}

/// Parse the optional `// @recorder:messages <int> <int> ...`
/// directive that opts a fixture into the multi-message dispatcher.
/// Returns `Some(vec![op0, op1, ...])` when the directive is
/// present and parses cleanly, `None` otherwise (the single-shot
/// path).  Each integer literal can be decimal (`1`), hex
/// (`0x01`), or negative (`-1`).
///
/// The directive lives inside the entry-file source text (passed in
/// verbatim).  Whitespace separates the op codes; commas and
/// trailing punctuation are tolerated.
fn parse_multi_message_directive(source: Option<&str>) -> Option<Vec<i64>> {
    let source = source?;
    for line in source.lines() {
        let trimmed = line.trim_start();
        let body = trimmed
            .strip_prefix("//")
            .or_else(|| trimmed.strip_prefix(";;"))?
            .trim_start();
        let body = body.strip_prefix("@recorder:messages")?.trim();
        let mut codes = Vec::new();
        for tok in body.split(|c: char| c.is_whitespace() || c == ',') {
            let tok = tok.trim();
            if tok.is_empty() {
                continue;
            }
            let parsed =
                if let Some(hex) = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")) {
                    i64::from_str_radix(hex, 16).ok()
                } else {
                    tok.parse::<i64>().ok()
                };
            match parsed {
                Some(n) => codes.push(n),
                None => return None,
            }
        }
        return Some(codes);
    }
    None
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

    // var/val binding.  Two accepted shapes:
    //   * `var <name>: <type> = <expr>;` / `val <name>: <type> = <expr>;`
    //   * `var <name> = <expr>;` (typeless — Tolk allows this for
    //     Cell / Slice / Builder bindings; the recorder defaults the
    //     declared type to `int` for trace emission so the value still
    //     round-trips through `register_variable_with_full_value`).
    //
    // To distinguish the two we look at whether a `:` precedes the
    // first top-level `=` sign.  If yes, the type annotation lives
    // between them; if no, the binding is typeless.
    if let Some(after_keyword) = trimmed
        .strip_prefix("var ")
        .or_else(|| trimmed.strip_prefix("val "))
    {
        if let Some(eq_pos) = find_top_level_assign(after_keyword) {
            let lhs = after_keyword[..eq_pos].trim();
            let expr = after_keyword[eq_pos + 1..]
                .trim()
                .trim_end_matches(';')
                .trim()
                .to_string();
            if !expr.is_empty() {
                if let Some(colon_pos) = lhs.find(':') {
                    let name = lhs[..colon_pos].trim().to_string();
                    let type_name = lhs[colon_pos + 1..].trim().to_string();
                    if !name.is_empty() && !type_name.is_empty() {
                        return Some(Statement::VarBinding {
                            name,
                            type_name: Some(type_name),
                            expr,
                            line: line_num,
                        });
                    }
                } else {
                    let name = lhs.to_string();
                    if !name.is_empty() && is_simple_identifier(&name) {
                        return Some(Statement::VarBinding {
                            name,
                            type_name: None,
                            expr,
                            line: line_num,
                        });
                    }
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

    // throwIf / throwUnless: `throwIf(<code>, <cond>);` and
    // `throwUnless(<code>, <cond>);` — Tolk's gated-throw guard idiom
    // (the pervasive shape behind `throwIf(40, recipient == sender);`
    // and `throwUnless(36, msg::value() >= price);` in real-world
    // contracts).  Both must evaluate the condition at runtime and
    // only fire the matching Error io_event when the gate trips;
    // implementation lives in the `Statement::GatedThrow` arm of
    // `execute_statement`.  Be lenient about whitespace between the
    // function name and the opening paren so both
    // `throwIf(...)` and `throwIf (...)` parse.
    for (prefix, mode) in [
        ("throwIf(", GateMode::If),
        ("throwIf (", GateMode::If),
        ("throwUnless(", GateMode::Unless),
        ("throwUnless (", GateMode::Unless),
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            // Strip the trailing `);` (and any extra semicolons) so
            // we're left with the bare arg pair.
            let inner = rest
                .trim()
                .trim_end_matches(';')
                .trim()
                .trim_end_matches(')')
                .trim();
            // Split on the FIRST top-level comma — the first arg is
            // the error code, the second is the condition expression
            // (which may itself contain commas inside calls, e.g.
            // `throwUnless(36, has_at_least(balance, price))`).
            let parts = split_top_level_commas(inner);
            if parts.len() >= 2 {
                let code = parts[0].trim().to_string();
                let condition = parts[1..].join(", ").trim().to_string();
                if !code.is_empty() && !condition.is_empty() {
                    return Some(Statement::GatedThrow {
                        code,
                        condition,
                        mode,
                        line: line_num,
                    });
                }
            }
        }
    }

    // bare assignment: `<name> = <expr>;`.  The LHS must be a simple
    // identifier (loop counters, accumulators, branch-arm sinks); we
    // do not support qualified or indexed targets here because the
    // env is a flat `HashMap<String, Value>`.  Compound operators
    // (`+=`, `-=`, …) are normalised to plain `=` by their respective
    // parse arms; a bare `=` line with an identifier LHS is the only
    // shape recognised here.
    if let Some(eq_pos) = find_top_level_assign(trimmed) {
        let lhs = trimmed[..eq_pos].trim();
        let rhs = trimmed[eq_pos + 1..]
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string();
        if is_simple_identifier(lhs) && !rhs.is_empty() {
            return Some(Statement::Assign {
                name: lhs.to_string(),
                expr: rhs,
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

    // Expression statement: a bare `<call>(...);` line invoked for
    // its side effect.  Used by `set_data(c);` and similar storage-
    // mutation calls.  We only register lines whose stripped form
    // looks like a function call (ends with `)` after stripping the
    // trailing semicolon) so we don't accidentally swallow noise.
    let body = trimmed.trim_end_matches(';').trim();
    if body.ends_with(')') && body.contains('(') && !body.starts_with('{') {
        return Some(Statement::ExprStatement {
            expr: body.to_string(),
            line: line_num,
        });
    }

    None
}

/// Find the position of the first top-level `=` sign that is NOT part
/// of a comparison / equality / inequality operator (`==`, `!=`,
/// `<=`, `>=`).  Returns `None` if no such sign exists.
fn find_top_level_assign(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'=' if depth == 0 => {
                // Skip `==`.
                let next = bytes.get(i + 1).copied();
                if next == Some(b'=') {
                    i += 2;
                    continue;
                }
                // Skip `!=`, `<=`, `>=`.
                let prev = if i == 0 { None } else { Some(bytes[i - 1]) };
                if prev == Some(b'!')
                    || prev == Some(b'<')
                    || prev == Some(b'>')
                    || prev == Some(b'=')
                {
                    i += 1;
                    continue;
                }
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Check if an expression is a simple function call like `compute()`.
/// Returns the function name if so.
///
/// Retained for unit-test coverage of the legacy zero-arg recognition
/// path; production call sites use `parse_call_with_args` so they can
/// thread positional arguments through to the callee (see the M10
/// `arg_passing_test.tolk` fixture).
#[cfg(test)]
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

/// Parse a top-level call shape `<ident>(<args>)` into the function
/// name plus a `Vec<String>` of trimmed argument expressions.  Returns
/// `None` for non-call expressions, or when the identifier portion
/// contains anything other than the usual `[A-Za-z_][A-Za-z0-9_]*`
/// alphabet (so we don't accidentally match `a + b()` as a call to
/// `a + b`).
fn parse_call_with_args(expr: &str) -> Option<(&str, Vec<String>)> {
    let expr = expr.trim();
    if !expr.ends_with(')') {
        return None;
    }
    // Find the position of the matching `(` by walking from the right
    // and counting parens.  The first `(` we encounter at depth 1 (we
    // start at depth 0 for the trailing `)`) is the splitter.
    let bytes = expr.as_bytes();
    let mut depth = 0i32;
    let mut open_pos: Option<usize> = None;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                depth -= 1;
                if depth == 0 {
                    open_pos = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let open = open_pos?;
    let raw_name = expr[..open].trim();
    if raw_name.is_empty() {
        return None;
    }
    // Tolk's monomorphised call-site syntax is `max<int>(3, 5)` —
    // strip the trailing `<...>` group so the lookup matches the
    // declared `max(...)`.  See `strip_generic_args` for the
    // semantics; non-generic call sites pass through unchanged.
    let name = strip_generic_args(raw_name);
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_')
        || !name
            .chars()
            .next()
            .map(|c| c.is_alphabetic() || c == '_')
            .unwrap_or(false)
    {
        return None;
    }
    let inner = &expr[open + 1..expr.len() - 1];
    let args = if inner.trim().is_empty() {
        Vec::new()
    } else {
        split_top_level_commas(inner)
    };
    Some((name, args))
}

/// Parse a method-call shape `<lhs>.<method>(<args>)` into
/// `(<lhs-expr>, <method-name>, <arg-strings>)`.  The split point is
/// the rightmost top-level dot that precedes a `<method>(<args>)`
/// call segment, so chained calls like `beginCell().storeInt(42, 32)
/// .endCell()` parse as `(beginCell().storeInt(42, 32), endCell, [])`
/// and the LHS recurses through the same resolver.
fn parse_method_call(expr: &str) -> Option<(&str, &str, Vec<String>)> {
    let expr = expr.trim();
    if !expr.ends_with(')') {
        return None;
    }
    // Find the matching open paren (the one that pairs with the final `)`).
    let bytes = expr.as_bytes();
    let mut depth = 0i32;
    let mut open_pos: Option<usize> = None;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                depth -= 1;
                if depth == 0 {
                    open_pos = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let open = open_pos?;
    // The character immediately before the `(` must end a method
    // identifier; we then walk left to the dot.
    let pre_call = expr[..open].trim_end();
    let dot_pos = pre_call.rfind('.')?;
    let method = pre_call[dot_pos + 1..].trim();
    if method.is_empty()
        || !method.chars().all(|c| c.is_alphanumeric() || c == '_')
        || !method
            .chars()
            .next()
            .map(|c| c.is_alphabetic() || c == '_')
            .unwrap_or(false)
    {
        return None;
    }
    // The dot we found must be at top level (depth 0) in the original
    // expression — otherwise we'd be slicing in the middle of a
    // parenthesised arg.
    let dot_abs = dot_pos;
    let prefix_bytes = &expr.as_bytes()[..dot_abs];
    let mut d = 0i32;
    for &b in prefix_bytes {
        match b {
            b'(' | b'[' | b'{' => d += 1,
            b')' | b']' | b'}' => d -= 1,
            _ => {}
        }
    }
    if d != 0 {
        return None;
    }
    let lhs = expr[..dot_abs].trim();
    if lhs.is_empty() {
        return None;
    }
    let inner = &expr[open + 1..expr.len() - 1];
    let args = if inner.trim().is_empty() {
        Vec::new()
    } else {
        split_top_level_commas(inner)
    };
    Some((lhs, method, args))
}

/// Split a string on top-level commas (commas at depth 0 across all
/// bracket flavours).  Each element is trimmed; an all-whitespace input
/// yields an empty `Vec`.
///
/// Tracks `()`, `[]`, and `{}` together so structured literals nested
/// inside an outer expression are passed through atomically.
fn split_top_level_commas(s: &str) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim().to_string());
    out
}

/// Recognise an identifier (alphanumeric + underscore, starting with a
/// letter or underscore).  Used by `eval_expr_to_value` to short-circuit
/// the TVM round-trip when the expression is a bare variable reference
/// whose env value is already a `Value` (so structured shapes survive
/// the lookup instead of being projected to `i64` and re-lifted to
/// `Value::Int`).
fn is_simple_identifier(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// Strip a Tolk generic-argument suffix (`<...>`) from a function- or
/// type-name token.  Tolk's parametric polymorphism surface is
/// Rust-flavoured: `fun max<T>(...)` declares the generic helper, and
/// `max<int>(3, 5)` instantiates it with `T = int` at the call site.
/// At the recorder level we don't carry per-instantiation type info
/// (the trace's `ValueRecord` shape is a function of the runtime
/// `Value`, not the source-level type variable), so we collapse every
/// monomorphised name back to the bare ident.  This makes the lookup
/// in the function map symmetric: both `parse_functions` (declaration
/// side) and `parse_call_with_args` / `parse_struct_literal`
/// (call/literal side) call this helper before consulting the map.
///
/// We strip a single trailing `<...>` group, properly bracket-balanced
/// so we don't mis-parse a comparison like `a < b`.  Multi-arg
/// generics (`Map<K, V>`) and nested ones (`Vec<Box<int>>`) collapse
/// to the bare ident in one pass.  Returns the original token
/// unchanged if no `<` is present or the brackets don't balance.
fn strip_generic_args(s: &str) -> &str {
    let trimmed = s.trim();
    let bytes = trimmed.as_bytes();
    if !trimmed.ends_with('>') {
        return trimmed;
    }
    // Walk from the right counting `>` / `<` pairs.  The matching `<`
    // for the trailing `>` is the splitter; everything before it is
    // the bare ident.
    let mut depth = 0i32;
    let mut open_pos: Option<usize> = None;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b'>' => depth += 1,
            b'<' => {
                depth -= 1;
                if depth == 0 {
                    open_pos = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    match open_pos {
        Some(p) => trimmed[..p].trim_end(),
        None => trimmed,
    }
}

/// Find a top-level `.` separator between a left-hand expression and a
/// single trailing field name / numeric index.  Used by
/// `eval_expr_to_value` to recognise field-access expressions like `p.x`
/// (struct field) or `pair.0` (tuple positional access).
///
/// Returns `(<lhs>, <field>)` when the input has the shape
/// `<expr>.<simple-name-or-digits>` at top level (i.e. the dot is at
/// depth 0 across all bracket flavours).  Returns `None` for non-
/// matching shapes — including chained accesses (`a.b.c`), arithmetic
/// with `.` (we don't support floats), or anything where the field side
/// isn't a simple ident / digit run.
fn split_top_level_dot(expr: &str) -> Option<(&str, &str)> {
    let expr = expr.trim();
    let bytes = expr.as_bytes();
    let mut depth = 0i32;
    let mut last_dot: Option<usize> = None;
    for i in 0..bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'.' if depth == 0 => last_dot = Some(i),
            _ => {}
        }
    }
    let pos = last_dot?;
    let lhs = expr[..pos].trim();
    let field = expr[pos + 1..].trim();
    if lhs.is_empty() || field.is_empty() {
        return None;
    }
    let valid_field = field.chars().all(|c| c.is_alphanumeric() || c == '_')
        && field
            .chars()
            .next()
            .map(|c| c.is_alphanumeric() || c == '_')
            .unwrap_or(false);
    if !valid_field {
        return None;
    }
    Some((lhs, field))
}

/// Recognise a Tolk tuple literal `(a, b[, c...])` and return the
/// element-expression strings.  Returns `None` for non-tuple shapes
/// — including unit `()` and parenthesised single expressions `(x)`
/// (only `(a, b)` and longer count as tuple literals).
fn parse_tuple_literal(expr: &str) -> Option<Vec<String>> {
    let expr = expr.trim();
    let inner = expr.strip_prefix('(')?.strip_suffix(')')?;
    // Top-level paren match: the trailing `)` must close the leading
    // `(` at depth 0, with nothing past it.
    let bytes = expr.as_bytes();
    let mut depth = 0i32;
    for (i, ch) in bytes.iter().enumerate() {
        match ch {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 && i != expr.len() - 1 {
                    return None;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    let parts = split_top_level_commas(inner);
    if parts.len() < 2 {
        return None;
    }
    Some(parts)
}

/// Recognise a Tolk source-level string slice literal.  Returns the
/// resulting `Value::SliceLit` for either the UTF-8 text shape
/// (`"abc"`) or the hex shape (`0x"abcd"`); returns `None` for
/// non-literal expressions.
///
/// We accept only the bare literal text — no concatenation, no
/// escape sequences past the most basic ones (`\\`, `\"`).  Real-
/// world Tolk supports a richer escape grammar (`\n`, `\t`,
/// `\xNN`, …); the small set above is enough for the corpus we
/// need to record today and the helper degrades gracefully for
/// unrecognised escapes (it leaves them in the byte stream so the
/// trace still surfaces something tractable).
fn parse_slice_literal(expr: &str) -> Option<Value> {
    let s = expr.trim();
    if s.len() < 2 {
        return None;
    }
    // Hex shape: `0x"..."`.
    if let Some(rest) = s.strip_prefix("0x\"").or_else(|| s.strip_prefix("0X\"")) {
        let body = rest.strip_suffix('"')?;
        // Strip whitespace from the body so `0x"ab cd"` (rare but
        // legal) round-trips like `0x"abcd"`.  Reject any character
        // outside `[0-9a-fA-F]` so we don't accidentally accept a
        // typo'd literal.
        let mut clean = String::with_capacity(body.len());
        for ch in body.chars() {
            if ch.is_ascii_whitespace() {
                continue;
            }
            if !ch.is_ascii_hexdigit() {
                return None;
            }
            clean.push(ch);
        }
        // An odd nibble count means the literal is half a byte; we
        // pad with a trailing `0` so the byte stream stays
        // well-defined (matches what TON's slice constructor does
        // for `0x"abc"` style literals).
        if clean.len() % 2 == 1 {
            clean.push('0');
        }
        let mut bytes = Vec::with_capacity(clean.len() / 2);
        let mut chars = clean.chars();
        while let (Some(hi), Some(lo)) = (chars.next(), chars.next()) {
            let hi = hi.to_digit(16)? as u8;
            let lo = lo.to_digit(16)? as u8;
            bytes.push((hi << 4) | lo);
        }
        return Some(Value::SliceLit {
            encoding: SliceEncoding::Hex,
            bytes,
        });
    }
    // Text shape: `"..."`.
    if let Some(rest) = s.strip_prefix('"') {
        let body = rest.strip_suffix('"')?;
        // Cheap escape pass: handle only `\\`, `\"`, `\n`, `\t`.
        let mut bytes = Vec::with_capacity(body.len());
        let mut chars = body.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('\\') => bytes.push(b'\\'),
                    Some('"') => bytes.push(b'"'),
                    Some('n') => bytes.push(b'\n'),
                    Some('t') => bytes.push(b'\t'),
                    Some(other) => {
                        bytes.push(b'\\');
                        let mut buf = [0u8; 4];
                        bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                    }
                    None => bytes.push(b'\\'),
                }
            } else {
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
        }
        return Some(Value::SliceLit {
            encoding: SliceEncoding::Text,
            bytes,
        });
    }
    None
}

/// Recognise a Tolk struct literal `Type { field: value, ... }` and
/// return the type name plus a `Vec<(field_name, value_expr)>`.
///
/// The type name must be a simple identifier starting with an upper-
/// case letter (Tolk convention — `Point`, `Coord`, etc.).  Each field
/// entry must have the shape `<simple-ident>: <expr>` separated by
/// top-level commas.  Returns `None` for non-struct-shaped input.
fn parse_struct_literal(expr: &str) -> Option<(String, Vec<(String, String)>)> {
    let expr = expr.trim();
    let brace_open = expr.find('{')?;
    if !expr.ends_with('}') {
        return None;
    }
    // Strip a generic-instantiation suffix (`Box<int> { ... }` →
    // `Box`) so the recorder registers the literal under the bare
    // type name — same convention as the function-name resolver, so
    // `value_to_record` can look the type id up by the canonical
    // `Box`/`Pair`/`Status` etc.  See `strip_generic_args`.
    let type_name = strip_generic_args(expr[..brace_open].trim()).to_string();
    if type_name.is_empty() || !is_simple_identifier(&type_name) {
        return None;
    }
    let first = type_name.chars().next().unwrap();
    if !first.is_uppercase() {
        return None;
    }
    let inner = &expr[brace_open + 1..expr.len() - 1];
    let parts = split_top_level_commas(inner);
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        let colon = p.find(':')?;
        let fname = p[..colon].trim().to_string();
        let fexpr = p[colon + 1..].trim().to_string();
        if fname.is_empty() || fexpr.is_empty() || !is_simple_identifier(&fname) {
            return None;
        }
        out.push((fname, fexpr));
    }
    Some((type_name, out))
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
        let functions = parse_functions(source, Path::new("test.tolk"));
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
                assert_eq!(type_name, Some("int".to_string()));
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
                assert_eq!(type_name, Some("bool".to_string()));
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
        let functions = parse_functions(source, Path::new("test.tolk"));
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
