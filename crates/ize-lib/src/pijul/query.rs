//! Query operations for Pijul repositories
//!
//! This module provides a clean query interface for reading data from Pijul
//! repositories. It wraps the PijulBackend and provides structured query methods.
//!
//! ## Design
//!
//! The `PijulQuery` struct holds a reference to a `PijulBackend` and provides
//! query-only operations. This separates read operations from write operations,
//! making the API clearer and easier to reason about.
//!
//! ## Usage
//!
//! ```ignore
//! let backend = PijulBackend::open(&pijul_dir, &working_dir)?;
//! let query = PijulQuery::new(&backend);
//!
//! // List all changes with metadata
//! for change in query.list_changes_detailed()? {
//!     println!("{}: {}", change.hash_short(), change.message);
//! }
//! ```

use chrono::{DateTime, TimeZone, Utc};
use libpijul::changestore::ChangeStore;
use libpijul::pristine::Hash;
use libpijul::{Base32, TxnT, TxnTExt};
use std::path::PathBuf;

use super::backend::{PijulBackend, PijulError};

/// Detailed information about a change/commit
#[derive(Debug, Clone)]
pub struct ChangeInfo {
    /// The change hash
    pub hash: Hash,
    /// Commit message
    pub message: String,
    /// Optional longer description
    pub description: Option<String>,
    /// When the change was created
    pub timestamp: DateTime<Utc>,
    /// List of author names
    pub authors: Vec<String>,
    /// Number of files affected by this change
    pub files_changed: usize,
}

impl ChangeInfo {
    /// Get a short version of the hash (first 7 characters)
    pub fn hash_short(&self) -> String {
        let full = self.hash.to_base32();
        if full.len() > 7 {
            full[..7].to_string()
        } else {
            full
        }
    }

    /// Get the full hash as a base32 string
    pub fn hash_full(&self) -> String {
        self.hash.to_base32()
    }

    /// Get a human-readable relative timestamp (e.g., "2 min ago")
    pub fn timestamp_relative(&self) -> String {
        let now = Utc::now();
        let duration = now.signed_duration_since(self.timestamp);

        if duration.num_seconds() < 60 {
            "just now".to_string()
        } else if duration.num_minutes() < 60 {
            let mins = duration.num_minutes();
            format!("{} min ago", mins)
        } else if duration.num_hours() < 24 {
            let hours = duration.num_hours();
            format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
        } else if duration.num_days() < 30 {
            let days = duration.num_days();
            format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
        } else {
            self.timestamp.format("%Y-%m-%d").to_string()
        }
    }

    /// Get the first author name, or "Unknown" if none
    pub fn primary_author(&self) -> &str {
        self.authors
            .first()
            .map(|s| s.as_str())
            .unwrap_or("Unknown")
    }
}

/// Information about a file in the repository
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// Relative path from repository root
    pub path: PathBuf,
    /// Whether this is a directory
    pub is_directory: bool,
}

/// Query interface for Pijul repositories
///
/// Provides read-only query operations over a PijulBackend.
pub struct PijulQuery<'a> {
    backend: &'a PijulBackend,
}

impl<'a> PijulQuery<'a> {
    /// Create a new query interface for the given backend
    pub fn new(backend: &'a PijulBackend) -> Self {
        Self { backend }
    }

    // === Channel Queries ===

    /// List all channels in the repository
    pub fn list_channels(&self) -> Result<Vec<String>, PijulError> {
        self.backend.list_channels()
    }

    /// Get the current channel name
    pub fn current_channel(&self) -> &str {
        self.backend.current_channel()
    }

    // === Change Queries ===

    /// List all change hashes in the current channel (chronological order)
    pub fn list_change_hashes(&self) -> Result<Vec<Hash>, PijulError> {
        self.backend.list_changes()
    }

    /// List all changes with detailed metadata
    ///
    /// Returns changes in chronological order (oldest first).
    pub fn list_changes_detailed(&self) -> Result<Vec<ChangeInfo>, PijulError> {
        let txn = self.backend.txn_begin()?;
        let channel = txn
            .load_channel(self.backend.current_channel())
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?
            .ok_or_else(|| {
                PijulError::ChannelNotFound(self.backend.current_channel().to_string())
            })?;

        let channel_ref = channel.read();
        let change_store = self.backend.get_change_store();
        let mut changes = Vec::new();

        // Iterate through the channel's log
        for entry in txn
            .log(&*channel_ref, 0)
            .map_err(|e| PijulError::Transaction(format!("{:?}", e)))?
        {
            let (_, (hash_ref, _)) =
                entry.map_err(|e| PijulError::Transaction(format!("{:?}", e)))?;
            let hash: Hash = (*hash_ref).into();

            // Get the change header for metadata
            match change_store.get_header(&hash) {
                Ok(header) => {
                    // Convert timestamp to DateTime
                    // jiff::Timestamp can be converted to seconds via as_second()
                    let timestamp = Utc
                        .timestamp_opt(header.timestamp.as_second(), 0)
                        .single()
                        .unwrap_or_else(Utc::now);

                    // Extract author names from the Author struct
                    let authors: Vec<String> = header
                        .authors
                        .iter()
                        .filter_map(|author| {
                            // Author is a BTreeMap wrapper, try to get "name" key
                            author.0.get("name").cloned()
                        })
                        .collect();

                    // TODO: Get actual file count from touched_files
                    let files_changed = 0;

                    changes.push(ChangeInfo {
                        hash,
                        message: header.message,
                        description: header.description,
                        timestamp,
                        authors,
                        files_changed,
                    });
                }
                Err(_) => {
                    // If we can't get the header, create a minimal entry
                    changes.push(ChangeInfo {
                        hash,
                        message: "(unable to read change)".to_string(),
                        description: None,
                        timestamp: Utc::now(),
                        authors: vec![],
                        files_changed: 0,
                    });
                }
            }
        }

        Ok(changes)
    }

    /// List changes in reverse chronological order (newest first)
    pub fn list_changes_detailed_reverse(&self) -> Result<Vec<ChangeInfo>, PijulError> {
        let mut changes = self.list_changes_detailed()?;
        changes.reverse();
        Ok(changes)
    }

    /// Get detailed information about a specific change
    pub fn get_change_info(&self, hash: &Hash) -> Result<ChangeInfo, PijulError> {
        let change_store = self.backend.get_change_store();
        let header = change_store
            .get_header(hash)
            .map_err(|e| PijulError::ChangeStore(format!("{:?}", e)))?;

        let timestamp = Utc
            .timestamp_opt(header.timestamp.as_second(), 0)
            .single()
            .unwrap_or_else(Utc::now);

        let authors: Vec<String> = header
            .authors
            .iter()
            .filter_map(|author| author.0.get("name").cloned())
            .collect();

        Ok(ChangeInfo {
            hash: *hash,
            message: header.message,
            description: header.description,
            timestamp,
            authors,
            files_changed: 0, // TODO: implement
        })
    }

    /// Get the number of changes in the current channel
    pub fn change_count(&self) -> Result<usize, PijulError> {
        Ok(self.backend.list_changes()?.len())
    }

    // === File Queries ===

    /// Check if a file exists in the current channel
    pub fn file_exists(&self, path: &str) -> Result<bool, PijulError> {
        self.backend.file_exists(path)
    }

    /// Get the content of a file
    pub fn get_file_content(&self, path: &str) -> Result<Vec<u8>, PijulError> {
        self.backend.get_file_content(path)
    }

    /// Get the content of a file as a string (assuming UTF-8)
    pub fn get_file_content_string(&self, path: &str) -> Result<String, PijulError> {
        let content = self.backend.get_file_content(path)?;
        String::from_utf8(content).map_err(|e| PijulError::Diff(format!("Invalid UTF-8: {}", e)))
    }

    /// List all files in the current channel
    ///
    /// Note: This is currently a stub and returns an empty list.
    /// TODO: Implement proper file listing from pristine.
    pub fn list_files(&self) -> Result<Vec<String>, PijulError> {
        self.backend.list_files()
    }

    // === Utility Methods ===

    /// Get a reference to the underlying backend
    pub fn backend(&self) -> &PijulBackend {
        self.backend
    }

    /// Parse a hash from a base32 string
    pub fn parse_hash(hash_str: &str) -> Result<Hash, PijulError> {
        Hash::from_base32(hash_str.as_bytes())
            .ok_or_else(|| PijulError::Transaction(format!("Invalid hash: {}", hash_str)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_backend() -> (TempDir, PijulBackend) {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");

        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();
        (temp, backend)
    }

    #[test]
    fn test_query_new() {
        let (_temp, backend) = setup_test_backend();
        let query = PijulQuery::new(&backend);
        assert_eq!(query.current_channel(), "main");
    }

    #[test]
    fn test_list_channels() {
        let (_temp, backend) = setup_test_backend();
        let query = PijulQuery::new(&backend);

        let channels = query.list_channels().unwrap();
        assert!(channels.contains(&"main".to_string()));
    }

    #[test]
    fn test_list_changes_empty() {
        let (_temp, backend) = setup_test_backend();
        let query = PijulQuery::new(&backend);

        let changes = query.list_changes_detailed().unwrap();
        assert!(changes.is_empty());
    }

    #[test]
    fn test_change_count_empty() {
        let (_temp, backend) = setup_test_backend();
        let query = PijulQuery::new(&backend);

        assert_eq!(query.change_count().unwrap(), 0);
    }

    #[test]
    fn test_file_not_exists() {
        let (_temp, backend) = setup_test_backend();
        let query = PijulQuery::new(&backend);

        assert!(!query.file_exists("nonexistent.txt").unwrap());
    }

    #[test]
    fn test_change_info_hash_short() {
        let info = ChangeInfo {
            hash: Hash::None, // Placeholder
            message: "Test".to_string(),
            description: None,
            timestamp: Utc::now(),
            authors: vec!["Test Author".to_string()],
            files_changed: 1,
        };

        // Just verify it doesn't panic
        let _short = info.hash_short();
    }

    #[test]
    fn test_change_info_timestamp_relative() {
        let info = ChangeInfo {
            hash: Hash::None,
            message: "Test".to_string(),
            description: None,
            timestamp: Utc::now(),
            authors: vec![],
            files_changed: 0,
        };

        assert_eq!(info.timestamp_relative(), "just now");
    }

    #[test]
    fn test_change_info_primary_author() {
        let info_with_author = ChangeInfo {
            hash: Hash::None,
            message: "Test".to_string(),
            description: None,
            timestamp: Utc::now(),
            authors: vec!["Alice".to_string(), "Bob".to_string()],
            files_changed: 0,
        };
        assert_eq!(info_with_author.primary_author(), "Alice");

        let info_without_author = ChangeInfo {
            hash: Hash::None,
            message: "Test".to_string(),
            description: None,
            timestamp: Utc::now(),
            authors: vec![],
            files_changed: 0,
        };
        assert_eq!(info_without_author.primary_author(), "Unknown");
    }
}
