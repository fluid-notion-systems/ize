pub mod error;
pub mod observing;
pub mod passthrough;

// Re-export key types for convenience
pub use observing::{FsObserver, ObservingFS};
pub use passthrough::PassthroughFS;
