// use diesel::prelude::*;
// use diesel::sqlite::SqliteConnection;
// use diesel::QueryableByName;
// use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
// use std::path::PathBuf;
// use std::time::{SystemTime, UNIX_EPOCH};
// use tempfile::tempdir;

// // Define a struct for the last insert rowid query
// #[derive(QueryableByName)]
// struct LastId {
//     #[diesel(sql_type = diesel::sql_types::BigInt)]
//     #[allow(dead_code)]
//     last_insert_rowid: i64,
// }

// // Define the embedded migrations
// pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

// // Setup a test database
// fn setup_test_db() -> (SqliteConnection, PathBuf) {
//     // Create a temporary directory for our test database
//     let temp_dir = tempdir().expect("Failed to create temporary directory");
//     let db_path = temp_dir.path().join("test.db");

//     // Create a new SQLite connection
//     let mut connection = SqliteConnection::establish(db_path.to_str().unwrap())
//         .expect("Failed to create SQLite connection");

//     // Run migrations to set up the schema
//     connection
//         .run_pending_migrations(MIGRATIONS)
//         .expect("Failed to run migrations");

//     (connection, db_path)
// }

// // Get current timestamp
// fn get_unix_timestamp() -> i64 {
//     SystemTime::now()
//         .duration_since(UNIX_EPOCH)
//         .unwrap()
//         .as_secs() as i64
// }

// // Cleanup utility
// fn cleanup(db_path: PathBuf) {
//     if db_path.exists() {
//         std::fs::remove_file(db_path).expect("Failed to remove test database file");
//     }
// }

// // Define the table! macros for our tests
// table! {
//     file_paths (id) {
//         id -> BigInt,
//         path -> Text,
//         created_at -> BigInt,
//         last_modified -> BigInt,
//     }
// }

// table! {
//     versions (id) {
//         id -> BigInt,
//         file_path_id -> BigInt,
//         operation_type -> Text,
//         timestamp -> BigInt,
//         size -> BigInt,
//         content_hash -> Nullable<Text>,
//         description -> Nullable<Text>,
//     }
// }

// table! {
//     contents (version_id) {
//         version_id -> BigInt,
//         data -> Binary,
//     }
// }

// joinable!(versions -> file_paths (file_path_id));
// joinable!(contents -> versions (version_id));

// allow_tables_to_appear_in_same_query!(file_paths, versions, contents,);

// // Define the model structs
// #[derive(Queryable, Identifiable, Debug, Clone)]
// #[diesel(table_name = file_paths)]
// struct DbFilePath {
//     pub id: i64,
//     pub path: String,
//     pub created_at: i64,
//     pub last_modified: i64,
// }

// #[derive(Insertable, Debug)]
// #[diesel(table_name = file_paths)]
// struct NewDbFilePath<'a> {
//     pub path: &'a str,
//     pub created_at: i64,
//     pub last_modified: i64,
// }

// #[derive(Queryable, Identifiable, Associations, Debug, Clone)]
// #[diesel(table_name = versions)]
// #[diesel(belongs_to(DbFilePath, foreign_key = file_path_id))]
// struct DbVersion {
//     pub id: i64,
//     pub file_path_id: i64,
//     pub operation_type: String,
//     pub timestamp: i64,
//     pub size: i64,
//     pub content_hash: Option<String>,
//     pub description: Option<String>,
// }

// #[derive(Insertable, Debug)]
// #[diesel(table_name = versions)]
// struct NewDbVersion<'a> {
//     pub file_path_id: i64,
//     pub operation_type: &'a str,
//     pub timestamp: i64,
//     pub size: i64,
//     pub content_hash: Option<&'a str>,
//     pub description: Option<&'a str>,
// }

// #[derive(Queryable, Identifiable, Associations, Debug)]
// #[diesel(table_name = contents)]
// #[diesel(primary_key(version_id))]
// #[diesel(belongs_to(DbVersion, foreign_key = version_id))]
// struct DbContent {
//     pub version_id: i64,
//     pub data: Vec<u8>,
// }

// #[derive(Insertable, Debug)]
// #[diesel(table_name = contents)]
// struct NewDbContent<'a> {
//     pub version_id: i64,
//     pub data: &'a [u8],
// }

// #[test]
// fn test_create_and_retrieve_file_path() {
//     let (mut conn, db_path) = setup_test_db();

//     // Create a new file path entry
//     let now = get_unix_timestamp();

//     let new_file_path = NewDbFilePath {
//         path: "/test/path/file.txt",
//         created_at: now,
//         last_modified: now,
//     };

//     // Insert the file path
//     diesel::insert_into(file_paths::table)
//         .values(&new_file_path)
//         .execute(&mut conn)
//         .expect("Failed to insert file path");

//     // Get the last inserted ID
//     let file_path_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
//         .get_result::<LastId>(&mut conn)
//         .expect("Failed to get last insert ID")
//         .last_insert_rowid;

//     // Retrieve the file path
//     let retrieved_file_path = file_paths::table
//         .find(file_path_id)
//         .first::<DbFilePath>(&mut conn)
//         .expect("Failed to retrieve file path");

//     // Verify the retrieved file path
//     assert_eq!(retrieved_file_path.path, "/test/path/file.txt");
//     assert_eq!(retrieved_file_path.created_at, now);
//     assert_eq!(retrieved_file_path.last_modified, now);

//     // Clean up the test database
//     cleanup(db_path);
// }

// #[test]
// fn test_create_and_retrieve_version() {
//     let (mut conn, db_path) = setup_test_db();

//     // Create a file path first
//     let now = get_unix_timestamp();

//     let new_file_path = NewDbFilePath {
//         path: "/test/path/file.txt",
//         created_at: now,
//         last_modified: now,
//     };

//     diesel::insert_into(file_paths::table)
//         .values(&new_file_path)
//         .execute(&mut conn)
//         .expect("Failed to insert file path");

//     // Get the last inserted ID
//     let file_path_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
//         .get_result::<LastId>(&mut conn)
//         .expect("Failed to get last insert ID")
//         .last_insert_rowid;

//     // Create a new version
//     let new_version = NewDbVersion {
//         file_path_id,
//         operation_type: "WRITE",
//         timestamp: now,
//         size: 1024,
//         content_hash: Some("hash123"),
//         description: Some("Test version"),
//     };

//     // Insert the version
//     diesel::insert_into(versions::table)
//         .values(&new_version)
//         .execute(&mut conn)
//         .expect("Failed to insert version");

//     // Get the last inserted ID
//     let version_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
//         .get_result::<LastId>(&mut conn)
//         .expect("Failed to get last insert ID")
//         .last_insert_rowid;

//     // Retrieve the version
//     let retrieved_version = versions::table
//         .find(version_id)
//         .first::<DbVersion>(&mut conn)
//         .expect("Failed to retrieve version");

//     // Verify the retrieved version
//     assert_eq!(retrieved_version.file_path_id, file_path_id);
//     assert_eq!(retrieved_version.operation_type, "WRITE");
//     assert_eq!(retrieved_version.timestamp, now);
//     assert_eq!(retrieved_version.size, 1024);
//     assert_eq!(retrieved_version.content_hash, Some("hash123".to_string()));
//     assert_eq!(
//         retrieved_version.description,
//         Some("Test version".to_string())
//     );

//     // Clean up the test database
//     cleanup(db_path);
// }

// #[test]
// fn test_create_and_retrieve_content() {
//     let (mut conn, db_path) = setup_test_db();

//     // Create a file path first
//     let now = get_unix_timestamp();

//     let new_file_path = NewDbFilePath {
//         path: "/test/path/file.txt",
//         created_at: now,
//         last_modified: now,
//     };

//     diesel::insert_into(file_paths::table)
//         .values(&new_file_path)
//         .execute(&mut conn)
//         .expect("Failed to insert file path");

//     // Get the last inserted ID
//     let file_path_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
//         .get_result::<LastId>(&mut conn)
//         .expect("Failed to get last insert ID")
//         .last_insert_rowid;

//     // Create a new version
//     let new_version = NewDbVersion {
//         file_path_id,
//         operation_type: "WRITE",
//         timestamp: now,
//         size: 5,
//         content_hash: Some("hash123"),
//         description: Some("Test version"),
//     };

//     diesel::insert_into(versions::table)
//         .values(&new_version)
//         .execute(&mut conn)
//         .expect("Failed to insert version");

//     // Get the last inserted ID
//     let version_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
//         .get_result::<LastId>(&mut conn)
//         .expect("Failed to get last insert ID")
//         .last_insert_rowid;

//     // Create new content
//     let test_data = b"hello";
//     let new_content = NewDbContent {
//         version_id,
//         data: test_data,
//     };

//     // Insert the content
//     diesel::insert_into(contents::table)
//         .values(&new_content)
//         .execute(&mut conn)
//         .expect("Failed to insert content");

//     // Retrieve the content
//     let retrieved_content = contents::table
//         .find(version_id)
//         .first::<DbContent>(&mut conn)
//         .expect("Failed to retrieve content");

//     // Verify the retrieved content
//     assert_eq!(retrieved_content.version_id, version_id);
//     assert_eq!(retrieved_content.data, test_data);

//     // Clean up the test database
//     cleanup(db_path);
// }

// #[test]
// fn test_associations() {
//     let (mut conn, db_path) = setup_test_db();

//     // Create a file path
//     let now = get_unix_timestamp();

//     let new_file_path = NewDbFilePath {
//         path: "/test/path/file.txt",
//         created_at: now,
//         last_modified: now,
//     };

//     diesel::insert_into(file_paths::table)
//         .values(&new_file_path)
//         .execute(&mut conn)
//         .expect("Failed to insert file path");

//     // Get the last inserted ID
//     let file_path_id = diesel::sql_query("SELECT last_insert_rowid() as last_insert_rowid")
//         .get_result::<LastId>(&mut conn)
//         .expect("Failed to get last insert ID")
//         .last_insert_rowid;

//     // Create multiple versions for the file
//     for i in 1..=3 {
//         let hash_desc = format!("hash{}", i);
//         let description = format!("Version {}", i);
//         let new_version = NewDbVersion {
//             file_path_id,
//             operation_type: "WRITE",
//             timestamp: now + i,
//             size: i * 100,
//             content_hash: Some(&hash_desc),
//             description: Some(&description),
//         };

//         diesel::insert_into(versions::table)
//             .values(&new_version)
//             .execute(&mut conn)
//             .expect("Failed to insert version");
//     }

//     // Test the association between file_paths and versions
//     let file_path = file_paths::table
//         .find(file_path_id)
//         .first::<DbFilePath>(&mut conn)
//         .expect("Failed to retrieve file path");

//     let versions = DbVersion::belonging_to(&file_path)
//         .load::<DbVersion>(&mut conn)
//         .expect("Failed to load versions");

//     assert_eq!(versions.len(), 3);

//     // Check if versions are correctly associated
//     for version in versions {
//         assert_eq!(version.file_path_id, file_path_id);
//     }

//     // Clean up the test database
//     cleanup(db_path);
// }
