pub mod error;
pub mod observing;
pub mod passthrough;
pub mod passthrough_fd;

// Re-export key types for convenience
pub use observing::{FsObserver, ObservingFS};
pub use passthrough::PassthroughFS;
pub use passthrough_fd::FdPassthroughFS;
