//! On-chain transaction replay infrastructure.
//!
//! This module provides the types and functions needed to fetch TON
//! transactions from a Liteserver, reconstruct the contract state at
//! transaction time, and replay TVM execution while capturing a
//! CodeTracer trace.
//!
//! # M6 milestone -- On-Chain Transaction Replay
//!
//! The current implementation provides the structural foundation:
//! type definitions, configuration, BOC parsing stubs, and the
//! replay orchestration function.  Actual Liteserver communication
//! is stubbed out with placeholder implementations that will be
//! filled in once the `tonlib` or `adnl` client crate is integrated.

use std::path::{Path, PathBuf};

use codetracer_trace_types::{Line, TypeKind, ValueRecord};
use codetracer_trace_writer::trace_writer::TraceWriter;
use codetracer_trace_writer::{create_trace_writer, TraceEventsFileFormat};
use eyre::{eyre, Context, Result};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A lightweight client for communicating with a TON Liteserver.
///
/// In the current milestone this is a placeholder -- the `endpoint` is
/// stored but no network calls are made yet.
#[derive(Debug, Clone)]
pub struct LiteserverClient {
    /// Liteserver endpoint URL or address (e.g. "https://ton.org/global-config.json").
    pub endpoint: String,
}

impl LiteserverClient {
    /// Create a new client targeting the given endpoint.
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
        }
    }

    /// Fetch the contract state at the time of a transaction.
    ///
    /// **Placeholder** -- returns an error indicating that Liteserver
    /// communication is not yet implemented.
    pub fn fetch_contract_state(&self, _address: &str) -> Result<ContractState> {
        Err(eyre!(
            "Liteserver communication not yet implemented (endpoint: {})",
            self.endpoint
        ))
    }

    /// Fetch transaction details by hash.
    ///
    /// **Placeholder** -- returns an error indicating that Liteserver
    /// communication is not yet implemented.
    pub fn fetch_transaction(&self, _tx_hash: &str, _address: &str) -> Result<TransactionInfo> {
        Err(eyre!(
            "Liteserver communication not yet implemented (endpoint: {})",
            self.endpoint
        ))
    }
}

/// Configuration for replaying an on-chain transaction.
#[derive(Debug, Clone)]
pub struct ReplayConfig {
    /// Transaction hash to replay (hex-encoded).
    pub tx_hash: String,
    /// Contract address that executed the transaction.
    pub address: String,
    /// Liteserver endpoint for fetching chain data.
    pub endpoint: String,
    /// Optional path to the contract source directory (for source mapping).
    pub source_dir: Option<PathBuf>,
}

/// The on-chain state of a contract at a particular point in time.
#[derive(Debug, Clone)]
pub struct ContractState {
    /// BOC-encoded contract code cell.
    pub code: Vec<u8>,
    /// BOC-encoded contract data/storage cell.
    pub data: Vec<u8>,
    /// Account balance in nanoTON.
    pub balance: u64,
}

/// Information about an on-chain transaction needed for replay.
#[derive(Debug, Clone)]
pub struct TransactionInfo {
    /// Body of the incoming message (BOC-encoded).
    pub in_msg_body: Vec<u8>,
    /// Contract state at the time of the transaction.
    pub contract_state: ContractState,
}

// ---------------------------------------------------------------------------
// BOC parsing
// ---------------------------------------------------------------------------

/// Parse a BOC (Bag of Cells) envelope and return the raw cell bytes.
///
/// BOC is the standard serialisation format for TVM cells on the TON
/// blockchain.  This function validates the BOC magic prefix and
/// extracts the payload.
///
/// **Stub** -- the current implementation performs only a minimal magic
/// byte check and returns the payload bytes unchanged.  A full
/// implementation will deserialise the cell tree.
pub fn parse_boc(bytes: &[u8]) -> Result<Vec<u8>> {
    // BOC magic: b5ee9c72 (4 bytes).
    const BOC_MAGIC: [u8; 4] = [0xb5, 0xee, 0x9c, 0x72];

    if bytes.len() < 4 {
        return Err(eyre!("BOC data too short ({} bytes)", bytes.len()));
    }

    if bytes[..4] != BOC_MAGIC {
        return Err(eyre!(
            "invalid BOC magic: expected b5ee9c72, got {:02x}{:02x}{:02x}{:02x}",
            bytes[0],
            bytes[1],
            bytes[2],
            bytes[3]
        ));
    }

    // For now, return the payload after the magic prefix.
    Ok(bytes[4..].to_vec())
}

// ---------------------------------------------------------------------------
// Replay orchestration
// ---------------------------------------------------------------------------

/// Replay a TON transaction and produce a CodeTracer trace.
///
/// High-level flow:
/// 1. Connect to the Liteserver and fetch the transaction + contract state.
/// 2. Deserialise the contract code and data cells from BOC.
/// 3. Set up a TVM instance with the contract's code, data, and the
///    incoming message.
/// 4. Execute the TVM while emitting CodeTracer trace events.
/// 5. Write trace output files to `out_dir`.
///
/// In the current milestone, step 1 uses mock/placeholder data when
/// real Liteserver access is unavailable.
pub fn replay_transaction(
    config: &ReplayConfig,
    out_dir: &Path,
    format: TraceEventsFileFormat,
) -> Result<()> {
    eprintln!(
        "Replaying transaction {} on contract {}",
        config.tx_hash, config.address
    );

    // -- 1. Fetch transaction data from Liteserver --
    let client = LiteserverClient::new(&config.endpoint);
    let tx_info = client.fetch_transaction(&config.tx_hash, &config.address);

    let tx_info = match tx_info {
        Ok(info) => info,
        Err(e) => {
            eprintln!(
                "Warning: could not fetch transaction from Liteserver: {e}. \
                 Liteserver integration is not yet implemented."
            );
            return Err(e).with_context(|| {
                format!(
                    "failed to replay transaction {} -- Liteserver fetch not yet implemented",
                    config.tx_hash
                )
            });
        }
    };

    // -- 2. Set up trace writer --
    let source_label = config
        .source_dir
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| config.address.clone());

    let source_path = config
        .source_dir
        .as_deref()
        .unwrap_or_else(|| Path::new("contract.tolk"));

    let mut writer = create_trace_writer(&source_label, &[], format);

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    let events_filename = match format {
        TraceEventsFileFormat::Json => "trace.json",
        TraceEventsFileFormat::Binary | TraceEventsFileFormat::BinaryV0 => "trace.bin",
    };
    let events_path = out_dir.join(events_filename);
    let metadata_path = out_dir.join("trace_metadata.json");
    let paths_path = out_dir.join("trace_paths.json");

    TraceWriter::begin_writing_trace_events(&mut *writer, &events_path)
        .map_err(|e| eyre!("{e}"))?;
    TraceWriter::begin_writing_trace_metadata(&mut *writer, &metadata_path)
        .map_err(|e| eyre!("{e}"))?;
    TraceWriter::begin_writing_trace_paths(&mut *writer, &paths_path).map_err(|e| eyre!("{e}"))?;

    TraceWriter::start(&mut *writer, source_path, Line(1));

    // Register types.
    let int_type_id = TraceWriter::ensure_type_id(&mut *writer, TypeKind::Int, "int");

    // -- 3. Create a trace function for the replayed contract --
    let fn_id = TraceWriter::ensure_function_id(
        &mut *writer,
        &format!("replay:{}", config.tx_hash),
        source_path,
        Line(1),
    );
    TraceWriter::register_call(&mut *writer, fn_id, vec![]);

    // Emit a step recording the contract balance as a variable.
    TraceWriter::register_step(&mut *writer, source_path, Line(1));

    let balance_value = ValueRecord::Int {
        i: tx_info.contract_state.balance as i64,
        type_id: int_type_id,
    };
    TraceWriter::register_variable_with_full_value(&mut *writer, "balance", balance_value);

    // -- 4. Finish writing --
    TraceWriter::finish_writing_trace_events(&mut *writer).map_err(|e| eyre!("{e}"))?;
    TraceWriter::finish_writing_trace_metadata(&mut *writer).map_err(|e| eyre!("{e}"))?;
    TraceWriter::finish_writing_trace_paths(&mut *writer).map_err(|e| eyre!("{e}"))?;

    eprintln!("Replay trace written to {}", out_dir.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- ReplayConfig tests --------------------------------------------------

    #[test]
    fn test_replay_config_construction() {
        let config = ReplayConfig {
            tx_hash: "abc123".to_string(),
            address: "EQD...".to_string(),
            endpoint: "https://ton.org/global-config.json".to_string(),
            source_dir: None,
        };
        assert_eq!(config.tx_hash, "abc123");
        assert_eq!(config.address, "EQD...");
        assert_eq!(config.endpoint, "https://ton.org/global-config.json");
        assert!(config.source_dir.is_none());
    }

    #[test]
    fn test_replay_config_with_source_dir() {
        let config = ReplayConfig {
            tx_hash: "def456".to_string(),
            address: "EQA...".to_string(),
            endpoint: "http://localhost:4443".to_string(),
            source_dir: Some(PathBuf::from("/tmp/contract-src")),
        };
        assert_eq!(config.source_dir, Some(PathBuf::from("/tmp/contract-src")));
    }

    // -- ContractState tests -------------------------------------------------

    #[test]
    fn test_contract_state_creation() {
        let state = ContractState {
            code: vec![0xb5, 0xee, 0x9c, 0x72, 0x01],
            data: vec![0xb5, 0xee, 0x9c, 0x72, 0x02],
            balance: 1_000_000_000, // 1 TON
        };
        assert_eq!(state.code.len(), 5);
        assert_eq!(state.data.len(), 5);
        assert_eq!(state.balance, 1_000_000_000);
    }

    #[test]
    fn test_contract_state_empty() {
        let state = ContractState {
            code: vec![],
            data: vec![],
            balance: 0,
        };
        assert!(state.code.is_empty());
        assert!(state.data.is_empty());
        assert_eq!(state.balance, 0);
    }

    // -- TransactionInfo tests -----------------------------------------------

    #[test]
    fn test_transaction_info_creation() {
        let tx = TransactionInfo {
            in_msg_body: vec![0x00, 0x01, 0x02],
            contract_state: ContractState {
                code: vec![0xff],
                data: vec![0xaa],
                balance: 500_000_000,
            },
        };
        assert_eq!(tx.in_msg_body, vec![0x00, 0x01, 0x02]);
        assert_eq!(tx.contract_state.balance, 500_000_000);
    }

    // -- LiteserverClient tests ----------------------------------------------

    #[test]
    fn test_liteserver_client_creation() {
        let client = LiteserverClient::new("https://example.com/config.json");
        assert_eq!(client.endpoint, "https://example.com/config.json");
    }

    #[test]
    fn test_liteserver_fetch_returns_not_implemented() {
        let client = LiteserverClient::new("https://example.com");
        let result = client.fetch_contract_state("EQD...");
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("not yet implemented"));
    }

    #[test]
    fn test_liteserver_fetch_transaction_returns_not_implemented() {
        let client = LiteserverClient::new("https://example.com");
        let result = client.fetch_transaction("abc123", "EQD...");
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("not yet implemented"));
    }

    // -- BOC parsing tests ---------------------------------------------------

    #[test]
    fn test_parse_boc_valid() {
        let boc = vec![0xb5, 0xee, 0x9c, 0x72, 0x01, 0x02, 0x03];
        let payload = parse_boc(&boc).unwrap();
        assert_eq!(payload, vec![0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_parse_boc_invalid_magic() {
        let boc = vec![0x00, 0x00, 0x00, 0x00, 0x01];
        let result = parse_boc(&boc);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("invalid BOC magic"));
    }

    #[test]
    fn test_parse_boc_too_short() {
        let boc = vec![0xb5, 0xee];
        let result = parse_boc(&boc);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("too short"));
    }

    #[test]
    fn test_parse_boc_empty_payload() {
        let boc = vec![0xb5, 0xee, 0x9c, 0x72];
        let payload = parse_boc(&boc).unwrap();
        assert!(payload.is_empty());
    }

    // -- Replay transaction test (mock) --------------------------------------

    #[test]
    fn test_replay_transaction_fails_without_liteserver() {
        let config = ReplayConfig {
            tx_hash: "deadbeef".to_string(),
            address: "EQDtest".to_string(),
            endpoint: "https://not-a-real-endpoint.example.com".to_string(),
            source_dir: None,
        };
        let tmp = tempfile::tempdir().unwrap();
        let result = replay_transaction(&config, tmp.path(), TraceEventsFileFormat::Json);
        // Should fail because Liteserver is not implemented yet.
        assert!(result.is_err());
    }
}
