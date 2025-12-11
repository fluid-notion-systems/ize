// pub mod domain;
// pub mod filesystem;
pub mod cli;
pub mod filesystems;
pub mod operations;
pub mod pijul;
pub mod project;
pub mod storage;

pub use pijul::{PijulBackend, PijulError};
pub use project::{IzeProject, ProjectError, ProjectInfo, ProjectManager};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_not_empty() {
        assert!(!version().is_empty());
    }
}
