// Main test module for claris-fuse-lib
//
// This file organizes our test modules into a clear hierarchy:
// - unit/: Unit tests that need external dependencies
// - integration/: Tests for component interactions
// - functional/: End-to-end tests
// - common/: Shared test utilities and helpers
// - fixtures/: Test data and fixtures

// Common test utilities
pub mod common;

// Import test modules from subdirectories
// Unit tests
#[path = "unit/diesel_basic_test.rs"]
mod diesel_basic_test;

#[path = "unit/diesel_isolated.rs"]
mod diesel_isolated;

#[path = "unit/timestamp_test.rs"]
mod timestamp_test;

#[path = "unit/test_timestamp.rs"]
mod test_timestamp;

// Integration tests
#[path = "integration/diesel_storage_test.rs"]
mod diesel_storage_test;

#[path = "integration/passthrough_test.rs"]
mod passthrough_test;

#[path = "integration/touch_test.rs"]
mod touch_test;

// Functional tests
#[path = "functional/mount_test.rs"]
mod functional_mount_test;

#[path = "functional/cli_commands_test.rs"]
mod cli_commands_test;