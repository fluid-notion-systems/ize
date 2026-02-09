//! Pijul VCS backend implementation.

use std::path::Path;

use super::{path_starts_with_dir, VcsBackend};

/// Pijul version control system backend.
///
/// Detects the presence of a `.pijul` directory and filters paths within it.
pub struct PijulBackend;

impl VcsBackend for PijulBackend {
    fn name(&self) -> &str {
        "Pijul"
    }

    fn vcs_dir_name(&self) -> &str {
        ".pijul"
    }

    fn is_present(&self, base_path: &Path) -> bool {
        base_path.join(".pijul").is_dir()
    }

    fn should_ignore(&self, rel_path: &Path) -> bool {
        path_starts_with_dir(rel_path, ".pijul")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pijul_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = PijulBackend;

        assert!(!backend.is_present(tmp.path()));

        std::fs::create_dir(tmp.path().join(".pijul")).unwrap();
        assert!(backend.is_present(tmp.path()));
    }

    #[test]
    fn test_pijul_should_ignore() {
        let backend = PijulBackend;

        assert!(backend.should_ignore(Path::new(".pijul")));
        assert!(backend.should_ignore(Path::new(".pijul/config")));
        assert!(backend.should_ignore(Path::new(".pijul/changes")));
        assert!(!backend.should_ignore(Path::new("src")));
        assert!(!backend.should_ignore(Path::new("src/.pijul")));
        assert!(!backend.should_ignore(Path::new("")));
    }

    #[test]
    fn test_pijul_metadata() {
        let backend = PijulBackend;
        assert_eq!(backend.name(), "Pijul");
        assert_eq!(backend.vcs_dir_name(), ".pijul");
    }
}
