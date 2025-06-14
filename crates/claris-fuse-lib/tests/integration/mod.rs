//! Integration tests for Claris-FUSE
//!
//! These tests verify the complete system behavior, including:
//! - Filesystem operations through FUSE
//! - Storage backend integration
//! - End-to-end workflows

pub mod passthrough_operations_test;

// The mount test is available but may require special permissions
#[cfg(feature = "mount-tests")]
pub mod passthrough_mount_test;
