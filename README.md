## codetracer-ton-recorder

A recorder of TON/Tolk smart contract executions that produces [CodeTracer](https://github.com/metacraft-labs/CodeTracer) traces.

> [!WARNING]
> Currently it is in a very early phase: we're welcoming contribution and discussion!

### Overview

codetracer-ton-recorder executes Tolk programs on TVM, captures step-level traces with symbolic stack tracking, and emits structured trace files compatible with CodeTracer. It can also parse `vm_logs_full` output from `@ton/sandbox` and replay on-chain transactions.

### Building

```bash
cargo build
```

### Usage

Record a trace from a Tolk source file:

```bash
codetracer-ton-recorder record <tolk-file> --out-dir <dir> [--format binary|json]
# Produces trace files in <dir>.
# --format selects the output format (defaults to binary).
```

Parse and trace from `@ton/sandbox` vm_logs_full output:

```bash
codetracer-ton-recorder trace-sandbox <vm-log-file> --out-dir <dir> [--format binary|json]
```

Replay an on-chain transaction:

```bash
codetracer-ton-recorder replay <tx-hash> --out-dir <dir> [--format binary|json]
```

However, you probably want to use it in combination with CodeTracer, which would be released soon.

### Architecture

The recorder is organized into the following modules:

* `recorder.rs` — top-level recording orchestration and trace file output
* `tracer.rs` — step-level TVM execution tracing
* `tvm.rs` — TVM execution engine integration
* `source_map.rs` — mapping from TVM instructions back to Tolk source locations
* `sandbox.rs` — parser for `@ton/sandbox` vm_logs_full output
* `stack_tracker.rs` — symbolic TVM stack tracking to recover variable names
* `replay.rs` — on-chain transaction replay

### Testing

Test programs live in `test-programs/tolk/`. Run the test suite with:

```bash
cargo test
```

### Environment variables

* `RUST_LOG` — controls log verbosity (standard `env_logger` syntax, e.g. `RUST_LOG=debug`)

### Contributing

We'd be very happy if the community finds this useful, and if anyone wants to:

* Use and test the TON/Tolk support or CodeTracer.
* Provide feedback and discuss alternative implementation ideas: in the issue tracker, or in our [discord](https://discord.gg/qSDCAFMP).
* Contribute code to enhance the TON/Tolk support of CodeTracer.
* Provide [sponsorship](https://opencollective.com/codetracer), so we can hire dedicated full-time maintainers for this project.

### Legal info

LICENSE: MIT

Copyright (c) 2025 Metacraft Labs Ltd
