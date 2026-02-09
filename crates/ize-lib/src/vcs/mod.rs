//! Version Control System (VCS) detection and filtering.
//!
//! This module provides a trait-based abstraction for detecting VCS directories
//! and determining which paths should be ignored during filesystem observation.
//!
//! # Design
//!
//! Each VCS (Git, Jujutsu, Pijul) implements the `VcsBackend` trait, which provides:
//! - Detection of the VCS directory in a given path
//! - Filtering logic to determine if a path should be ignored
//!
//! Multiple VCS systems can coexist in the same directory (e.g., both `.git` and `.jj`).
//! The `ObservingFS` queries all detected VCS backends to decide whether to observe a path.

use std::ffi::OsStr;
use std::path::Path;

mod git;
mod jujutsu;
mod pijul;

pub use git::GitBackend;
pub use jujutsu::JujutsuBackend;
pub use pijul::PijulBackend;

/// Trait for VCS detection and path filtering.
///
/// Each VCS implementation (Git, Jujutsu, Pijul) provides detection logic
/// and determines which paths should be excluded from observation.
pub trait VcsBackend: Send + Sync {
    /// Human-readable name of the VCS.
    fn name(&self) -> &str;

    /// The primary directory name for this VCS (e.g., ".git", ".jj", ".pijul").
    fn vcs_dir_name(&self) -> &str;

    /// Check if this VCS is present in the given directory.
    ///
    /// This typically checks if the VCS directory exists as a subdirectory.
    ///
    /// # Arguments
    /// * `base_path` - The root directory to check
    ///
    /// # Returns
    /// `true` if the VCS directory is detected
    fn is_present(&self, base_path: &Path) -> bool;

    /// Determine if a relative path should be ignored (not observed).
    ///
    /// # Arguments
    /// * `rel_path` - Path relative to the mount point/backing root
    ///
    /// # Returns
    /// `true` if this path should be ignored (not observed)
    fn should_ignore(&self, rel_path: &Path) -> bool;
}

/// Detect all VCS systems present in a directory.
///
/// Returns a vector of boxed VCS backends for all detected VCS systems.
///
/// # Arguments
/// * `base_path` - The directory to scan for VCS systems
///
/// # Example
/// ```no_run
/// use std::path::Path;
/// use ize_lib::vcs::detect_all_vcs;
///
/// let vcs_backends = detect_all_vcs(Path::new("/path/to/repo"));
/// for backend in &vcs_backends {
///     println!("Detected: {}", backend.name());
/// }
/// ```
pub fn detect_all_vcs(base_path: &Path) -> Vec<Box<dyn VcsBackend>> {
    let mut backends: Vec<Box<dyn VcsBackend>> = Vec::new();

    // Try each VCS backend
    let candidates: Vec<Box<dyn VcsBackend>> = vec![
        Box::new(GitBackend),
        Box::new(JujutsuBackend),
        Box::new(PijulBackend),
    ];

    for backend in candidates {
        if backend.is_present(base_path) {
            backends.push(backend);
        }
    }

    backends
}

/// Check if any VCS backend should ignore this path.
///
/// Returns `true` if at least one VCS backend says the path should be ignored.
///
/// # Arguments
/// * `backends` - The detected VCS backends
/// * `rel_path` - Path relative to the mount point/backing root
pub fn should_ignore_path(backends: &[Box<dyn VcsBackend>], rel_path: &Path) -> bool {
    backends
        .iter()
        .any(|backend| backend.should_ignore(rel_path))
}

/// Helper function to check if a path starts with a specific directory component.
///
/// # Arguments
/// * `path` - The path to check
/// * `dir_name` - The directory name to look for as the first component
pub(crate) fn path_starts_with_dir(path: &Path, dir_name: &str) -> bool {
    if let Some(first) = path.components().next() {
        let first_os: &OsStr = first.as_os_str();
        first_os == dir_name
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_path_starts_with_dir() {
        assert!(path_starts_with_dir(Path::new(".git"), ".git"));
        assert!(path_starts_with_dir(Path::new(".git/objects"), ".git"));
        assert!(!path_starts_with_dir(Path::new("src"), ".git"));
        assert!(!path_starts_with_dir(Path::new("src/.git"), ".git"));
        assert!(!path_starts_with_dir(Path::new(""), ".git"));
    }

    #[test]
    fn test_detect_all_vcs_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let backends = detect_all_vcs(tmp.path());
        assert_eq!(backends.len(), 0);
    }

    #[test]
    fn test_detect_all_vcs_git() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();

        let backends = detect_all_vcs(tmp.path());
        assert_eq!(backends.len(), 1);
        assert_eq!(backends[0].name(), "Git");
    }

    #[test]
    fn test_detect_all_vcs_multiple() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::create_dir(tmp.path().join(".jj")).unwrap();

        let backends = detect_all_vcs(tmp.path());
        assert_eq!(backends.len(), 2);

        let names: Vec<&str> = backends.iter().map(|b| b.name()).collect();
        assert!(names.contains(&"Git"));
        assert!(names.contains(&"Jujutsu"));
    }

    #[test]
    fn test_should_ignore_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();

        let backends = detect_all_vcs(tmp.path());

        assert!(should_ignore_path(&backends, Path::new(".git")));
        assert!(should_ignore_path(&backends, Path::new(".git/objects")));
        assert!(!should_ignore_path(&backends, Path::new("src")));
        assert!(!should_ignore_path(&backends, Path::new("README.md")));
    }
}
