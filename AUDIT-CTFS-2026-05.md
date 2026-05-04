# TON / Tolk Recorder CTFS Audit — 2026-05-02

This audit checks `codetracer-ton-recorder` against the canonical
CodeTracer multi-stream CTFS schema and the section 5.6 audit checklist
maintained in `/tmp/isonim-migration.txt`. Prior audits set the
canonical patterns: Ruby (1.21, 1.22), Python (1.27), JavaScript (1.38),
EVM (1.39), PHP (1.41), Solana (1.44), Move (1.46), Cardano (1.48),
Cairo (1.50), Flow / Cadence (1.52), Fuel / Sway (1.53), PolkaVM (1.55),
and Miden (1.56). This is the **fourteenth** recorder audited.

## Architecture

The TON recorder is a **single-process Rust crate** that embeds
`tycho-vm` 0.3 (a Rust implementation of the TON Virtual Machine):

* `tracer.rs` (`TolkTracer::trace_program`) parses a `.tolk` source
  file with a hand-rolled line-based parser (function definitions,
  `var`/`val` bindings, and `return` statements), then walks the parsed
  AST and emits canonical CodeTracer events through the Rust-native
  `NimTraceWriter` (the `codetracer_trace_writer_nim` sibling-path
  crate).
* Each `var <name>: <type> = <expr>;` binding compiles `<expr>` to TVM
  bytecode (`PUSHINT` / `ADD` / `SUB` / `MUL` / `DIV` / `MOD` /
  `EQUAL` / `LESS` / etc.) via `tvm.rs::TvmProgram`, runs it on a real
  `tycho_vm::VmState`, and registers the resulting integer through
  `register_variable_with_full_value`.
* Calls and returns are detected at the AST level: function-call
  expressions like `compute()` recurse into `evaluate_function`, which
  emits `register_call(fn_id, vec![])` on entry and `register_return`
  on exit. The entry-point function (`main`) is merged into
  `<toplevel>` (no `Call` event) so its body stays at depth 0 in the
  calltrace pane.
* `sandbox.rs` implements the `trace-sandbox` subcommand: it parses
  `@ton/sandbox` `vm_logs_full` output (instruction + gas + stack
  triples plus an optional terminal `exit code:` line) and emits one
  step per instruction with the top-of-stack as a `tos` variable.
* `replay.rs` implements the `replay` subcommand: a placeholder
  `LiteserverClient` shape that returns an "Liteserver communication
  not yet implemented" error today; once Liteserver / `tonlib` /
  `adnl` integration lands, the same trace-writer pipeline is in
  place.
* `stack_tracker.rs` provides a symbolic stack tracker that mirrors
  TVM operand-stack manipulations and propagates source-level
  variable names through derived expressions (used by `tracer.rs` to
  reconstruct names like `"a + b"` for intermediate values).
* `source_map.rs` provides byte-offset-to-line mapping for the
  source file.

The recorder is **not** an FFI consumer — every canonical entry point
(`register_call`, `register_step`, `register_special_event`, `arg`,
`register_thread_*`) is reachable through the
`codetracer_trace_writer_nim` Rust API. There are no `#[no_mangle]`
stubs and `add_event` does not appear in the source.

Architecturally closest to: Miden 1.56 (single-process Rust crate
embedding the VM via a sibling crate; stack-machine VM) and PolkaVM
1.55.

## Summary

| # | Check | Status (pre-fix) | Status (post-fix) | Notes |
|---|---|---|---|---|
| a | CLI defaults to `TraceEventsFileFormat::Ctfs` | **GAP** | **OK** | Pre-fix `src/main.rs`'s `OutputFormat` enum exposed only `Binary` (legacy CBOR + Zstd) and `Json`, with `Binary` as the default for all three subcommands (`record`, `replay`, `trace-sandbox`). The canonical CTFS multi-stream container — the one the Nim `ct_reader_*` FFI and the db-backend's `CTFSTraceReader` consume directly — was not selectable from the CLI at all. Post-fix the enum gains a `Ctfs` variant (listed first), with doc-comments on each option, plus an `impl From<OutputFormat> for TraceEventsFileFormat` so each of the three dispatch sites collapses to `let format: TraceEventsFileFormat = args.format.into();`. The `default_value` is now `"ctfs"` for all three `--format` flags. The `OutputFormat::as_str` helper is added (marked `#[allow(dead_code)]`) for future `trace_metadata.json` `format` field emission, mirroring Fuel 1.53 / PolkaVM 1.55 / Miden 1.56. Same default-format fix as EVM (1.39), Solana (1.44), Move (1.46), Cardano (1.48), Cairo (1.50), Flow (1.52), Fuel (1.53), PolkaVM (1.55), Miden (1.56). |
| b | `register_call` for each call | OK | OK | The recorder emits `register_call(fn_id, args)` for every parsed Tolk function call (any expression matching `<name>()` recurses into `evaluate_function` which pushes / pops the call boundary). The matching exit emits `register_return`. The entry point (`main`) is intentionally merged into `<toplevel>` so its body lives at depth 0 — this design pre-dates the audit and is documented in `tracer.rs`'s `evaluate_program`. `sandbox.rs` and `replay.rs` each emit one synthetic top-level call (`<sandbox>` / `replay:<txhash>`) framing the parsed instruction stream / replay summary. |
| c | Call args via `register_call_arg` / `arg()` | **GAP** | **OK** (declared params) / **OPEN** (live values) | Pre-fix the call-detection branch in `tracer.rs` always called `register_call(fn_id, vec![])`, so the calltrace pane showed every Tolk function invocation with empty arguments even when the function had a formal parameter list. Post-fix the call branch iterates `func.params` (the (name, type) pairs the parser already extracts from `fun foo(a: int, b: int): int`) and stages each through `TraceWriter::arg(param_name, NONE_VALUE)` immediately before `register_call`. This populates `CallRecord.args` with the declared parameter names so the `.call-arg` rows in the calltrace pane match the source. **Open**: actual run-time arg values are still `NONE_VALUE` because the current Tolk parser (`parse_function_call`) only recognises zero-arg call sites (`compute()`); no expression-arg syntax (`compute(10, x)`) is parsed yet. Once the parser is extended to thread arg expressions, the same `arg()` staging path will accept live `ValueRecord::Int { … }` values without further audit work. Tracked under "Open gaps" below as the parser-extension follow-up. Parallel to PolkaVM 1.55 ink!-metadata symbolic decoding and Miden 1.56 per-procedure ABI parsing. |
| d | Write/WriteOther/Error/EvmEvent for IO and structured events via `register_special_event` | **GAP (Error)** / **N/A (Write)** / **OPEN (EvmEvent)** | **OK (Error)** / **N/A (Write)** / **PARTIAL (EvmEvent)** | Pre-fix TVM execution failures (overflow, divide-by-zero, gas exhaustion, an unhandled `THROW`) propagated through `tvm.rs::run_tvm_program -> tvm_eval_expr` which mapped any error to `None` via `.ok()` — the failure was silently dropped, the partial trace did not surface the cause, and downstream eval continued as if the expression had simply been unparseable. Post-fix `tvm.rs` exposes a checked variant `tvm_eval_expr_checked -> Result<Option<i64>>` that distinguishes "not parseable / unbound variable" (`Ok(None)`) from "TVM execution failed" (`Err(message)`). `tracer.rs::eval_expr` now drives the checked variant and on `Err` routes the failure through `register_special_event(EventLogKind::Error, "tvm_exception", &message)` — the partial trace finalises cleanly and the structured event channel surfaces the error (mirrors Miden 1.56 `miden_vm_error`, Cairo 1.50 `CairoPanic`, and Fuel 1.53 `Panic`/`Revert` routing). `sandbox.rs` similarly checks `event.exit_code` per parsed `vm_logs_full` instruction and routes any non-zero exit code through `register_special_event(EventLogKind::Error, "tvm_exception", "TVM exception at instruction '…' (exit code N)")`. **N/A (Write)**: the recorder's TVM evaluator (`tycho-vm`) does not run a host-function bridge, so Tolk programs have no native stdout/stderr channel comparable to PolkaVM's `seal_debug_message` or EVM's `console_log`. **Partial (EvmEvent)**: sandbox action-list trailer lines are now parsed and routed through `register_special_event(EventLogKind::EvmEvent, "tvm_out_message", payload)` for `SENDRAWMSG` / `SENDMSG`, or `"tvm_action"` for other action names. `tests/test_ctfs_audit.rs::ctfs_reader_sees_sandbox_out_message_event` opens the produced `.ct` through `NimTraceReaderHandle` and asserts the out-message payload is readable in the EvmEvent/stderr bucket. The source-level `record` path still does not surface TVM action-list entries because it only evaluates arithmetic expressions and does not execute contract message opcodes or inspect `VmState::committed_state.c5`. |
| e | Thread events (Start / Exit / Switch) | OK (N/A) | OK (N/A) | TVM is single-threaded by design — one continuation chain at a time, no parallelism primitive. Recorder correctly emits no thread events. |
| f | Step records for line navigation | OK | OK | `tracer.rs::evaluate_function` calls `register_step(source_path, Line(line))` at every parsed `var`/`val` binding and `return` statement. `sandbox.rs::trace_sandbox` calls `register_step` per parsed instruction (using the 1-based step number as the line). `replay.rs::replay_transaction` emits a single `register_step` for the synthetic balance dump (the replay path is a placeholder pending Liteserver integration). |
| g | Canonical CTFS schema match | **GAP** | **OK** | Pre-fix the writer always produced a `.ct` file regardless of `--format` (the underlying Nim writer treats `Binary` and `Ctfs` identically at the time of writing — see `codetracer_trace_writer_nim/src/lib.rs::TraceEventsFileFormat::to_ffi`), but the CLI surface advertised only `binary`/`json` so consumers had no way to *request* the canonical container deliberately. Post-fix verified by `tests/test_ctfs_audit.rs::ctfs_writer_produces_ct_container`: invoking `record(flow_test.tolk, out_dir, TraceEventsFileFormat::Ctfs)` produces a single `.ct` file starting with the canonical magic bytes `0xC0 0xDE 0x72 0xAC 0xE2` and materially populated (>64 bytes). The existing `tests/test_tracer.rs` already asserted CTFS magic but was passing `TraceEventsFileFormat::Json` — that quirk worked because the underlying writer always emits the multi-stream container; the audit fixes the test to pass `TraceEventsFileFormat::Ctfs` explicitly so an eventual divergence between Json and Ctfs in the writer cannot silently break the recorder. |
| h | Obsolete `add_event` calls | OK | OK | `grep -r 'add_event' src/` returns nothing. Recorder predates the 1.30 footgun and has always used dedicated `register_*` entry points. |
| i | `#[no_mangle]` stubs colliding with upstream Nim exports | OK | OK | `grep -r '#\[no_mangle\]' src/` returns nothing. Recorder uses the `codetracer_trace_writer_nim` Rust API directly (sibling-path dep), not the C FFI. |

## Concrete fixes applied

### 1. CLI now exposes and defaults to `Ctfs`

`src/main.rs`'s `OutputFormat` enum used to expose only `Binary` and
`Json`, with `Binary` as the default for `record`, `replay`, and
`trace-sandbox`. There was no way to request the canonical CTFS
multi-stream container from the CLI.

Post-fix: `OutputFormat` gains a `Ctfs` variant (listed first), with
doc-comments explaining each option, and a freshly added
`impl From<OutputFormat> for TraceEventsFileFormat` makes each
dispatch site uniform:

```rust
#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    /// Canonical CodeTracer multi-stream container (recommended; default).
    Ctfs,
    /// Legacy CBOR + Zstd binary format.
    Binary,
    /// Human-readable JSON (slower; useful for debugging).
    Json,
}

impl From<OutputFormat> for TraceEventsFileFormat {
    fn from(fmt: OutputFormat) -> Self {
        match fmt {
            OutputFormat::Ctfs => TraceEventsFileFormat::Ctfs,
            OutputFormat::Binary => TraceEventsFileFormat::Binary,
            OutputFormat::Json => TraceEventsFileFormat::Json,
        }
    }
}
```

`RecordArgs.format`, `ReplayArgs.format`, and `TraceSandboxArgs.format`
all default to `"ctfs"`. Each of the three dispatch sites (`record`,
`replay`, `trace_sandbox`) now reduces to
`let format: TraceEventsFileFormat = args.format.into();`. The
`OutputFormat::as_str` helper is wired in (marked `#[allow(dead_code)]`)
for future `trace_metadata.json` `format` field emission, mirroring the
Fuel 1.53 / PolkaVM 1.55 / Miden 1.56 pattern.

### 2. Tolk function call branch now stages declared formal parameters

The call-detection branch in `tracer.rs::evaluate_function` previously
called `register_call(fn_id, vec![])`, so the calltrace pane showed
every Tolk function invocation with empty arguments. Tolk functions
have a formal parameter list at the AST level (`func.params: Vec<(name,
type)>`), but the call site in the parser only recognises zero-arg
calls (`compute()`), so the staging path needs to surface declared
parameter *names* even when no run-time values reach it.

Post-fix the branch iterates `func.params` and stages each name through
`TraceWriter::arg(param_name, NONE_VALUE)` before `register_call`:

```rust
for (param_name, _param_type) in &func.params {
    let _ = TraceWriter::arg(&mut *self.writer, param_name, NONE_VALUE);
}
TraceWriter::register_call(&mut *self.writer, fn_id, vec![]);
```

The argument values are `NONE_VALUE` for now: the current Tolk parser
does not surface arg expressions to the call site, so the caller has
no live values to stage. Extending `parse_function_call` /
`evaluate_function` to thread arg expressions would let the staging
path emit live `ValueRecord::Int { … }` values without further audit
work — tracked as an open follow-up below (parallel to PolkaVM 1.55
ink!-metadata symbolic decoding and Miden 1.56 per-procedure ABI /
argument-name parsing).

### 3. TVM execution errors route through the structured event channel

Pre-fix `tvm.rs::tvm_eval_expr` returned `Option<i64>`, mapping any
TVM execution failure (`build_cell()` cell-builder overflow,
`run_tvm_program(..)?` non-zero exit code from `vm.run()`, gas
exhaustion, `THROW` with non-zero value) to `None` via `.ok()`. The
recorder dropped the failure entirely — the partial trace did not
surface it, the structured event channel never received it, and
downstream eval continued as if the expression had simply been
unparseable.

Post-fix `tvm.rs` adds a checked variant
`tvm_eval_expr_checked -> Result<Option<i64>>` that distinguishes
"expression unparseable / variable unbound" (`Ok(None)`, structural)
from "TVM execution failed" (`Err(message)`). `tvm_eval_expr` becomes
a `.ok().flatten()` thin wrapper preserving the existing tests'
contract.

`tracer.rs::eval_expr` now drives the checked variant and on `Err`
routes the failure through the structured event channel:

```rust
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
        Ok(None) // continue with partial trace
    }
}
```

`sandbox.rs::trace_sandbox` similarly inspects each parsed
`event.exit_code`: any non-zero exit code is routed through
`register_special_event(EventLogKind::Error, "tvm_exception", msg)`.
TVM uses non-zero exit codes for its `THROW` family of opcodes (TVM
Spec §4.5 "Exception primitives" —
https://docs.ton.org/tvm.pdf). Pre-fix the recorder dropped that
signal entirely; post-fix the frontend's error stream surfaces it.

This matches the Miden 1.56 `miden_vm_error` routing, the PolkaVM 1.55
trap / segfault / out-of-gas routing, the Cairo 1.50 `CairoPanic`
routing, and the Fuel 1.53 `Panic` / `Revert` routing.

## Tests added

`tests/test_ctfs_audit.rs` (4 new cases):

* `ctfs_writer_produces_ct_container` — runs `flow_test.tolk` through
  `recorder::record` with `TraceEventsFileFormat::Ctfs` and asserts
  the resulting `.ct` file starts with the canonical CTFS magic bytes
  (`0xC0 0xDE 0x72 0xAC 0xE2`) and is materially populated
  (>64 bytes).
* `ctfs_format_advertised_in_record_help` — CLI smoke test that
  `record --help` advertises `ctfs` as a `--format` value with
  `[default: ctfs]`. Uses `CARGO_BIN_EXE_codetracer-ton-recorder` to
  locate the just-built binary (same idiom as Flow 1.52, Fuel 1.53,
  PolkaVM 1.55, Miden 1.56). Catches accidental defaults regressions.
* `ctfs_format_advertised_in_replay_and_sandbox_help` — same
  `[default: ctfs]` assertion against `replay --help` and
  `trace-sandbox --help`, since all three subcommands had the same
  pre-fix default-format gap.
* `call_arg_staging_does_not_empty_trace` — structural smoke test for
  the `TraceWriter::arg(param_name, NONE_VALUE)` staging path
  introduced in this audit. The bundled `flow_test.tolk` fixture
  declares no formal parameters on its `compute` / `main` functions,
  so this is primarily a regression guard asserting that the staging
  branch still produces a valid CTFS container even when the loop
  body iterates zero times. When the Tolk parser is extended to
  accept arg-passing call sites (open follow-up below), this test
  should be upgraded to also assert that staged arg values appear on
  the `CallRecord.args` slice once the read-side helper lands.

`tests/test_tracer.rs` (touched):

* `run_tracer_on_file` now passes `TraceEventsFileFormat::Ctfs`
  explicitly (was `Json`). Pre-fix the test asserted CTFS magic on a
  file produced under `Json` mode — only working because the
  underlying Nim writer happens to treat `Json`'s writer-side dispatch
  identically to `Ctfs` for the multi-stream container path. Once the
  writer differentiates, the explicit `Ctfs` is the documented and
  correct intent.
* `test_ton_cli_record` now passes `--format ctfs` (was `--format
  json`). Same reasoning.

Read-side end-to-end content assertions on the embedded event records
(e.g. that `register_special_event(EventLogKind::Error,
"tvm_exception", …)` actually appears in the event-log of the `.ct`
container when a TVM error occurs) need the
`codetracer_trace_reader_nim` dev-dep added and a small reader-walk
helper. Tracked as an open follow-up below (also open for Cairo,
Cardano, Flow, Fuel, PolkaVM, and Miden).

## Verification

```
cd /home/zahary/metacraft/codetracer-ton-recorder
AH_TEST_RESOURCE_GUARD=1 cargo test --release
```

* lib unit tests: 67 / 67 passing
* `test_tracer` (existing): 7 / 7 passing
* `test_ctfs_audit` (new): 4 / 4 passing

Total: 78 / 78 passing across all suites, 0 regressions.
`cargo build --release` clean.

### Targeted Playwright sweep

`src/tests/gui/tests/program_specific_tests/tolk_example.spec.ts`
exists in the codetracer repo and contains 10 tests. 8 are gated on a
Tolk pipeline being integrated into `ct record` (the `tolkPipelineAvailable`
guard) and remain skipped both pre-fix and post-fix. The 2 unconditional
structural tests (language detection, tool availability detection)
pass identically pre-fix and post-fix:

```
Running 10 tests using 1 worker
  ✓   9 tolk_example — environment detection › tolk extension is classified as DB-based
  ✓  10 tolk_example — environment detection › tool availability detection does not throw
  8 skipped, 2 passed
```

The skipped tests cannot exercise the audit's recorder-side fixes
until the Tolk pipeline is wired into `ct record`. Cross-cutting
gating on consumer integration is the same shape as PolkaVM 1.55
(`polkatool` toolchain gating).

## Open gaps (not blocking, documented for follow-up)

### TVM action-list / out-message routing (audit d, EvmEvent)

TON contracts emit structured outputs through TVM's *action list*
(`SENDRAWMSG`, `RAWRESERVE`, `SETCODE`, `CHANGELIB`, `SENDMSG`) —
the closest analogue to EVM's `LOG` opcodes, Cairo's
`StarknetEvent`, Fuel's `Receipt::LogData` / `MessageOut`, and
PolkaVM's `seal_deposit_event`. The `trace-sandbox` path now parses
plain action-list trailer lines such as
`action: SENDRAWMSG mode=... dst=... value=... body=...` and routes
them as `EventLogKind::EvmEvent` special events with metadata
`"tvm_out_message"` (or `"tvm_action"` for non-message actions).
The CTFS reader projects `EvmEvent` into the existing `stderr` bucket,
matching the EVM / Circom audit convention.

Remaining open source-level gap: the `record` path does not currently
surface action-list entries because it compiles only simple arithmetic
expressions to TVM bytecode and does not execute a contract receive
path that can produce `SENDRAWMSG` / `SENDMSG` / `SETCODE` actions.
When that execution layer exists, inspect `VmState::committed_state.c5`
after `VmState::run()` and decode it with `tycho_types::models::
OutActionsRevIter`.

Concrete fix shape (mirrors EVM 1.39 `LOG` routing, Cairo 1.50
`StarknetEvent` routing, Fuel 1.53 `Receipt` routing, and PolkaVM 1.55
`seal_deposit_event` routing):

* In `tracer.rs`, after the contract-level `vm.run()` call, inspect
  `vm.committed_state.c5` and emit one `register_special_event` per
  decoded action:
  - `SendMsg` / `SendRawMsg` → `EventLogKind::EvmEvent` with metadata
    `"tvm_out_message"` and content describing destination + value + body.
  - `SetCode` → `EventLogKind::TraceLogEvent` with metadata
    `"tvm_action"`.
  - `RawReserve` → `EventLogKind::TraceLogEvent` with metadata
    `"tvm_action"`.
* Once `client_replay.rs` / Liteserver replay lands, the same
  routing applies to replay traces.

The writer API (`register_special_event`) already supports all three
kinds; the remaining gap is contract-level recorder wiring plus
`committed_state.c5` decoding.

### Tolk parser extension for arg-passing call sites (audit c)

The post-fix call-arg staging emits declared parameter *names* via
`writer.arg(param_name, NONE_VALUE)`. Run-time *values* are still
`NONE_VALUE` because the current Tolk parser only recognises
`<name>()` zero-arg call sites — there is no parsing for `compute(10,
x + 1)` or similar.

Concrete fix shape:

* Extend `parse_function_call` to parse a comma-separated arg list
  inside the parentheses.
* Thread the parsed arg expressions from the call site through
  `evaluate_function`'s parameter list, evaluating each via the
  existing `eval_expr` path.
* Replace the `NONE_VALUE` in the audit's `arg()` staging loop with
  the matched evaluated value.

Once that lands, the same `arg()` staging path emits live values
without further audit work. Parallel to PolkaVM 1.55 ink!-metadata
symbolic decoding and Miden 1.56 per-procedure ABI parsing.

### Replay-path tracing (audit f)

`replay.rs::replay_transaction` is currently a placeholder:
`LiteserverClient::fetch_contract_state` and `fetch_transaction` both
return "Liteserver communication not yet implemented". The audit fix
shape (default `Ctfs`, `From<OutputFormat>`, etc.) is already in
place; once Liteserver / `tonlib` / `adnl` integration lands, the
canonical recorder writes the `.ct` container directly. Same shape
of gap as Cairo 1.50 (replay-side tracing), Fuel 1.53 (node-replay),
PolkaVM 1.55 (Substrate-RPC), and Miden 1.56 (transaction replay).

### Tolk source parser robustness

The hand-rolled line-based parser in `tracer.rs::parse_functions`
handles only a subset of Tolk syntax:

* No nested expressions on multiple lines.
* No `if` / `while` / `repeat` control flow.
* No struct / tuple destructuring.
* No `asm` (raw-TVM) function bodies.

These limit what `record` can trace symbolically. Closing them
requires either an upstream Tolk parser (`tolk-cli` exposes a JSON
AST mode that could be consumed instead) or a tree-sitter integration
similar to Cairo's `tree-sitter-cairo`. Out of scope for the CTFS
audit; flagged because it bounds how much of audit (b) / (c) /
(f) can ever populate live data.

### Multi-stream IO event collapse (cross-cutting)

Same writer-side issue documented in 1.39 (EVM), 1.41 (PHP), 1.44
(Solana), 1.46 (Move), 1.48 (Cardano), 1.50 (Cairo), 1.52 (Flow),
1.53 (Fuel), 1.55 (PolkaVM), and 1.56 (Miden): the multi-stream IO
event writer's `toIOEventKind` collapses 13 `EventLogKind`s onto 4
`IOEventKind` buckets, losing the original kind byte and the
metadata string. The new `EventLogKind::Error` records
(`"tvm_exception"`) collapse onto `stderr` and lose the
`tvm_exception` metadata in the multi-stream pane. Out of scope for
any single recorder audit; flagged as a writer-side fix in
`codetracer_trace_writer_ffi.nim`'s `toIOEventKind`.

### Read-side end-to-end content assertions

The audit tests assert the `.ct` file starts with the CTFS magic and
is materially populated. Verifying that the embedded event stream
contains the expected `register_call` / `register_special_event`
records (e.g. `EventLogKind::Error` with `"tvm_exception"` metadata
when a `THROW` occurs) requires the `codetracer_trace_reader_nim`
dep added as a `[dev-dependencies]` entry plus a small reader-walk
helper. Tracked here for the next pass (also open for Cairo,
Cardano, Flow, Fuel, PolkaVM, and Miden).

## After this audit

Section 5.6's recorder list shows `codetracer-ton-recorder` as
audited (gaps closed for default-Ctfs CLI + declared formal-parameter
arg staging via `TraceWriter::arg(param_name, NONE_VALUE)` + TVM
exception routing through `register_special_event(EventLogKind::Error,
"tvm_exception", …)` for both the `record` (Tolk parser-driven)
and `trace-sandbox` (vm_logs_full exit-code-driven) paths; sandbox
action-list / out-message trailer routing through
`register_special_event(EventLogKind::EvmEvent, "tvm_out_message",
...)` is partial-closed with a CTFS reader assertion; source-level
contract action-list decoding + Tolk parser extension for arg-passing
call sites + Liteserver replay-path tracing open as recorder-side /
parser / RPC-integration follow-ups). Audited recorder count: 13 → 14.
