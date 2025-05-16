use diesel::prelude::*;
use diesel::dsl::*;
use diesel::sqlite::SqliteConnection;
use std::time::{SystemTime, UNIX_EPOCH};

// Define the table! macros for our tests
table! {
    file_paths (id) {
        id -> BigInt,
        path -> Text,
        created_at -> BigInt,
        last_modified -> BigInt,
    }
}

table! {
    versions (id) {
        id -> BigInt,
        file_path_id -> BigInt,
        operation_type -> Text,
        timestamp -> BigInt,
        size -> BigInt,
        content_hash -> Nullable<Text>,
        description -> Nullable<Text>,
    }
}

table! {
    contents (version_id) {
        version_id -> BigInt,
        data -> Binary,
    }
}

joinable!(versions -> file_paths (file_path_id));
joinable!(contents -> versions (version_id));

allow_tables_to_appear_in_same_query!(
    file_paths,
    versions,
    contents,
);

// Define the model structs
#[derive(Queryable, Identifiable, Debug, Clone)]
#[diesel(table_name = file_paths)]
struct DbFilePath {
    pub id: i64,
    pub path: String,
    pub created_at: i64,
    pub last_modified: i64,
}

#[derive(Insertable, Debug)]
#[diesel(table_name = file_paths)]
struct NewDbFilePath<'a> {
    pub path: &'a str,
    pub created_at: i64,
    pub last_modified: i64,
}

#[derive(Queryable, Identifiable, Associations, Debug, Clone)]
#[diesel(table_name = versions)]
#[diesel(belongs_to(DbFilePath, foreign_key = file_path_id))]
struct DbVersion {
    pub id: i64,
    pub file_path_id: i64,
    pub operation_type: String,
    pub timestamp: i64,
    pub size: i64,
    pub content_hash: Option<String>,
    pub description: Option<String>,
}

#[derive(Insertable, Debug)]
#[diesel(table_name = versions)]
struct NewDbVersion<'a> {
    pub file_path_id: i64,
    pub operation_type: &'a str,
    pub timestamp: i64,
    pub size: i64,
    pub content_hash: Option<&'a str>,
    pub description: Option<&'a str>,
}

#[derive(Queryable, Identifiable, Associations, Debug)]
#[diesel(table_name = contents)]
#[diesel(primary_key(version_id))]
#[diesel(belongs_to(DbVersion, foreign_key = version_id))]
struct DbContent {
    pub version_id: i64,
    pub data: Vec<u8>,
}

#[derive(Insertable, Debug)]
#[diesel(table_name = contents)]
struct NewDbContent<'a> {
    pub version_id: i64,
    pub data: &'a [u8],
}

// Utility function to get current timestamp
fn get_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// Operation type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationType {
    Create,
    Write,
    Rename,
    Delete,
}

fn operation_type_to_string(op_type: &OperationType) -> String {
    match op_type {
        OperationType::Create => "CREATE".to_string(),
        OperationType::Write => "WRITE".to_string(),
        OperationType::Rename => "RENAME".to_string(),
        OperationType::Delete => "DELETE".to_string(),
    }
}

fn string_to_operation_type(op_str: &str) -> OperationType {
    match op_str {
        "CREATE" => OperationType::Create,
        "WRITE" => OperationType::Write,
        "RENAME" => OperationType::Rename,
        "DELETE" => OperationType::Delete,
        _ => OperationType::Write, // Default if unknown
    }
}

// This test simply verifies that our model structs compile properly
fn test_diesel_models() {
    // Create test instances of our models
    let timestamp = get_unix_timestamp();
    
    // File path
    let file_path = DbFilePath {
        id: 1,
        path: "/test/file.txt".to_string(),
        created_at: timestamp,
        last_modified: timestamp,
    };
    
    // New file path for insertion
    let new_file_path = NewDbFilePath {
        path: "/test/file.txt",
        created_at: timestamp,
        last_modified: timestamp,
    };
    
    // Version
    let version = DbVersion {
        id: 1,
        file_path_id: 1,
        operation_type: "WRITE".to_string(),
        timestamp,
        size: 1024,
        content_hash: Some("abc123".to_string()),
        description: Some("Test version".to_string()),
    };
    
    // New version for insertion
    let new_version = NewDbVersion {
        file_path_id: 1,
        operation_type: "WRITE",
        timestamp,
        size: 1024,
        content_hash: Some("abc123"),
        description: Some("Test version"),
    };
    
    // Content
    let content = DbContent {
        version_id: 1,
        data: b"test content".to_vec(),
    };
    
    // New content for insertion
    let new_content = NewDbContent {
        version_id: 1,
        data: b"test content",
    };
    
    // Verify basic properties
    assert_eq!(file_path.path, "/test/file.txt");
    assert_eq!(file_path.created_at, timestamp);
    
    assert_eq!(version.file_path_id, file_path.id);
    assert_eq!(version.operation_type, "WRITE");
    
    assert_eq!(content.version_id, version.id);
    assert_eq!(content.data, b"test content");
    
    // Test model relationships
    assert_eq!(new_file_path.path, "/test/file.txt");
    assert_eq!(new_version.file_path_id, file_path.id);
    assert_eq!(new_content.version_id, version.id);
    
    println!("✅ Diesel models test passed!");
}

fn main() {
    println!("Running Diesel model tests...");
    
    // Test that our models compile properly
    test_diesel_models();
    
    println!("✅ All tests passed successfully!");
}