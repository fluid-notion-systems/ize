use diesel::prelude::*;
use diesel::sql_types::Text;
use diesel::QueryableByName;

// Define a struct for the last insert rowid query
#[derive(QueryableByName)]
struct LastId {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    #[allow(dead_code)]
    last_insert_rowid: i64,
}

// Import the modules from the library
use claris_fuse_lib::storage::diesel_sqlite::models::*;
use claris_fuse_lib::storage::diesel_sqlite::schema::*;

mod common;
use common::{cleanup, get_unix_timestamp, setup_test_db};

#[test]
fn test_create_and_retrieve_file_path() {
    let (mut conn, db_path) = setup_test_db();

    // Create a new file path entry
    let now = get_unix_timestamp();

    let new_file_path = NewDbFilePath {
        path: "/test/path/file.txt",
        created_at: now,
        last_modified: now,
    };

    // Insert the file path
    diesel::insert_into(file_paths::table)
        .values(&new_file_path)
        .execute(&mut conn)
        .expect("Failed to insert file path");

    // Get the last inserted ID
    let file_path_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
        .get_result::<LastId>(&mut conn)
        .expect("Failed to get last insert ID")
        .last_insert_rowid;

    // Retrieve the file path
    let retrieved_file_path = file_paths::table
        .find(file_path_id)
        .first::<DbFilePath>(&mut conn)
        .expect("Failed to retrieve file path");

    // Verify the retrieved file path
    assert_eq!(retrieved_file_path.path, "/test/path/file.txt");
    assert_eq!(retrieved_file_path.created_at, now);
    assert_eq!(retrieved_file_path.last_modified, now);

    // Clean up the test database
    cleanup(db_path);
}

#[test]
fn test_create_and_retrieve_version() {
    let (mut conn, db_path) = setup_test_db();

    // Create a file path first
    let now = get_unix_timestamp();

    let new_file_path = NewDbFilePath {
        path: "/test/path/file.txt",
        created_at: now,
        last_modified: now,
    };

    diesel::insert_into(file_paths::table)
        .values(&new_file_path)
        .execute(&mut conn)
        .expect("Failed to insert file path");

    // Get the last inserted ID
    let file_path_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
        .get_result::<LastId>(&mut conn)
        .expect("Failed to get last insert ID")
        .last_insert_rowid;

    // Create a new version
    let new_version = NewDbVersion {
        file_path_id,
        operation_type: "WRITE",
        timestamp: now,
        size: 1024,
        content_hash: Some("hash123"),
        description: Some("Test version"),
    };

    // Insert the version
    diesel::insert_into(versions::table)
        .values(&new_version)
        .execute(&mut conn)
        .expect("Failed to insert version");

    // Get the last inserted ID
    let version_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
        .get_result::<LastId>(&mut conn)
        .expect("Failed to get last insert ID")
        .last_insert_rowid;

    // Retrieve the version
    let retrieved_version = versions::table
        .find(version_id)
        .first::<DbVersion>(&mut conn)
        .expect("Failed to retrieve version");

    // Verify the retrieved version
    assert_eq!(retrieved_version.file_path_id, file_path_id);
    assert_eq!(retrieved_version.operation_type, "WRITE");
    assert_eq!(retrieved_version.timestamp, now);
    assert_eq!(retrieved_version.size, 1024);
    assert_eq!(retrieved_version.content_hash, Some("hash123".to_string()));
    assert_eq!(
        retrieved_version.description,
        Some("Test version".to_string())
    );

    // Clean up the test database
    cleanup(db_path);
}

#[test]
fn test_create_and_retrieve_content() {
    let (mut conn, db_path) = setup_test_db();

    // Create a file path first
    let now = get_unix_timestamp();

    let new_file_path = NewDbFilePath {
        path: "/test/path/file.txt",
        created_at: now,
        last_modified: now,
    };

    diesel::insert_into(file_paths::table)
        .values(&new_file_path)
        .execute(&mut conn)
        .expect("Failed to insert file path");

    // Get the last inserted ID
    let file_path_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
        .get_result::<LastId>(&mut conn)
        .expect("Failed to get last insert ID")
        .last_insert_rowid;

    // Create a new version
    let new_version = NewDbVersion {
        file_path_id,
        operation_type: "WRITE",
        timestamp: now,
        size: 5,
        content_hash: Some("hash123"),
        description: Some("Test version"),
    };

    diesel::insert_into(versions::table)
        .values(&new_version)
        .execute(&mut conn)
        .expect("Failed to insert version");

    // Get the last inserted ID
    let version_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
        .get_result::<LastId>(&mut conn)
        .expect("Failed to get last insert ID")
        .last_insert_rowid;

    // Create new content
    let test_data = b"hello";
    let new_content = NewDbContent {
        version_id,
        data: test_data,
    };

    // Insert the content
    diesel::insert_into(contents::table)
        .values(&new_content)
        .execute(&mut conn)
        .expect("Failed to insert content");

    // Retrieve the content
    let retrieved_content = contents::table
        .find(version_id)
        .first::<DbContent>(&mut conn)
        .expect("Failed to retrieve content");

    // Verify the retrieved content
    assert_eq!(retrieved_content.version_id, version_id);
    assert_eq!(retrieved_content.data, test_data);

    // Clean up the test database
    cleanup(db_path);
}

#[test]
fn test_create_and_search_fts() {
    let (mut conn, db_path) = setup_test_db();

    // Insert into the FTS table directly (normally this would be done via triggers)
    let description = "This is a searchable description with unique terms";

    diesel::sql_query(format!(
        "INSERT INTO versions_fts (id, description) VALUES (1, '{}')",
        description
    ))
    .execute(&mut conn)
    .expect("Failed to insert into FTS table");

    #[derive(QueryableByName, Debug)]
    struct FtsResult {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        id: i64,
        #[diesel(sql_type = Text)]
        description: String,
    }

    // Query the FTS table
    let results = diesel::sql_query(
        "SELECT id, description FROM versions_fts WHERE description MATCH 'searchable unique'",
    )
    .load::<FtsResult>(&mut conn)
    .expect("Failed to search FTS table");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].description, description);
    assert_eq!(results[0].id, 1);

    // Clean up the test database
    cleanup(db_path);
}

#[test]
fn test_timestamp_conversion() {
    use chrono::{TimeZone, Utc};

    // Test datetime to timestamp
    let dt = Utc.with_ymd_and_hms(2023, 10, 1, 12, 0, 0).unwrap();
    let timestamp = datetime_to_timestamp(dt);
    assert_eq!(timestamp, 1696161600); // Expected timestamp for 2023-10-01 12:00:00 UTC

    // Test timestamp to datetime
    let dt_converted = timestamp_to_datetime(1696161600);
    assert_eq!(dt_converted, dt);
}

#[test]
fn test_associations() {
    let (mut conn, db_path) = setup_test_db();

    // Create a file path
    let now = get_unix_timestamp();

    let new_file_path = NewDbFilePath {
        path: "/test/path/file.txt",
        created_at: now,
        last_modified: now,
    };

    diesel::insert_into(file_paths::table)
        .values(&new_file_path)
        .execute(&mut conn)
        .expect("Failed to insert file path");

    // Get the last inserted ID
    let file_path_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
        .get_result::<LastId>(&mut conn)
        .expect("Failed to get last insert ID")
        .last_insert_rowid;

    // Create multiple versions for the file
    for i in 1..=3 {
        let hash_str = format!("hash{}", i);
        let desc_str = format!("Version {}", i);
        let new_version = NewDbVersion {
            file_path_id,
            operation_type: "WRITE",
            timestamp: now + i,
            size: i * 100,
            content_hash: Some(&hash_str),
            description: Some(&desc_str),
        };

        diesel::insert_into(versions::table)
            .values(&new_version)
            .execute(&mut conn)
            .expect("Failed to insert version");
    }

    // Test the association between file_paths and versions
    let file_path = file_paths::table
        .find(file_path_id)
        .first::<DbFilePath>(&mut conn)
        .expect("Failed to retrieve file path");

    let versions = DbVersion::belonging_to(&file_path)
        .load::<DbVersion>(&mut conn)
        .expect("Failed to load versions");

    assert_eq!(versions.len(), 3);

    // Check if versions are correctly associated
    for version in versions {
        assert_eq!(version.file_path_id, file_path_id);
    }

    // Clean up the test database
    cleanup(db_path);
}

// Call cleanup at the end of each test to ensure database files are removed
