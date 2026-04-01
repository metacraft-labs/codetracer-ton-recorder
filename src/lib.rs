//! CodeTracer recorder for Tolk/TON smart contract programs.
//!
//! This crate captures execution traces from Tolk programs by parsing
//! function definitions, variable declarations, and return statements,
//! evaluating them in order, and converting the results into the
//! CodeTracer trace format for debugging and analysis.

pub mod recorder;
pub mod replay;
pub mod sandbox;
pub mod source_map;
pub mod stack_tracker;
pub mod tracer;
pub mod tvm;
