//! Integration tests for Ize
//!
//! These tests verify the complete system behavior, including:
//! - Filesystem operations through FUSE
//! - Storage backend integration
//! - End-to-end workflows

pub mod operation_tracking_test;
pub mod passthrough_operations_test;
pub mod write_operations_test;
