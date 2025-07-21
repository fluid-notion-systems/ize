//! Test suite for Ize
//!
//! This test suite is organized into several categories:
//! - `unit`: Fast, isolated tests of individual components
//! - `functional`: Tests of complete features with real filesystem operations
//! - `integration`: End-to-end tests of the full system
//! - `benchmarks`: Performance benchmarks
//!
//! All tests use the harness framework defined in the `common` module to
//! eliminate duplicate setup code and ensure consistent test environments.

pub mod common;
pub mod functional;
pub mod integration;

// Test modules are automatically discovered by cargo test
// They don't need to be declared here unless we want to share
// code between test files
