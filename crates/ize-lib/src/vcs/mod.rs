//! Ignore filtering for VCS and other directories.
//!
//! This module provides a trait-based abstraction for detecting directories
//! and determining which paths should be ignored during filesystem observation.
//!
//! # Design
//!
//! Each filter (Git, Jujutsu, Pijul, etc.) implements the `IgnoreFilter` trait,
//! which provides:
//! - Detection of a managed directory in a given path
//! - Filtering logic to determine if a path should be ignored
//!
//! Multiple filters can coexist (e.g., both `.git` and `.jj`).
//! Observers use detected filters to decide whether to record an operation.

use std::ffi::OsStr;
use std::path::Path;

mod git;
mod jujutsu;
mod pijul;

pub use git::GitBackend;
pub use jujutsu::JujutsuBackend;
pub use pijul::PijulBackend;

/// Trait for path-based ignore filtering.
///
/// Each implementation (Git, Jujutsu, Pijul, .gitignore, tmp/, etc.)
/// provides detection logic and determines which paths should be excluded
/// from observation or recording.
pub trait IgnoreFilter: Send + Sync {
    /// Human-readable name of the filter.
    fn name(&self) -> &str;

    /// The primary directory name for this filter (e.g., ".git", ".jj", ".pijul").
    fn dir_name(&self) -> &str;

    /// Check if this filter is active in the given directory.
    ///
    /// This typically checks if a managed directory exists as a subdirectory.
    ///
    /// # Arguments
    /// * `base_path` - The root directory to check
    ///
    /// # Returns
    /// `true` if the managed directory is detected
    fn is_present(&self, base_path: &Path) -> bool;

    /// Determine if a relative path should be ignored (not recorded).
    ///
    /// # Arguments
    /// * `rel_path` - Path relative to the mount point/backing root
    ///
    /// # Returns
    /// `true` if this path should be ignored
    fn should_ignore(&self, rel_path: &Path) -> bool;
}

// Backward-compatible alias during migration
pub use IgnoreFilter as VcsBackend;

/// Detect all ignore filters present in a directory.
///
/// Returns a vector of boxed filters for all detected systems.
///
/// # Arguments
/// * `base_path` - The directory to scan
///
/// # Example
/// ```no_run
/// use std::path::Path;
/// use ize_lib::vcs::detect_all_filters;
///
/// let filters = detect_all_filters(Path::new("/path/to/repo"));
/// for filter in &filters {
///     println!("Detected: {}", filter.name());
/// }
/// ```
pub fn detect_all_filters(base_path: &Path) -> Vec<Box<dyn IgnoreFilter>> {
    let mut filters: Vec<Box<dyn IgnoreFilter>> = Vec::new();

    let candidates: Vec<Box<dyn IgnoreFilter>> = vec![
        Box::new(GitBackend),
        Box::new(JujutsuBackend),
        Box::new(PijulBackend),
    ];

    for candidate in candidates {
        if candidate.is_present(base_path) {
            filters.push(candidate);
        }
    }

    filters
}

/// Backward-compatible alias.
pub fn detect_all_vcs(base_path: &Path) -> Vec<Box<dyn IgnoreFilter>> {
    detect_all_filters(base_path)
}

/// Check if any filter says this path should be ignored.
///
/// Returns `true` if at least one filter matches.
///
/// # Arguments
/// * `filters` - The active ignore filters
/// * `rel_path` - Path relative to the mount point/backing root
pub fn should_ignore_path(filters: &[Box<dyn IgnoreFilter>], rel_path: &Path) -> bool {
    filters.iter().any(|filter| filter.should_ignore(rel_path))
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
    fn test_detect_all_filters_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let filters = detect_all_filters(tmp.path());
        assert_eq!(filters.len(), 0);
    }

    #[test]
    fn test_detect_all_filters_git() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();

        let filters = detect_all_filters(tmp.path());
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].name(), "Git");
    }

    #[test]
    fn test_detect_all_filters_multiple() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::create_dir(tmp.path().join(".jj")).unwrap();

        let filters = detect_all_filters(tmp.path());
        assert_eq!(filters.len(), 2);

        let names: Vec<&str> = filters.iter().map(|f| f.name()).collect();
        assert!(names.contains(&"Git"));
        assert!(names.contains(&"Jujutsu"));
    }

    #[test]
    fn test_should_ignore_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();

        let filters = detect_all_filters(tmp.path());

        assert!(should_ignore_path(&filters, Path::new(".git")));
        assert!(should_ignore_path(&filters, Path::new(".git/objects")));
        assert!(!should_ignore_path(&filters, Path::new("src")));
        assert!(!should_ignore_path(&filters, Path::new("README.md")));
    }

    #[test]
    fn test_backward_compat_alias() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();

        // detect_all_vcs still works
        let filters = detect_all_vcs(tmp.path());
        assert_eq!(filters.len(), 1);
    }
}
