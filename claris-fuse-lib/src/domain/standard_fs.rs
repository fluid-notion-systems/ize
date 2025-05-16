use std::path::{Path, PathBuf};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use async_trait::async_trait;

use super::models::DomainError;
use super::repositories::{FileSystemRepository, RepositoryResult};

/// Standard filesystem implementation of the FileSystemRepository
pub struct StandardFileSystem {
    root_dir: PathBuf,
}

impl StandardFileSystem {
    /// Create a new standard filesystem with the given root directory
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        Self {
            root_dir: root_dir.as_ref().to_path_buf(),
        }
    }
    
    /// Get the absolute path by joining with the root directory
    fn absolute_path(&self, path: &Path) -> PathBuf {
        self.root_dir.join(path.strip_prefix("/").unwrap_or(path))
    }
    
    /// Create parent directories if needed
    fn ensure_parent_dir(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        Ok(())
    }
    
    /// Convert an IO error to a domain error
    fn io_to_domain_error(err: io::Error, path: &Path) -> DomainError {
        match err.kind() {
            io::ErrorKind::NotFound => DomainError::FileNotFound(path.to_path_buf()),
            _ => DomainError::InternalError(format!("IO error: {}", err)),
        }
    }
}

#[async_trait]
impl FileSystemRepository for StandardFileSystem {
    async fn init(&self) -> RepositoryResult<()> {
        // Ensure the root directory exists
        fs::create_dir_all(&self.root_dir)
            .map_err(|e| DomainError::InternalError(format!("Failed to create root directory: {}", e)))?;
        Ok(())
    }
    
    async fn close(&self) -> RepositoryResult<()> {
        // Nothing to clean up for a standard filesystem
        Ok(())
    }
    
    async fn read_file(&self, path: &Path) -> RepositoryResult<Vec<u8>> {
        let absolute_path = self.absolute_path(path);
        
        let mut file = File::open(&absolute_path)
            .map_err(|e| Self::io_to_domain_error(e, path))?;
        
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)
            .map_err(|e| DomainError::InternalError(format!("Failed to read file {}: {}", path.display(), e)))?;
        
        Ok(contents)
    }
    
    async fn write_file(&self, path: &Path, content: &[u8]) -> RepositoryResult<()> {
        let absolute_path = self.absolute_path(path);
        
        // Ensure parent directory exists
        self.ensure_parent_dir(&absolute_path)
            .map_err(|e| DomainError::InternalError(format!("Failed to create parent directory: {}", e)))?;
        
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&absolute_path)
            .map_err(|e| DomainError::InternalError(format!("Failed to open file for writing: {}", e)))?;
        
        file.write_all(content)
            .map_err(|e| DomainError::InternalError(format!("Failed to write to file: {}", e)))?;
        
        Ok(())
    }
    
    async fn delete_file(&self, path: &Path) -> RepositoryResult<()> {
        let absolute_path = self.absolute_path(path);
        
        fs::remove_file(&absolute_path)
            .map_err(|e| Self::io_to_domain_error(e, path))?;
        
        Ok(())
    }
    
    async fn rename_file(&self, from: &Path, to: &Path) -> RepositoryResult<()> {
        let absolute_from = self.absolute_path(from);
        let absolute_to = self.absolute_path(to);
        
        // Ensure parent directory exists for destination
        self.ensure_parent_dir(&absolute_to)
            .map_err(|e| DomainError::InternalError(format!("Failed to create parent directory: {}", e)))?;
        
        fs::rename(&absolute_from, &absolute_to)
            .map_err(|e| DomainError::InternalError(format!("Failed to rename file: {}", e)))?;
        
        Ok(())
    }
    
    async fn file_exists(&self, path: &Path) -> RepositoryResult<bool> {
        let absolute_path = self.absolute_path(path);
        Ok(absolute_path.exists() && absolute_path.is_file())
    }
    
    async fn get_metadata(&self, path: &Path) -> RepositoryResult<fs::Metadata> {
        let absolute_path = self.absolute_path(path);
        
        absolute_path.metadata()
            .map_err(|e| Self::io_to_domain_error(e, path))
    }
    
    async fn list_directory(&self, path: &Path) -> RepositoryResult<Vec<PathBuf>> {
        let absolute_path = self.absolute_path(path);
        
        if !absolute_path.is_dir() {
            return Err(DomainError::InternalError(format!("Path is not a directory: {}", path.display())));
        }
        
        let entries = fs::read_dir(&absolute_path)
            .map_err(|e| DomainError::InternalError(format!("Failed to read directory: {}", e)))?;
        
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| DomainError::InternalError(format!("Failed to read directory entry: {}", e)))?;
            
            // Convert absolute path back to relative path
            let entry_path = entry.path();
            if let Ok(relative) = entry_path.strip_prefix(&self.root_dir) {
                paths.push(relative.to_path_buf());
            } else {
                // This shouldn't happen, but just in case
                paths.push(entry_path.file_name().unwrap().into());
            }
        }
        
        Ok(paths)
    }
    
    async fn create_directory(&self, path: &Path) -> RepositoryResult<()> {
        let absolute_path = self.absolute_path(path);
        
        fs::create_dir_all(&absolute_path)
            .map_err(|e| DomainError::InternalError(format!("Failed to create directory: {}", e)))?;
        
        Ok(())
    }
    
    async fn remove_directory(&self, path: &Path) -> RepositoryResult<()> {
        let absolute_path = self.absolute_path(path);
        
        fs::remove_dir_all(&absolute_path)
            .map_err(|e| Self::io_to_domain_error(e, path))?;
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    
    #[tokio::test]
    async fn test_file_operations() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let fs_repo = StandardFileSystem::new(temp_dir.path());
        
        // Initialize
        fs_repo.init().await.expect("Failed to initialize");
        
        // Test write and read
        let test_path = Path::new("/test.txt");
        let test_content = b"Hello, world!";
        
        fs_repo.write_file(test_path, test_content).await.expect("Failed to write file");
        
        let read_content = fs_repo.read_file(test_path).await.expect("Failed to read file");
        assert_eq!(read_content, test_content);
        
        // Test file exists
        assert!(fs_repo.file_exists(test_path).await.expect("Failed to check if file exists"));
        
        // Test get metadata
        let metadata = fs_repo.get_metadata(test_path).await.expect("Failed to get metadata");
        assert_eq!(metadata.len(), test_content.len() as u64);
        
        // Test rename
        let renamed_path = Path::new("/renamed.txt");
        fs_repo.rename_file(test_path, renamed_path).await.expect("Failed to rename file");
        
        assert!(!fs_repo.file_exists(test_path).await.expect("Failed to check if file exists"));
        assert!(fs_repo.file_exists(renamed_path).await.expect("Failed to check if file exists"));
        
        // Test directory operations
        let dir_path = Path::new("/test_dir");
        fs_repo.create_directory(dir_path).await.expect("Failed to create directory");
        
        // Write a file in the directory
        let file_in_dir_path = Path::new("/test_dir/file.txt");
        fs_repo.write_file(file_in_dir_path, b"File in directory").await.expect("Failed to write file");
        
        // List directory
        let dir_contents = fs_repo.list_directory(dir_path).await.expect("Failed to list directory");
        assert_eq!(dir_contents.len(), 1);
        
        // Delete file and directory
        fs_repo.delete_file(renamed_path).await.expect("Failed to delete file");
        fs_repo.remove_directory(dir_path).await.expect("Failed to remove directory");
        
        assert!(!fs_repo.file_exists(renamed_path).await.expect("Failed to check if file exists"));
        assert!(!fs_repo.file_exists(dir_path).await.expect("Failed to check if file exists"));
    }
}