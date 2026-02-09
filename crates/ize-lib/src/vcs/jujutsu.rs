//! Jujutsu ignore filter implementation.

use std::path::Path;

use super::{path_starts_with_dir, IgnoreFilter};

/// Jujutsu version control system ignore filter.
///
/// Detects the presence of a `.jj` directory and filters paths within it.
pub struct JujutsuBackend;

impl IgnoreFilter for JujutsuBackend {
    fn name(&self) -> &str {
        "Jujutsu"
    }

    fn dir_name(&self) -> &str {
        ".jj"
    }

    fn is_present(&self, base_path: &Path) -> bool {
        base_path.join(".jj").is_dir()
    }

    fn should_ignore(&self, rel_path: &Path) -> bool {
        path_starts_with_dir(rel_path, ".jj")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jujutsu_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = JujutsuBackend;

        assert!(!backend.is_present(tmp.path()));

        std::fs::create_dir(tmp.path().join(".jj")).unwrap();
        assert!(backend.is_present(tmp.path()));
    }

    #[test]
    fn test_jujutsu_should_ignore() {
        let backend = JujutsuBackend;

        assert!(backend.should_ignore(Path::new(".jj")));
        assert!(backend.should_ignore(Path::new(".jj/repo")));
        assert!(backend.should_ignore(Path::new(".jj/repo/store")));
        assert!(!backend.should_ignore(Path::new("src")));
        assert!(!backend.should_ignore(Path::new("src/.jj")));
        assert!(!backend.should_ignore(Path::new("")));
    }

    #[test]
    fn test_jujutsu_metadata() {
        let backend = JujutsuBackend;
        assert_eq!(backend.name(), "Jujutsu");
        assert_eq!(backend.dir_name(), ".jj");
    }
}
