use super::schema::*;
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use std::path::PathBuf;

/// Entity type indicates whether a path is a file or directory
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityType {
    File,
    Directory,
}

impl EntityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntityType::File => "file",
            EntityType::Directory => "directory",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "file" => Some(EntityType::File),
            "directory" => Some(EntityType::Directory),
            _ => None,
        }
    }
}

/// Database model that directly maps to the paths table
#[derive(Queryable, Identifiable, Debug, Clone)]
#[diesel(table_name = paths)]
pub struct DbPath {
    pub id: i64,
    pub path: String,
    pub entity_type: String, // "file" or "directory"
    pub created_at: i64,
    pub last_modified: i64,
}

impl DbPath {
    /// Get the entity type as an enum
    pub fn get_entity_type(&self) -> Option<EntityType> {
        EntityType::from_str(&self.entity_type)
    }
    
    /// Check if this path is a file
    pub fn is_file(&self) -> bool {
        self.entity_type == "file"
    }
    
    /// Check if this path is a directory
    pub fn is_directory(&self) -> bool {
        self.entity_type == "directory"
    }
    
    /// Get the path as a PathBuf
    pub fn as_path_buf(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }
}

/// Insertable struct for creating new paths records
#[derive(Insertable, Debug)]
#[diesel(table_name = paths)]
pub struct NewDbPath<'a> {
    pub path: &'a str,
    pub entity_type: &'a str,
    pub created_at: i64,
    pub last_modified: i64,
}

/// Database model that maps to the metadata table
#[derive(Queryable, Identifiable, Associations, Debug, Clone)]
#[diesel(table_name = metadata)]
#[diesel(primary_key(path_id))]
#[diesel(belongs_to(DbPath, foreign_key = path_id))]
pub struct DbMetadata {
    pub path_id: i64,
    pub mode: i32,
    pub uid: i32,
    pub gid: i32,
    pub atime: i64,
    pub mtime: i64,
    pub ctime: i64,
}

/// Insertable struct for creating new metadata records
#[derive(Insertable, Debug)]
#[diesel(table_name = metadata)]
pub struct NewDbMetadata {
    pub path_id: i64,
    pub mode: i32,
    pub uid: i32,
    pub gid: i32,
    pub atime: i64,
    pub mtime: i64,
    pub ctime: i64,
}

impl NewDbMetadata {
    /// Create a new metadata record with defaults
    pub fn new(path_id: i64, timestamp: i64) -> Self {
        Self {
            path_id,
            mode: 0o644,
            uid: 1000,
            gid: 1000,
            atime: timestamp,
            mtime: timestamp,
            ctime: timestamp,
        }
    }
    
    /// Create a new directory metadata record with defaults
    pub fn new_directory(path_id: i64, timestamp: i64) -> Self {
        Self {
            path_id,
            mode: 0o755,
            uid: 1000,
            gid: 1000,
            atime: timestamp,
            mtime: timestamp,
            ctime: timestamp,
        }
    }
}

/// Database model that directly maps to the versions table
#[derive(Queryable, Identifiable, Associations, Debug, Clone)]
#[diesel(table_name = versions)]
#[diesel(belongs_to(DbPath, foreign_key = file_path_id))]
pub struct DbVersion {
    pub id: i64,
    pub file_path_id: i64,
    pub operation_type: String,
    pub timestamp: i64,
    pub size: i64,
    pub content_hash: Option<String>,
    pub description: Option<String>,
}

/// Insertable struct for creating new versions records
#[derive(Insertable, Debug)]
#[diesel(table_name = versions)]
pub struct NewDbVersion<'a> {
    pub file_path_id: i64,
    pub operation_type: &'a str,
    pub timestamp: i64,
    pub size: i64,
    pub content_hash: Option<&'a str>,
    pub description: Option<&'a str>,
}

/// Database model that directly maps to the contents table
#[derive(Queryable, Identifiable, Associations, Debug)]
#[diesel(table_name = contents)]
#[diesel(primary_key(version_id))]
#[diesel(belongs_to(DbVersion, foreign_key = version_id))]
pub struct DbContent {
    pub version_id: i64,
    pub data: Vec<u8>,
}

/// Insertable struct for creating new contents records
#[derive(Insertable, Debug)]
#[diesel(table_name = contents)]
pub struct NewDbContent<'a> {
    pub version_id: i64,
    pub data: &'a [u8],
}

/// Database model that maps to the FTS (Full-Text Search) table
#[derive(Queryable, Identifiable, Debug)]
#[diesel(table_name = versions_fts)]
#[diesel(primary_key(id))]
pub struct DbVersionFts {
    pub id: i64,
    pub description: String,
}

// Utility functions for converting between DB timestamps and DateTime<Utc>

/// Convert a UNIX timestamp to DateTime<Utc>
pub fn timestamp_to_datetime(timestamp: i64) -> DateTime<Utc> {
    let naive = DateTime::from_timestamp(timestamp, 0)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap())
        .naive_utc();
    DateTime::from_naive_utc_and_offset(naive, Utc)
}

/// Convert a DateTime<Utc> to UNIX timestamp
pub fn datetime_to_timestamp(datetime: DateTime<Utc>) -> i64 {
    datetime.timestamp()
}