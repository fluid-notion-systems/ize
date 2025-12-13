//! Project Manager for Ize
//!
//! This module provides the `ProjectManager` struct for managing multiple
//! Ize projects stored in the central location (~/.local/share/ize/projects/).

use super::{IzeProject, ProjectError, ProjectMetadata};
use std::path::{Path, PathBuf};

/// Information about a tracked project
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    /// Project UUID
    pub uuid: String,
    /// Original source directory being tracked
    pub source_dir: PathBuf,
    /// Path to the project in central store
    pub project_path: PathBuf,
    /// When the project was created
    pub created: String,
    /// Default channel name
    pub default_channel: String,
    /// Project name (from config or dirname fallback)
    pub name: String,
}

/// Manages multiple Ize projects in the central store
pub struct ProjectManager {
    /// Path to the central directory (e.g., ~/.local/share/ize)
    central_dir: PathBuf,
}

impl ProjectManager {
    /// Create a new ProjectManager using the default central directory
    ///
    /// Default location: `~/.local/share/ize`
    pub fn new() -> Result<Self, ProjectError> {
        let central_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ize");

        Self::with_central_dir(central_dir)
    }

    /// Create a new ProjectManager with a custom central directory
    pub fn with_central_dir(central_dir: PathBuf) -> Result<Self, ProjectError> {
        std::fs::create_dir_all(&central_dir)?;
        std::fs::create_dir_all(central_dir.join("projects"))?;

        Ok(Self { central_dir })
    }

    /// Get the path to the central directory
    pub fn central_dir(&self) -> &Path {
        &self.central_dir
    }

    /// Get the path to the projects directory
    pub fn projects_dir(&self) -> PathBuf {
        self.central_dir.join("projects")
    }

    /// Create a new project for the given source directory
    ///
    /// # Arguments
    /// * `source_dir` - The directory to track (will be canonicalized)
    ///
    /// # Returns
    /// The newly created `IzeProject`
    pub fn create_project(&self, source_dir: &Path) -> Result<IzeProject, ProjectError> {
        // Canonicalize the source directory
        let source_dir = std::fs::canonicalize(source_dir)?;

        // Check if this directory is already tracked
        if self.find_by_source_dir(&source_dir)?.is_some() {
            return Err(ProjectError::AlreadyExists(source_dir));
        }

        // Generate UUID for the project
        let uuid = uuid::Uuid::new_v4();
        let project_dir = self.projects_dir().join(uuid.to_string());

        IzeProject::init(&project_dir, &source_dir)
    }

    /// Find a project by its source directory path
    ///
    /// # Arguments
    /// * `source_dir` - The source directory to look up
    ///
    /// # Returns
    /// `Some(IzeProject)` if found, `None` otherwise
    pub fn find_by_source_dir(
        &self,
        source_dir: &Path,
    ) -> Result<Option<IzeProject>, ProjectError> {
        let projects_dir = self.projects_dir();

        // Canonicalize for comparison
        let source_dir = match std::fs::canonicalize(source_dir) {
            Ok(p) => p,
            Err(_) => return Ok(None), // If we can't canonicalize, it doesn't exist
        };

        if !projects_dir.exists() {
            return Ok(None);
        }

        for entry in std::fs::read_dir(&projects_dir)? {
            let entry = entry?;
            let meta_path = entry.path().join("meta").join("project.toml");

            if meta_path.exists() {
                let content = std::fs::read_to_string(&meta_path)?;
                if let Ok(meta) = toml::from_str::<ProjectMetadata>(&content) {
                    // Canonicalize the stored source_dir for comparison
                    let stored_source = PathBuf::from(&meta.project.source_dir);
                    if let Ok(stored_canonical) = std::fs::canonicalize(&stored_source) {
                        if stored_canonical == source_dir {
                            return Ok(Some(IzeProject::open(&entry.path())?));
                        }
                    } else if stored_source == source_dir {
                        // Fallback to direct comparison if can't canonicalize
                        return Ok(Some(IzeProject::open(&entry.path())?));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Find a project by its UUID
    pub fn find_by_uuid(&self, uuid: &str) -> Result<Option<IzeProject>, ProjectError> {
        let project_dir = self.projects_dir().join(uuid);

        if project_dir.exists() {
            Ok(Some(IzeProject::open(&project_dir)?))
        } else {
            Ok(None)
        }
    }

    /// List all tracked projects
    pub fn list_projects(&self) -> Result<Vec<ProjectInfo>, ProjectError> {
        let projects_dir = self.projects_dir();
        let mut projects = Vec::new();

        if !projects_dir.exists() {
            return Ok(projects);
        }

        for entry in std::fs::read_dir(&projects_dir)? {
            let entry = entry?;
            let meta_path = entry.path().join("meta").join("project.toml");

            if meta_path.exists() {
                let content = std::fs::read_to_string(&meta_path)?;
                if let Ok(meta) = toml::from_str::<ProjectMetadata>(&content) {
                    // Use config name if available, otherwise fallback to source dir name
                    let name = meta.project.name.clone().unwrap_or_else(|| {
                        PathBuf::from(&meta.project.source_dir)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "Unknown".to_string())
                    });

                    projects.push(ProjectInfo {
                        uuid: meta.project.uuid,
                        source_dir: PathBuf::from(&meta.project.source_dir),
                        project_path: entry.path(),
                        created: meta.project.created,
                        default_channel: meta.pijul.default_channel,
                        name,
                    });
                }
            }
        }

        Ok(projects)
    }

    /// Delete a project by its source directory
    ///
    /// This removes the project from the central store but does NOT
    /// modify the original source directory.
    pub fn delete_project(&self, source_dir: &Path) -> Result<bool, ProjectError> {
        if let Some(project) = self.find_by_source_dir(source_dir)? {
            std::fs::remove_dir_all(&project.project_dir)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Delete a project by its UUID
    pub fn delete_project_by_uuid(&self, uuid: &str) -> Result<bool, ProjectError> {
        let project_dir = self.projects_dir().join(uuid);

        if project_dir.exists() {
            std::fs::remove_dir_all(&project_dir)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl Default for ProjectManager {
    fn default() -> Self {
        Self::new().expect("Failed to create default ProjectManager")
    }
}

impl std::fmt::Debug for ProjectManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectManager")
            .field("central_dir", &self.central_dir)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_and_find_project() {
        let temp = TempDir::new().unwrap();
        let central_dir = temp.path().join("central");
        let source_dir = temp.path().join("my-project");

        // Create source directory
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("README.md"), "# My Project").unwrap();

        let manager = ProjectManager::with_central_dir(central_dir).unwrap();

        // Create project
        let project = manager.create_project(&source_dir).unwrap();
        let uuid = project.uuid().to_string();

        // Find by source dir
        let found = manager.find_by_source_dir(&source_dir).unwrap().unwrap();
        assert_eq!(found.uuid(), uuid);

        // Find by UUID
        let found = manager.find_by_uuid(&uuid).unwrap().unwrap();
        assert_eq!(
            found.source_dir(),
            std::fs::canonicalize(&source_dir).unwrap()
        );
    }

    #[test]
    fn test_list_projects() {
        let temp = TempDir::new().unwrap();
        let central_dir = temp.path().join("central");

        let manager = ProjectManager::with_central_dir(central_dir).unwrap();

        // Create multiple projects
        for i in 0..3 {
            let source_dir = temp.path().join(format!("project-{}", i));
            std::fs::create_dir_all(&source_dir).unwrap();
            manager.create_project(&source_dir).unwrap();
        }

        let projects = manager.list_projects().unwrap();
        assert_eq!(projects.len(), 3);
    }

    #[test]
    fn test_duplicate_project_fails() {
        let temp = TempDir::new().unwrap();
        let central_dir = temp.path().join("central");
        let source_dir = temp.path().join("my-project");

        std::fs::create_dir_all(&source_dir).unwrap();

        let manager = ProjectManager::with_central_dir(central_dir).unwrap();

        // First create succeeds
        manager.create_project(&source_dir).unwrap();

        // Second create should fail
        let result = manager.create_project(&source_dir);
        assert!(matches!(result, Err(ProjectError::AlreadyExists(_))));
    }

    #[test]
    fn test_delete_project() {
        let temp = TempDir::new().unwrap();
        let central_dir = temp.path().join("central");
        let source_dir = temp.path().join("my-project");

        std::fs::create_dir_all(&source_dir).unwrap();

        let manager = ProjectManager::with_central_dir(central_dir).unwrap();

        // Create and then delete
        let project = manager.create_project(&source_dir).unwrap();
        let project_path = project.project_dir.clone();

        assert!(manager.delete_project(&source_dir).unwrap());
        assert!(!project_path.exists());

        // Should not find it anymore
        assert!(manager.find_by_source_dir(&source_dir).unwrap().is_none());
    }

    #[test]
    fn test_find_nonexistent_project() {
        let temp = TempDir::new().unwrap();
        let central_dir = temp.path().join("central");
        let source_dir = temp.path().join("nonexistent");

        let manager = ProjectManager::with_central_dir(central_dir).unwrap();

        assert!(manager.find_by_source_dir(&source_dir).unwrap().is_none());
        assert!(manager.find_by_uuid("nonexistent-uuid").unwrap().is_none());
    }
}
