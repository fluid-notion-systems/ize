//! Project management for Ize
//!
//! This module provides the `IzeProject` struct which represents a tracked
//! directory and the `ProjectManager` for managing multiple projects in
//! the central store.

mod manager;

pub use manager::{ProjectInfo, ProjectManager};

use crate::pijul::{PijulBackend, PijulError};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProjectError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Pijul error: {0}")]
    Pijul(#[from] PijulError),

    #[error("Project not found for source directory: {0}")]
    NotFound(PathBuf),

    #[error("Project already exists for directory: {0}")]
    AlreadyExists(PathBuf),

    #[error("Invalid project metadata: {0}")]
    InvalidMetadata(String),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

/// An Ize project representing a tracked directory
pub struct IzeProject {
    /// Path to the project directory in central store
    pub project_dir: PathBuf,
    /// The Pijul backend for version control
    pub pijul: PijulBackend,
    /// Path to the metadata directory
    pub meta_dir: PathBuf,
    /// The original source directory this project tracks
    pub source_dir: PathBuf,
    /// Project UUID
    pub uuid: String,
}

impl IzeProject {
    /// Initialize a new Ize project for the given source directory
    ///
    /// # Arguments
    /// * `project_dir` - Path where the project will be stored (in central store)
    /// * `source_dir` - The original directory being tracked
    pub fn init(project_dir: &Path, source_dir: &Path) -> Result<Self, ProjectError> {
        let pijul_dir = project_dir.join(".pijul");
        let working_dir = project_dir.join("working");
        let meta_dir = project_dir.join("meta");

        // Create meta directory
        std::fs::create_dir_all(&meta_dir)?;

        // Initialize pijul via backend
        let pijul = PijulBackend::init(&pijul_dir, &working_dir, None)?;

        // Copy existing contents from source_dir to working_dir if source exists
        if source_dir.exists() && source_dir.is_dir() {
            copy_dir_contents(source_dir, &working_dir)?;
        }

        // Use the project directory name as the UUID
        // This ensures the UUID in metadata matches the directory name
        let uuid = project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let now = chrono::Utc::now().to_rfc3339();

        let metadata = ProjectMetadata {
            project: ProjectSection {
                uuid: uuid.clone(),
                source_dir: source_dir.display().to_string(),
                created: now,
            },
            pijul: PijulSection {
                default_channel: pijul.current_channel().to_string(),
            },
        };

        // Write project metadata
        let meta_toml = toml::to_string_pretty(&metadata)?;
        std::fs::write(meta_dir.join("project.toml"), meta_toml)?;

        Ok(Self {
            project_dir: project_dir.to_path_buf(),
            pijul,
            meta_dir,
            source_dir: source_dir.to_path_buf(),
            uuid,
        })
    }

    /// Open an existing project from its project directory
    pub fn open(project_dir: &Path) -> Result<Self, ProjectError> {
        let pijul_dir = project_dir.join(".pijul");
        let working_dir = project_dir.join("working");
        let meta_dir = project_dir.join("meta");
        let meta_path = meta_dir.join("project.toml");

        // Read metadata
        let content = std::fs::read_to_string(&meta_path)?;
        let metadata: ProjectMetadata = toml::from_str(&content)?;

        let pijul = PijulBackend::open(&pijul_dir, &working_dir)?;

        Ok(Self {
            project_dir: project_dir.to_path_buf(),
            pijul,
            meta_dir,
            source_dir: PathBuf::from(&metadata.project.source_dir),
            uuid: metadata.project.uuid,
        })
    }

    /// Get the working directory path (where actual files are stored)
    pub fn working_dir(&self) -> &Path {
        self.pijul.working_dir()
    }

    /// Get the pijul directory path
    pub fn pijul_dir(&self) -> &Path {
        self.pijul.pijul_dir()
    }

    /// Get the project UUID
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// Get the source directory this project tracks
    pub fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    /// List all channels in this project
    pub fn list_channels(&self) -> Result<Vec<String>, ProjectError> {
        Ok(self.pijul.list_channels()?)
    }

    /// Get the current channel name
    pub fn current_channel(&self) -> &str {
        self.pijul.current_channel()
    }

    /// Create a new channel
    pub fn create_channel(&self, name: &str) -> Result<(), ProjectError> {
        Ok(self.pijul.create_channel(name)?)
    }

    /// Switch to a different channel
    pub fn switch_channel(&mut self, name: &str) -> Result<(), ProjectError> {
        Ok(self.pijul.switch_channel(name)?)
    }
}

impl std::fmt::Debug for IzeProject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IzeProject")
            .field("project_dir", &self.project_dir)
            .field("source_dir", &self.source_dir)
            .field("uuid", &self.uuid)
            .field("current_channel", &self.pijul.current_channel())
            .finish()
    }
}

/// Project metadata stored in project.toml
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ProjectMetadata {
    pub project: ProjectSection,
    pub pijul: PijulSection,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ProjectSection {
    pub uuid: String,
    pub source_dir: String,
    pub created: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PijulSection {
    pub default_channel: String,
}

/// Recursively copy directory contents from src to dst
fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_contents(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &dst_path)?;
        } else if file_type.is_symlink() {
            let target = std::fs::read_link(&src_path)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dst_path)?;
            #[cfg(windows)]
            {
                if target.is_dir() {
                    std::os::windows::fs::symlink_dir(&target, &dst_path)?;
                } else {
                    std::os::windows::fs::symlink_file(&target, &dst_path)?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_project_init_empty_dir() {
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("project");
        let source_dir = temp.path().join("source");

        std::fs::create_dir_all(&source_dir).unwrap();

        let project = IzeProject::init(&project_dir, &source_dir).unwrap();

        assert!(project.pijul_dir().exists());
        assert!(project.working_dir().exists());
        assert!(project.meta_dir.join("project.toml").exists());
        assert_eq!(project.current_channel(), "main");
    }

    #[test]
    fn test_project_init_with_files() {
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("project");
        let source_dir = temp.path().join("source");

        // Create source directory with files
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("README.md"), "# Test Project").unwrap();
        std::fs::create_dir_all(source_dir.join("src")).unwrap();
        std::fs::write(source_dir.join("src/main.rs"), "fn main() {}").unwrap();

        let project = IzeProject::init(&project_dir, &source_dir).unwrap();

        // Verify files were copied
        assert!(project.working_dir().join("README.md").exists());
        assert!(project.working_dir().join("src/main.rs").exists());

        let content = std::fs::read_to_string(project.working_dir().join("README.md")).unwrap();
        assert_eq!(content, "# Test Project");
    }

    #[test]
    fn test_project_open() {
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("project");
        let source_dir = temp.path().join("source");

        std::fs::create_dir_all(&source_dir).unwrap();

        // Initialize and get UUID
        let project = IzeProject::init(&project_dir, &source_dir).unwrap();
        let uuid = project.uuid().to_string();
        drop(project);

        // Open and verify
        let project = IzeProject::open(&project_dir).unwrap();
        assert_eq!(project.uuid(), uuid);
        assert_eq!(project.source_dir(), source_dir);
    }

    #[test]
    fn test_channel_operations() {
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("project");
        let source_dir = temp.path().join("source");

        std::fs::create_dir_all(&source_dir).unwrap();

        let mut project = IzeProject::init(&project_dir, &source_dir).unwrap();

        // Create a new channel
        project.create_channel("feature").unwrap();

        let channels = project.list_channels().unwrap();
        assert!(channels.contains(&"main".to_string()));
        assert!(channels.contains(&"feature".to_string()));

        // Switch to the new channel
        project.switch_channel("feature").unwrap();
        assert_eq!(project.current_channel(), "feature");
    }
}
