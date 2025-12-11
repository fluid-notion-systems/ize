//! Operations module for Ize
//!
//! This module contains the opcode system for capturing and processing
//! filesystem mutations:
//!
//! - [`opcode`]: Core `Opcode` and `Operation` types
//! - [`queue`]: Thread-safe `OpcodeQueue` for buffering operations
//! - [`recorder`]: `OpcodeRecorder` that implements `FsObserver`

pub mod opcode;
pub mod queue;
pub mod recorder;

// Re-export key types for convenience
pub use opcode::{Opcode, Operation};
pub use queue::{OpcodeQueue, OpcodeSender};
pub use recorder::OpcodeRecorder;
