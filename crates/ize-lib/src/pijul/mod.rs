//! Pijul backend for Ize
//!
//! This module provides a clean interface to libpijul, handling the
//! differences between standard pijul directory structure and Ize's
//! custom layout.
//!
//! Note: This is the first backend implementation. The architecture is
//! designed to support pluggable backends in the future via a VcsBackend trait.
//!
//! ## Modules
//!
//! - [`backend`]: Core PijulBackend implementation
//! - [`operations`]: Opcode recording - converts filesystem operations into Pijul changes

pub mod backend;
pub mod operations;

// Re-export key types from backend module
pub use backend::{
    PijulBackend, PijulError, CHANGES_DIR, CONFIG_FILE, DB_FILE, DEFAULT_PRISTINE_SIZE,
    PRISTINE_DIR,
};

// Re-export key types from operations module
pub use operations::{OpcodeError, OpcodeRecordingBackend};
