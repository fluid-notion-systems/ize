//! Git VCS backend implementation.

use std::path::Path;

use super::{path_starts_with_dir, VcsBackend};

/// Git version control system backend.
///
/// Detects the presence of a `.git` directory and filters paths within it.
pub struct GitBackend;

impl VcsBackend for GitBackend {
    fn name(&self) -> &str {
        "Git"
    }

    fn vcs_dir_name(&self) -> &str {
        ".git"
    }

    fn is_present(&self, base_path: &Path) -> bool {
        base_path.join(".git").is_dir()
    }

    fn should_ignore(&self, rel_path: &Path) -> bool {
        path_starts_with_dir(rel_path, ".git")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = GitBackend;

        assert!(!backend.is_present(tmp.path()));

        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        assert!(backend.is_present(tmp.path()));
    }

    #[test]
    fn test_git_should_ignore() {
        let backend = GitBackend;

        assert!(backend.should_ignore(Path::new(".git")));
        assert!(backend.should_ignore(Path::new(".git/objects")));
        assert!(backend.should_ignore(Path::new(".git/objects/abc123")));
        assert!(!backend.should_ignore(Path::new("src")));
        assert!(!backend.should_ignore(Path::new("src/.git")));
        assert!(!backend.should_ignore(Path::new(".github")));
        assert!(!backend.should_ignore(Path::new("")));
    }

    #[test]
    fn test_git_metadata() {
        let backend = GitBackend;
        assert_eq!(backend.name(), "Git");
        assert_eq!(backend.vcs_dir_name(), ".git");
    }
}
