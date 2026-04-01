//! CLI entry point for the CodeTracer Tolk/TON recorder.
//!
//! Supports the `record` subcommand which takes a Tolk source file,
//! parses and evaluates function bodies, and writes CodeTracer trace
//! output files.
//!
//! # Usage
//!
//! ```text
//! codetracer-ton-recorder record <tolk-file> \
//!     --out-dir <output-dir> \
//!     [--format binary|json]
//! ```

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use codetracer_trace_writer::TraceEventsFileFormat;
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

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Binary,
    Json,
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
    #[arg(short = 'f', long, default_value = "binary")]
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
    #[arg(short = 'f', long, default_value = "binary")]
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
    #[arg(short = 'f', long, default_value = "binary")]
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
            println!(
                "codetracer-ton-recorder {}",
                env!("CARGO_PKG_VERSION")
            );
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

    let format = match args.format {
        OutputFormat::Binary => TraceEventsFileFormat::Binary,
        OutputFormat::Json => TraceEventsFileFormat::Json,
    };

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
    let format = match args.format {
        OutputFormat::Binary => TraceEventsFileFormat::Binary,
        OutputFormat::Json => TraceEventsFileFormat::Json,
    };

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

    let format = match args.format {
        OutputFormat::Binary => TraceEventsFileFormat::Binary,
        OutputFormat::Json => TraceEventsFileFormat::Json,
    };

    let source_path = &args.source;
    let out_dir = &args.out_dir;

    codetracer_ton_recorder::sandbox::trace_sandbox(&vm_log_path, source_path, out_dir, format)?;

    eprintln!("Trace files written to {}", out_dir.display());

    Ok(())
}
