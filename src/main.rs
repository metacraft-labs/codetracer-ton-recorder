//! CLI entry point for the CodeTracer Tolk/TON recorder.
//!
//! Supports the `record`, `trace-sandbox`, and `replay` subcommands.
//! Each takes a Tolk source / sandbox log / on-chain transaction,
//! produces a CodeTracer trace, and writes it to `--out-dir`.
//!
//! # Usage
//!
//! ```text
//! codetracer-ton-recorder record <tolk-file> \
//!     --out-dir <output-dir> \
//!     [--format ctfs|binary|json]
//! ```
//!
//! The default output format is `ctfs` -- the canonical CodeTracer
//! multi-stream container that the Nim `ct_reader_*` FFI and the
//! db-backend's `CTFSTraceReader` consume directly.  `binary`
//! (legacy CBOR + Zstd) and `json` (human-readable) are kept for
//! compatibility / debugging.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use codetracer_trace_writer_nim::TraceEventsFileFormat;
use eyre::{Context, Result};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// CodeTracer Tolk/TON recorder -- record Tolk program execution traces.
#[derive(Debug, Parser)]
#[command(
    name = "codetracer-ton-recorder",
    version,
    about = "Record Tolk/TON program execution traces for CodeTracer"
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
    /// captures the execution trace, and writes CodeTracer trace files
    /// to `--out-dir`.
    Record(RecordArgs),

    /// Parse @ton/sandbox vm_logs_full output and produce a CodeTracer trace.
    TraceSandbox(TraceSandboxArgs),

    /// Replay an on-chain TON transaction and produce a CodeTracer trace.
    ///
    /// Fetches the transaction and contract state from a Liteserver,
    /// re-executes the TVM computation, and writes trace output files.
    Replay(ReplayArgs),

    /// Print version information.
    Version,
}

/// Output format for the trace files.
///
/// `Ctfs` is the canonical CodeTracer multi-stream container (the
/// format the Nim `ct_reader_*` FFI and the db-backend's
/// `CTFSTraceReader` consume directly) and is the default.  `Binary`
/// is the legacy CBOR + Zstd container kept for compatibility with
/// older readers.  `Json` is a slower, human-readable form useful
/// for debugging.
///
/// Same shape as the audited recorders (EVM 1.39, Solana 1.44, Move
/// 1.46, Cardano 1.48, Cairo 1.50, Flow 1.52, Fuel 1.53, PolkaVM
/// 1.55, Miden 1.56) so `From<OutputFormat>` collapses each
/// dispatch site to `args.format.into()`.
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

impl OutputFormat {
    /// Stable string representation suitable for `trace_metadata.json`'s
    /// `format` field.  Wired here for forward compatibility with the
    /// audit-aligned metadata emission path used by other recorders;
    /// not yet consumed by the writer plumbing in this crate.
    #[allow(dead_code)]
    fn as_str(self) -> &'static str {
        match self {
            OutputFormat::Ctfs => "ctfs",
            OutputFormat::Binary => "binary",
            OutputFormat::Json => "json",
        }
    }
}

#[derive(Debug, clap::Args)]
struct RecordArgs {
    /// Path to the Tolk source (.tolk) file.
    program: PathBuf,

    /// Directory where the trace files will be written.
    ///
    /// The directory will be created if it does not exist.
    #[arg(short = 'o', long, default_value = "./ct-traces/")]
    out_dir: PathBuf,

    /// Output format for the trace data.
    #[arg(short = 'f', long, default_value = "ctfs")]
    format: OutputFormat,
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
    #[arg(short = 'o', long, default_value = "./ct-traces/")]
    out_dir: PathBuf,

    /// Output format for the trace data.
    #[arg(short = 'f', long, default_value = "ctfs")]
    format: OutputFormat,
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
    #[arg(short = 'o', long, default_value = "./ct-traces/")]
    out_dir: PathBuf,

    /// Output format for the trace data.
    #[arg(short = 'f', long, default_value = "ctfs")]
    format: OutputFormat,
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

    let format: TraceEventsFileFormat = args.format.into();

    // 2. Create the output directory
    let out_dir = &args.out_dir;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    // 3. Run the recorder
    codetracer_ton_recorder::recorder::record(&source_path, out_dir, format)?;

    eprintln!("Trace files written to {}", out_dir.display());

    Ok(())
}

/// Execute the `replay` subcommand.
fn replay(args: ReplayArgs) -> Result<()> {
    let format: TraceEventsFileFormat = args.format.into();

    let config = codetracer_ton_recorder::replay::ReplayConfig {
        tx_hash: args.tx_hash,
        address: args.address,
        endpoint: args.endpoint,
        source_dir: args.source_dir,
    };

    codetracer_ton_recorder::replay::replay_transaction(&config, &args.out_dir, format)?;

    eprintln!("Replay trace files written to {}", args.out_dir.display());
    Ok(())
}

/// Execute the `trace-sandbox` subcommand.
fn trace_sandbox(args: TraceSandboxArgs) -> Result<()> {
    let vm_log_path = args
        .vm_log
        .canonicalize()
        .with_context(|| format!("vm_log file not found: {}", args.vm_log.display()))?;

    eprintln!("VM log file: {}", vm_log_path.display());

    let format: TraceEventsFileFormat = args.format.into();

    let source_path = &args.source;
    let out_dir = &args.out_dir;

    codetracer_ton_recorder::sandbox::trace_sandbox(&vm_log_path, source_path, out_dir, format)?;

    eprintln!("Trace files written to {}", out_dir.display());

    Ok(())
}
