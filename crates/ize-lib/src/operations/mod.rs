//! Operations module for Ize
//!
//! This module contains the opcode system for capturing and processing
//! filesystem mutations.

pub mod opcode;

// Re-export key types for convenience
pub use opcode::{Opcode, Operation};
