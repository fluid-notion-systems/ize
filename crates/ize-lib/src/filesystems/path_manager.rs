use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use log::debug;

/// PathManager helps manage virtual paths in a FUSE filesystem.
/// 
/// It handles the mapping between inodes and paths, providing consistent
/// path transformation and special handling for the root directory.
pub struct PathManager {
    /// Maps path -> inode
    path_map: HashMap<PathBuf, u64>,
    
    /// Maps inode -> path
    inode_map: HashMap<u64, PathBuf>,
    
    /// Next available inode number (starting after root)
    next_inode: AtomicU64,
    
    /// Path to the source directory
    source_dir: PathBuf,
}

/// The root inode number, hardcoded as 1
pub const ROOT_INODE: u64 = 1;

/// Path transformation types for consistent handling
pub enum PathForm {
    /// Absolute path with leading slash (e.g., "/dir/file.txt")
    Absolute,
    /// Relative path without leading slash (e.g., "dir/file.txt")
    Relative,
    /// Real path in the source directory (e.g., "/path/to/source/dir/file.txt")
    Real,
}

impl PathManager {
    /// Create a new PathManager with a given source directory
    pub fn new(source_dir: &Path) -> Self {
        let mut path_manager = PathManager {
            path_map: HashMap::new(),
            inode_map: HashMap::new(),
            next_inode: AtomicU64::new(2), // Start after root
            source_dir: source_dir.to_path_buf(),
        };
        
        // Initialize root inode mapping
        path_manager.path_map.insert(PathBuf::from(""), ROOT_INODE);
        path_manager.inode_map.insert(ROOT_INODE, PathBuf::from(""));
        
        path_manager
    }
    
    /// Convert a path to its requested form
    pub fn transform_path(&self, path: &Path, form: PathForm) -> PathBuf {
        match form {
            PathForm::Absolute => {
                if path.starts_with("/") || path.as_os_str().is_empty() {
                    path.to_path_buf()
                } else {
                    PathBuf::from("/").join(path)
                }
            },
            PathForm::Relative => {
                path.strip_prefix("/").unwrap_or(path).to_path_buf()
            },
            PathForm::Real => {
                let clean_path = path.strip_prefix("/").unwrap_or(path);
                let result = self.source_dir.join(clean_path);
                debug!("Transforming virtual path {:?} to real path {:?}", path, result);
                result
            }
        }
    }
    
    /// Get the real path in the source directory
    pub fn get_real_path(&self, path: &Path) -> PathBuf {
        self.transform_path(path, PathForm::Real)
    }
    
    /// Build a path from parent inode and name
    pub fn build_path(&self, parent: u64, name: &Path) -> Option<PathBuf> {
        if parent == ROOT_INODE {
            // Root directory
            Some(PathBuf::from("/").join(name))
        } else if let Some(parent_path) = self.get_path(parent) {
            // Build path properly based on parent
            let abs_parent = self.transform_path(&parent_path, PathForm::Absolute);
            Some(abs_parent.join(name))
        } else {
            None
        }
    }
    
    /// Get an inode number for a path, creating a new one if needed
    pub fn get_or_create_inode(&mut self, path: &Path) -> u64 {
        // Ensure we're using a relative path
        let rel_path = self.transform_path(path, PathForm::Relative);
        
        // Special case for root
        if rel_path.as_os_str().is_empty() {
            return ROOT_INODE;
        }
        
        // Look up existing inode
        if let Some(&ino) = self.path_map.get(&rel_path) {
            return ino;
        }
        
        // Create new inode
        let ino = self.next_inode.fetch_add(1, Ordering::SeqCst);
        self.path_map.insert(rel_path.clone(), ino);
        self.inode_map.insert(ino, rel_path);
        ino
    }
    
    /// Get a path for an inode number
    pub fn get_path(&self, ino: u64) -> Option<PathBuf> {
        if ino == ROOT_INODE {
            return Some(PathBuf::from("/"));
        }
        self.inode_map.get(&ino).cloned()
    }
    
    /// Update path mapping when a file or directory is renamed
    pub fn update_path(&mut self, old_path: &Path, new_path: &Path) -> bool {
        let old_rel_path = self.transform_path(old_path, PathForm::Relative);
        let new_rel_path = self.transform_path(new_path, PathForm::Relative);
        
        // Find the inode for this path
        if let Some(&ino) = self.path_map.get(&old_rel_path) {
            // Update the mappings
            self.inode_map.remove(&ino);
            self.path_map.remove(&old_rel_path);
            
            self.inode_map.insert(ino, new_rel_path.clone());
            self.path_map.insert(new_rel_path.clone(), ino);
            
            debug!(
                "Updated inode {} mapping from {:?} to {:?}",
                ino, old_rel_path, new_rel_path
            );
            true
        } else {
            false
        }
    }
    
    /// Remove a path from the mappings
    pub fn remove_path(&mut self, path: &Path) -> Option<u64> {
        let rel_path = self.transform_path(path, PathForm::Relative);
        
        if let Some(&ino) = self.path_map.get(&rel_path) {
            self.path_map.remove(&rel_path);
            self.inode_map.remove(&ino);
            Some(ino)
        } else {
            None
        }
    }
    
    /// Check if a path should be excluded from directory listings
    pub fn should_exclude_path(&self, path: &Path, db_filename: Option<&Path>) -> bool {
        if let Some(db_name) = db_filename {
            if let Some(file_name) = path.file_name() {
                if let Some(db_file_name) = db_name.file_name() {
                    return file_name == db_file_name;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_path_transformations() {
        let source_dir = PathBuf::from("/tmp/source");
        let manager = PathManager::new(&source_dir);
        
        // Test absolute paths
        assert_eq!(
            manager.transform_path(Path::new("a/b"), PathForm::Absolute),
            PathBuf::from("/a/b")
        );
        
        assert_eq!(
            manager.transform_path(Path::new("/a/b"), PathForm::Absolute),
            PathBuf::from("/a/b")
        );
        
        // Test relative paths
        assert_eq!(
            manager.transform_path(Path::new("/a/b"), PathForm::Relative),
            PathBuf::from("a/b")
        );
        
        assert_eq!(
            manager.transform_path(Path::new("a/b"), PathForm::Relative),
            PathBuf::from("a/b")
        );
        
        // Test real paths
        assert_eq!(
            manager.transform_path(Path::new("/a/b"), PathForm::Real),
            PathBuf::from("/tmp/source/a/b")
        );
        
        assert_eq!(
            manager.transform_path(Path::new("a/b"), PathForm::Real),
            PathBuf::from("/tmp/source/a/b")
        );
    }
    
    #[test]
    fn test_inode_path_mapping() {
        let source_dir = PathBuf::from("/tmp/source");
        let mut manager = PathManager::new(&source_dir);
        
        // Test root inode mapping
        assert_eq!(manager.get_path(ROOT_INODE), Some(PathBuf::from("/")));
        assert_eq!(manager.get_or_create_inode(Path::new("/")), ROOT_INODE);
        assert_eq!(manager.get_or_create_inode(Path::new("")), ROOT_INODE);
        
        // Test path to inode mapping
        let ino1 = manager.get_or_create_inode(Path::new("/a/b.txt"));
        assert!(ino1 > ROOT_INODE);
        assert_eq!(manager.get_path(ino1), Some(PathBuf::from("a/b.txt")));
        
        // Test getting existing inode
        let ino2 = manager.get_or_create_inode(Path::new("a/b.txt"));
        assert_eq!(ino1, ino2);
        
        // Test path update
        assert!(manager.update_path(Path::new("a/b.txt"), Path::new("c/d.txt")));
        assert_eq!(manager.get_path(ino1), Some(PathBuf::from("c/d.txt")));
        assert_eq!(manager.get_or_create_inode(Path::new("c/d.txt")), ino1);
        
        // Test path removal
        assert_eq!(manager.remove_path(Path::new("c/d.txt")), Some(ino1));
        assert_eq!(manager.get_path(ino1), None);
    }
    
    #[test]
    fn test_build_path() {
        let source_dir = PathBuf::from("/tmp/source");
        let mut manager = PathManager::new(&source_dir);
        
        // Test building paths from root
        assert_eq!(
            manager.build_path(ROOT_INODE, Path::new("file.txt")),
            Some(PathBuf::from("/file.txt"))
        );
        
        // Test building paths from non-root directory
        let dir_ino = manager.get_or_create_inode(Path::new("/dir1"));
        assert_eq!(
            manager.build_path(dir_ino, Path::new("file.txt")),
            Some(PathBuf::from("/dir1/file.txt"))
        );
        
        // Test with nested directories
        let nested_ino = manager.get_or_create_inode(Path::new("/dir1/subdir"));
        assert_eq!(
            manager.build_path(nested_ino, Path::new("file.txt")),
            Some(PathBuf::from("/dir1/subdir/file.txt"))
        );
        
        // Test with unknown inode
        assert_eq!(manager.build_path(999, Path::new("file.txt")), None);
    }
}