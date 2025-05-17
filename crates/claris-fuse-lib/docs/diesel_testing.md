# Testing Diesel Models in claris-fuse-lib

This document provides guidance on testing the Diesel models used for SQLite storage in the `claris-fuse-lib` project.

## Overview

The `diesel_sqlite` module uses Diesel ORM to interact with SQLite databases. This module contains:

1. Model definitions (`models.rs`)
2. Schema definitions (`schema.rs`)
3. Implementation of the storage backend (`mod.rs`)

Testing should cover:
- Model structure and relationships
- CRUD operations
- Database migrations
- Integration with the storage traits

## Test Structure

### Unit Tests for Models

These tests verify that the model definitions compile correctly and that their relationships are properly defined.

```rust
#[test]
fn test_diesel_model_structure() {
    // Create instances of the models
    let file_path = DbFilePath {
        id: 1,
        path: "/test/file.txt".to_string(),
        created_at: 1234567890,
        last_modified: 1234567890,
    };
    
    let version = DbVersion {
        id: 1,
        file_path_id: 1,
        operation_type: "WRITE".to_string(),
        timestamp: 1234567890,
        size: 1024,
        content_hash: Some("hash123".to_string()),
        description: Some("Test version".to_string()),
    };
    
    let content = DbContent {
        version_id: 1,
        data: b"test data".to_vec(),
    };
    
    // Verify relationships
    assert_eq!(file_path.id, 1);
    assert_eq!(version.file_path_id, file_path.id);
    assert_eq!(content.version_id, version.id);
}
```

### Database Operation Tests

These tests verify interactions with the database.

```rust
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel_migrations::MigrationHarness;
use tempfile::tempdir;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

fn setup_test_db() -> (SqliteConnection, PathBuf) {
    // Create a temporary directory for our test database
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    
    // Create a new SQLite connection
    let mut connection = SqliteConnection::establish(db_path.to_str().unwrap())
        .expect("Failed to create SQLite connection");
    
    // Run migrations to set up the schema
    connection.run_pending_migrations(MIGRATIONS)
        .expect("Failed to run migrations");
    
    (connection, db_path)
}

#[test]
fn test_create_and_retrieve_file_path() {
    let (mut conn, _db_path) = setup_test_db();
    
    // Create a test file path
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    
    let new_file_path = NewDbFilePath {
        path: "/test/path/file.txt",
        created_at: now,
        last_modified: now,
    };
    
    // Insert into database
    diesel::insert_into(file_paths::table)
        .values(&new_file_path)
        .execute(&mut conn)
        .expect("Failed to insert file path");
    
    // Get the last inserted ID (since SQLite doesn't support returning clauses)
    let file_path_id = diesel::select(sql::<diesel::sql_types::BigInt>("last_insert_rowid()"))
        .get_result::<i64>(&mut conn)
        .expect("Failed to get last inserted ID");
    
    // Retrieve and verify
    let retrieved_file_path = file_paths::table
        .find(file_path_id)
        .first::<DbFilePath>(&mut conn)
        .expect("Failed to retrieve file path");
    
    assert_eq!(retrieved_file_path.path, "/test/path/file.txt");
}
```

### Integration Tests for Storage

These tests verify the complete implementation of the storage backend.

```rust
#[tokio::test]
async fn test_storage_backend() {
    // Create a temporary database
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    
    // Create and initialize the storage backend
    let storage = DieselSqliteStorage::new(db_path.to_str().unwrap().to_string());
    storage.init().await.expect("Failed to initialize storage");
    
    // Test a storage operation
    let content = b"Hello, world!".to_vec();
    let path = "/test/file.txt".to_string();
    let description = Some("Initial version".to_string());
    
    // Record a version
    let version_id = storage.record_version(
        &path,
        OperationType::Write,
        SystemTime::now(),
        &content,
        description.clone(),
    ).await.expect("Failed to record version");
    
    // Retrieve and verify
    let version = storage.get_version(&path, version_id).await
        .expect("Failed to get version")
        .expect("Version not found");
    
    assert_eq!(version.path, path);
    assert_eq!(version.description, description);
    
    // Clean up
    storage.close().await.expect("Failed to close storage");
}
```

## Best Practices

1. **Use Temporary Databases**: Always use temporary files/directories for test databases to avoid interference with real data.

2. **Isolate Tests**: Use the `serial_test` crate with the `#[serial]` attribute when tests might interfere with each other.

3. **Test Edge Cases**: Include tests for empty content, large files, special characters in paths, etc.

4. **Test Transactions**: Verify that operations are properly atomic and can be rolled back if necessary.

5. **Clean Up**: Always close connections and delete temporary files after tests complete.

6. **Mock Time**: For time-dependent tests, consider mocking the system time to make tests deterministic.

## Running Tests

Run all tests with:

```bash
cargo test
```

Run specific tests with:

```bash
cargo test diesel_sqlite
```

To see more detailed output, including print statements:

```bash
cargo test -- --nocapture
```

## Troubleshooting

1. **SQLite Version Issues**: If you encounter problems with SQLite functions, make sure you're using a compatible version of `diesel` and `libsqlite3-sys`.

2. **Migration Failures**: Verify that your migrations directory is in the correct path and that migrations can be properly embedded.

3. **Connection Pooling**: For tests using connection pools, make sure pools are properly closed after tests to avoid resource leaks.