// use diesel::prelude::*;
// use diesel::sqlite::SqliteConnection;
// use diesel_migrations::{EmbeddedMigrations, MigrationHarness};
// use std::fs;
// use std::path::PathBuf;
// use tempfile::tempdir;

// // Get the migrations from the diesel_sqlite module
// #[allow(dead_code)]
// pub const MIGRATIONS: EmbeddedMigrations = diesel_migrations::embed_migrations!("migrations");

// #[allow(dead_code)]
// pub fn setup_test_db() -> (SqliteConnection, PathBuf) {
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

// #[allow(dead_code)]
// pub fn cleanup(db_path: PathBuf) {
//     if db_path.exists() {
//         fs::remove_file(db_path).expect("Failed to remove test database file");
//     }
// }

// #[allow(dead_code)]
// pub fn get_unix_timestamp() -> i64 {
//     use std::time::{SystemTime, UNIX_EPOCH};

//     SystemTime::now()
//         .duration_since(UNIX_EPOCH)
//         .unwrap()
//         .as_secs() as i64
// }
