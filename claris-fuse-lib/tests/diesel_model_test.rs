#[cfg(test)]
mod model_tests {
    use diesel::dsl::*;
    use diesel::prelude::*;
    use diesel::sqlite::SqliteConnection;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::NamedTempFile;

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

    allow_tables_to_appear_in_same_query!(file_paths, versions, contents,);

    // Define the model structs
    #[derive(Queryable, Identifiable, Debug, Clone)]
    #[diesel(table_name = file_paths)]
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

    // Test that the model structs compile properly
    #[test]
    fn test_diesel_model_compile() {
        // This test just checks that the model definitions compile correctly
        // We don't need to do database operations to verify this
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let file_path = DbFilePath {
            id: 1,
            path: "/test/path/file.txt".to_string(),
            created_at: timestamp,
            last_modified: timestamp,
        };

        let version = DbVersion {
            id: 1,
            file_path_id: file_path.id,
            operation_type: "WRITE".to_string(),
            timestamp: timestamp,
            size: 1024,
            content_hash: Some("hash123".to_string()),
            description: Some("Test version".to_string()),
        };

        let content = DbContent {
            version_id: version.id,
            data: b"Hello, world!".to_vec(),
        };

        // Verify objects are created correctly
        assert_eq!(file_path.id, 1);
        assert_eq!(version.file_path_id, file_path.id);
        assert_eq!(content.version_id, version.id);
    }

    // This function would be used to setup a test database in real tests
    #[allow(dead_code)]
    fn setup_test_db() -> SqliteConnection {
        let temp_file = NamedTempFile::new().expect("Failed to create temp file");
        let db_path = temp_file.path().to_str().unwrap();

        let mut conn = SqliteConnection::establish(db_path).expect("Failed to connect to database");

        // Create the tables
        sql_query(
            "CREATE TABLE file_paths (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_modified INTEGER NOT NULL
        )",
        )
        .execute(&mut conn)
        .expect("Failed to create file_paths table");

        sql_query(
            "CREATE TABLE versions (
            id INTEGER PRIMARY KEY,
            file_path_id INTEGER NOT NULL,
            operation_type TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            size INTEGER NOT NULL,
            content_hash TEXT,
            description TEXT,
            FOREIGN KEY (file_path_id) REFERENCES file_paths(id)
        )",
        )
        .execute(&mut conn)
        .expect("Failed to create versions table");

        sql_query(
            "CREATE TABLE contents (
            version_id INTEGER PRIMARY KEY,
            data BLOB NOT NULL,
            FOREIGN KEY (version_id) REFERENCES versions(id)
        )",
        )
        .execute(&mut conn)
        .expect("Failed to create contents table");

        conn
    }
}
