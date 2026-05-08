//! CLI entry point for the CodeTracer Tolk/TON recorder.
//!
//! Supports the `record`, `trace-sandbox`, and `replay` subcommands.
//! Each takes a Tolk source / sandbox log / on-chain transaction,
//! produces a CodeTracer trace, and writes it to `--out-dir`.
//!
//! # Usage
//!
//! ```text
//! codetracer-ton-recorder record <tolk-file> --out-dir <output-dir>
//! codetracer-ton-recorder trace-sandbox --vm-log <log> --out-dir <output-dir>
//! codetracer-ton-recorder replay --tx-hash <hash> --address <addr> --out-dir <output-dir>
//! ```
//!
//! The recorder always writes traces in the canonical CodeTracer
//! multi-stream CTFS format (see `Recorder-CLI-Conventions.md` §4 in
//! `codetracer-specs`).  No `--format` flag is exposed: human-readable
//! conversion is handled out-of-band by `ct print` (shipped with
//! `codetracer-trace-format-nim`).
//!
//! # Environment variables
//!
//! * `CODETRACER_TON_RECORDER_OUT_DIR` — fallback for `--out-dir` when the
//!   flag is not given. The CLI flag always wins.
//! * `CODETRACER_TON_RECORDER_DISABLED` — set to `1` or `true` to skip
//!   recording entirely. The recorder still validates its inputs (where
//!   applicable) and propagates a clean exit code.
//! * `CODETRACER_TON_RECORDER_LOG_LEVEL` — recorder log verbosity
//!   (advisory; the TON recorder currently logs to stderr unconditionally).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use eyre::{Context, Result};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Environment variable used as a fallback for `--out-dir` when the CLI
/// flag is omitted.  Convention: see `Recorder-CLI-Conventions.md` §5.
const ENV_OUT_DIR: &str = "CODETRACER_TON_RECORDER_OUT_DIR";

/// Environment variable that, when set to `1`/`true`, disables tracing
/// entirely — the recorder runs as a transparent pass-through.
const ENV_DISABLED: &str = "CODETRACER_TON_RECORDER_DISABLED";

/// Default output directory used when neither `--out-dir` nor
/// `CODETRACER_TON_RECORDER_OUT_DIR` is set.
const DEFAULT_OUT_DIR: &str = "./ct-traces/";

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// CodeTracer Tolk/TON recorder — record Tolk program execution traces.
///
/// Traces are always written in the canonical CTFS multi-stream format.
/// To convert a recorded `.ct` bundle to JSON / text for inspection, use
/// `ct print` from `codetracer-trace-format-nim`.
#[derive(Debug, Parser)]
#[command(
    name = "codetracer-ton-recorder",
    version,
    about = "Record Tolk/TON program execution traces for CodeTracer (CTFS-only). \
             Use `ct print` from codetracer-trace-format-nim for human-readable conversion.",
    long_about = "Record Tolk/TON program execution traces for CodeTracer.\n\
                  \n\
                  Output is always written in the canonical CodeTracer CTFS\n\
                  multi-stream format. Use `ct print` (shipped with the\n\
                  codetracer-trace-format-nim sibling) to convert a recorded\n\
                  `.ct` bundle to JSON or other human-readable forms.\n\
                  \n\
                  Environment variables:\n\
                    CODETRACER_TON_RECORDER_OUT_DIR    fallback for --out-dir\n\
                    CODETRACER_TON_RECORDER_DISABLED   set to 1/true to skip recording\n\
                    CODETRACER_TON_RECORDER_LOG_LEVEL  log verbosity (advisory)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Record execution of a Tolk program.
    ///
    /// Parses the given .tolk source file, evaluates function bodies,
    /// captures the execution trace, and writes a CTFS trace bundle
    /// to `--out-dir`.
    Record(RecordArgs),

    /// Parse @ton/sandbox vm_logs_full output and produce a CodeTracer trace.
    TraceSandbox(TraceSandboxArgs),

    /// Replay an on-chain TON transaction and produce a CodeTracer trace.
    ///
    /// Fetches the transaction and contract state from a Liteserver,
    /// re-executes the TVM computation, and writes a CTFS trace bundle.
    Replay(ReplayArgs),

    /// Print version information.
    Version,
}

#[derive(Debug, clap::Args)]
struct RecordArgs {
    /// Path to the Tolk source (.tolk) file.
    program: PathBuf,

    /// Directory where the trace files will be written.
    ///
    /// The directory will be created if it does not exist.  Falls back to
    /// the `CODETRACER_TON_RECORDER_OUT_DIR` environment variable when
    /// the flag is omitted.
    #[arg(short = 'o', long)]
    out_dir: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
struct ReplayArgs {
    /// Transaction hash to replay (hex-encoded).
    #[arg(long)]
    tx_hash: String,

    /// Contract address that executed the transaction.
    #[arg(long)]
    address: String,

    /// Liteserver endpoint for fetching chain data.
    #[arg(long, default_value = "https://ton.org/global-config.json")]
    endpoint: String,

    /// Optional path to the contract source directory (for source mapping).
    #[arg(long)]
    source_dir: Option<PathBuf>,

    /// Directory where the trace files will be written.
    ///
    /// Falls back to the `CODETRACER_TON_RECORDER_OUT_DIR` environment
    /// variable when the flag is omitted.
    #[arg(short = 'o', long)]
    out_dir: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
struct TraceSandboxArgs {
    /// Path to the vm_logs_full output file from @ton/sandbox.
    #[arg(long)]
    vm_log: PathBuf,

    /// Path to the Tolk source file (for source mapping).
    #[arg(long, default_value = "contract.tolk")]
    source: PathBuf,

    /// Directory where the trace files will be written.
    ///
    /// Falls back to the `CODETRACER_TON_RECORDER_OUT_DIR` environment
    /// variable when the flag is omitted.
    #[arg(short = 'o', long)]
    out_dir: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the effective output directory:
///   1. `--out-dir` if given on the CLI.
///   2. `CODETRACER_TON_RECORDER_OUT_DIR` env var.
///   3. `DEFAULT_OUT_DIR` ("./ct-traces/").
fn resolve_out_dir(cli_out_dir: Option<PathBuf>) -> PathBuf {
    if let Some(path) = cli_out_dir {
        return path;
    }
    if let Some(value) = std::env::var_os(ENV_OUT_DIR) {
        if !value.is_empty() {
            return PathBuf::from(value);
        }
    }
    PathBuf::from(DEFAULT_OUT_DIR)
}

/// Whether the recorder is disabled via env var.  When true, the CLI
/// must execute its target operation in pass-through mode without
/// emitting any trace artefacts.
fn recording_disabled() -> bool {
    match std::env::var(ENV_DISABLED) {
        Ok(value) => {
            let v = value.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Record(args) => record(args),
        Commands::TraceSandbox(args) => trace_sandbox(args),
        Commands::Replay(args) => replay(args),
        Commands::Version => {
            println!("codetracer-ton-recorder {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// `record` implementation
// ---------------------------------------------------------------------------

/// Execute the `record` subcommand.
fn record(args: RecordArgs) -> Result<()> {
    // 1. Validate the source file exists
    let source_path = args
        .program
        .canonicalize()
        .with_context(|| format!("source file not found: {}", args.program.display()))?;

    eprintln!("Source file: {}", source_path.display());

    if recording_disabled() {
        // Pass-through: the TON recorder doesn't run a separate target
        // process — it parses & evaluates the Tolk source itself — so
        // disabling recording simply means "don't emit any trace artefacts".
        eprintln!("{ENV_DISABLED} is set; skipping trace recording (no output written).");
        return Ok(());
    }

    // 2. Resolve and create the output directory
    let out_dir = resolve_out_dir(args.out_dir);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    // 3. Run the recorder (CTFS only)
    codetracer_ton_recorder::recorder::record(&source_path, &out_dir)?;

    eprintln!("Trace files written to {}", out_dir.display());

    Ok(())
}

/// Execute the `replay` subcommand.
fn replay(args: ReplayArgs) -> Result<()> {
    let config = codetracer_ton_recorder::replay::ReplayConfig {
        tx_hash: args.tx_hash,
        address: args.address,
        endpoint: args.endpoint,
        source_dir: args.source_dir,
    };

    if recording_disabled() {
        eprintln!("{ENV_DISABLED} is set; skipping replay recording (no output written).");
        return Ok(());
    }

    let out_dir = resolve_out_dir(args.out_dir);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    codetracer_ton_recorder::replay::replay_transaction(&config, &out_dir)?;

    eprintln!("Replay trace files written to {}", out_dir.display());
    Ok(())
}

/// Execute the `trace-sandbox` subcommand.
fn trace_sandbox(args: TraceSandboxArgs) -> Result<()> {
    let vm_log_path = args
        .vm_log
        .canonicalize()
        .with_context(|| format!("vm_log file not found: {}", args.vm_log.display()))?;

    eprintln!("VM log file: {}", vm_log_path.display());

    if recording_disabled() {
        eprintln!("{ENV_DISABLED} is set; skipping trace recording (no output written).");
        return Ok(());
    }

    let source_path = &args.source;
    let out_dir = resolve_out_dir(args.out_dir);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    codetracer_ton_recorder::sandbox::trace_sandbox(&vm_log_path, source_path, &out_dir)?;

    eprintln!("Trace files written to {}", out_dir.display());

    Ok(())
}
