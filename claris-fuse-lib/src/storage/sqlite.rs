use async_trait::async_trait;
use log::{debug, info};
use rusqlite::{params, Connection, Result as RusqliteResult};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use hex::encode as hex_encode;
use sha2::{Digest, Sha256};

use super::{
    FileVersion, OperationType, SearchableStorage, SqliteSchema, StorageBackend, StorageError,
    StorageResult, VersionStorage, VersionedFile,
};

/// SQLite implementation of the version storage backend
pub struct SqliteStorage {
    /// Path to the SQLite database file
    db_path: PathBuf,

    /// Connection to the SQLite database (wrapped in Mutex for thread safety)
    connection: Arc<Mutex<Option<Connection>>>,

    /// Whether to use WAL (Write-Ahead Logging) mode for better performance
    use_wal: bool,

    /// Whether to use synchronous mode (safer but slower)
    synchronous: bool,
}

impl SqliteStorage {
    /// Create a new SQLite storage backend
    pub fn new<P: AsRef<Path>>(db_path: P) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
            connection: Arc::new(Mutex::new(None)),
            use_wal: true,
            synchronous: true,
        }
    }

    /// Create a new SQLite storage backend with custom settings
    pub fn with_options<P: AsRef<Path>>(db_path: P, use_wal: bool, synchronous: bool) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
            connection: Arc::new(Mutex::new(None)),
            use_wal,
            synchronous,
        }
    }

    /// Calculate SHA-256 hash of the content for deduplication
    fn calculate_hash(content: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);
        hex_encode(hasher.finalize())
    }

    /// Convert timestamp to SQLite compatible integer (seconds since UNIX epoch)
    fn time_to_sqlite(time: SystemTime) -> i64 {
        time.duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs() as i64
    }

    /// Convert SQLite timestamp to SystemTime
    fn sqlite_to_time(timestamp: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(timestamp as u64)
    }

    /// Get file path ID or create a new entry if it doesn't exist
    fn get_or_create_file_path(&self, conn: &Connection, path: &Path) -> RusqliteResult<i64> {
        let path_str = path.to_string_lossy().to_string();
        let now = Self::time_to_sqlite(SystemTime::now());

        // Try to get existing path ID
        let mut stmt = conn.prepare("SELECT id FROM file_paths WHERE path = ?")?;
        let result = stmt.query_row(params![path_str], |row| row.get(0));

        match result {
            Ok(id) => Ok(id),
            Err(_) => {
                // Path doesn't exist, insert new entry
                conn.execute(
                    "INSERT INTO file_paths (path, created_at, last_modified) VALUES (?, ?, ?)",
                    params![path_str, now, now],
                )?;

                Ok(conn.last_insert_rowid())
            }
        }
    }

    /// Record a new version using the provided connection
    fn record_version_with_conn(
        &self,
        conn: &Connection,
        path: &Path,
        operation_type: &OperationType,
        content: Option<&[u8]>,
    ) -> RusqliteResult<i64> {
        let file_path_id = self.get_or_create_file_path(conn, path)?;
        let now = Self::time_to_sqlite(SystemTime::now());

        // Calculate content hash and size
        let (content_hash, size) = match &content {
            Some(content) => (Some(Self::calculate_hash(content)), content.len() as u64),
            None => (None, 0),
        };

        // Insert version record
        conn.execute(
            "INSERT INTO versions (file_path_id, operation_type, timestamp, size, content_hash) 
             VALUES (?, ?, ?, ?, ?)",
            params![
                file_path_id,
                &operation_type.to_string(),
                now,
                size as i64,
                content_hash
            ],
        )?;

        let version_id = conn.last_insert_rowid();

        // Store content if provided
        if let Some(content) = content {
            conn.execute(
                "INSERT INTO contents (version_id, data) VALUES (?, ?)",
                params![version_id, content],
            )?;
        }

        Ok(version_id)
    }

    /// Load a FileVersion from a row
    fn load_version_from_row(
        &self,
        path: PathBuf,
        row: &rusqlite::Row,
    ) -> RusqliteResult<FileVersion> {
        let id: i64 = row.get(0)?;
        let operation_type_str: String = row.get(1)?;
        let operation_type = operation_type_str
            .parse::<OperationType>()
            .map_err(|_e| rusqlite::Error::ExecuteReturnedResults)?;
        let timestamp: i64 = row.get(2)?;
        let size: i64 = row.get(3)?;
        let content_hash: Option<String> = row.get(4)?;
        let description: Option<String> = row.get(5)?;

        Ok(FileVersion {
            id,
            path,
            operation_type,
            timestamp: Self::sqlite_to_time(timestamp),
            size: size as u64,
            content_hash,
            description,
        })
    }
}

#[async_trait]
impl StorageBackend for SqliteStorage {
    async fn init(&self) -> StorageResult<()> {
        let mut conn_guard = self.connection.lock().await;

        let conn = Connection::open(&self.db_path)
            .map_err(|e| StorageError::DatabaseError(format!("Failed to open database: {}", e)))?;

        // Configure connection
        if self.use_wal {
            conn.execute("PRAGMA journal_mode = WAL", []).map_err(|e| {
                StorageError::DatabaseError(format!("Failed to set WAL mode: {}", e))
            })?;
        }

        conn.execute(
            &format!(
                "PRAGMA synchronous = {}",
                if self.synchronous { "NORMAL" } else { "OFF" }
            ),
            [],
        )
        .map_err(|e| {
            StorageError::DatabaseError(format!("Failed to set synchronous mode: {}", e))
        })?;

        // Initialize schema
        SqliteSchema::initialize(&conn).map_err(|e| {
            StorageError::DatabaseError(format!("Failed to initialize schema: {}", e))
        })?;

        *conn_guard = Some(conn);

        info!("SQLite storage initialized at {}", self.db_path.display());
        Ok(())
    }

    async fn close(&self) -> StorageResult<()> {
        let mut conn_guard = self.connection.lock().await;
        *conn_guard = None;
        info!("SQLite storage closed");
        Ok(())
    }

    fn name(&self) -> &str {
        "SQLite"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }
}

#[async_trait]
impl VersionStorage for SqliteStorage {
    async fn record_version(
        &self,
        path: PathBuf,
        operation_type: OperationType,
        content: Option<Vec<u8>>,
    ) -> StorageResult<i64> {
        let conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_ref()
            .ok_or_else(|| StorageError::DatabaseError("Database not initialized".to_string()))?;

        let content_ref = content.as_deref();
        let version_id = self
            .record_version_with_conn(conn, &path, &operation_type, content_ref)
            .map_err(|e| StorageError::StorageError(format!("Failed to record version: {}", e)))?;

        debug!(
            "Recorded new version {} for path {}",
            version_id,
            path.display()
        );
        Ok(version_id)
    }

    async fn get_file_versions(&self, path: &Path) -> StorageResult<VersionedFile> {
        let conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_ref()
            .ok_or_else(|| StorageError::DatabaseError("Database not initialized".to_string()))?;

        let path_str = path.to_string_lossy().to_string();

        // Check if file exists
        let mut stmt = conn
            .prepare("SELECT id FROM file_paths WHERE path = ?")
            .map_err(|e| StorageError::DatabaseError(format!("Query error: {}", e)))?;

        let file_path_id: i64 = stmt
            .query_row(params![path_str], |row| row.get(0))
            .map_err(|_| StorageError::FileNotFound(path.to_path_buf()))?;

        // Get versions for this file
        let mut stmt = conn
            .prepare(
                "SELECT v.id, v.operation_type, v.timestamp, v.size, v.content_hash, v.description
             FROM versions v
             WHERE v.file_path_id = ?
             ORDER BY v.timestamp DESC",
            )
            .map_err(|e| StorageError::DatabaseError(format!("Query error: {}", e)))?;

        let rows = stmt
            .query_map(params![file_path_id], |row| {
                self.load_version_from_row(path.to_path_buf(), row)
            })
            .map_err(|e| StorageError::DatabaseError(format!("Query error: {}", e)))?;

        let mut versions = Vec::new();
        for row_result in rows {
            match row_result {
                Ok(version) => versions.push(version),
                Err(e) => return Err(StorageError::DatabaseError(format!("Row error: {}", e))),
            }
        }

        Ok(VersionedFile {
            path: path.to_path_buf(),
            versions,
        })
    }

    async fn get_version(&self, version_id: i64) -> StorageResult<FileVersion> {
        let conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_ref()
            .ok_or_else(|| StorageError::DatabaseError("Database not initialized".to_string()))?;

        let mut stmt = conn.prepare(
            "SELECT v.id, v.operation_type, v.timestamp, v.size, v.content_hash, v.description, fp.path
             FROM versions v
             JOIN file_paths fp ON v.file_path_id = fp.id
             WHERE v.id = ?"
        ).map_err(|e| StorageError::StorageError(format!("Query error: {}", e)))?;

        let result = stmt.query_row(params![version_id], |row| {
            let path_str: String = row.get(6)?;
            let path = PathBuf::from(path_str);

            let id: i64 = row.get(0)?;
            let operation_type_str: String = row.get(1)?;
            let operation_type = operation_type_str
                .parse::<OperationType>()
                .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
            let timestamp: i64 = row.get(2)?;
            let size: i64 = row.get(3)?;
            let content_hash: Option<String> = row.get(4)?;
            let description: Option<String> = row.get(5)?;

            Ok(FileVersion {
                id,
                path,
                operation_type,
                timestamp: Self::sqlite_to_time(timestamp),
                size: size as u64,
                content_hash,
                description,
            })
        });

        match result {
            Ok(version) => Ok(version),
            Err(_) => Err(StorageError::VersionNotFound(version_id)),
        }
    }

    async fn get_version_content(&self, version_id: i64) -> StorageResult<Option<Vec<u8>>> {
        let conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_ref()
            .ok_or_else(|| StorageError::DatabaseError("Database not initialized".to_string()))?;

        // First check if version exists
        let mut stmt = conn
            .prepare("SELECT id FROM versions WHERE id = ?")
            .map_err(|e| StorageError::DatabaseError(format!("Query error: {}", e)))?;

        let version_exists = stmt.query_row(params![version_id], |_| Ok(())).is_ok();

        if !version_exists {
            return Err(StorageError::VersionNotFound(version_id));
        }

        // Get content
        let mut stmt = conn
            .prepare("SELECT data FROM contents WHERE version_id = ?")
            .map_err(|e| StorageError::DatabaseError(format!("Query error: {}", e)))?;

        let result = stmt.query_row(params![version_id], |row| {
            let data: Vec<u8> = row.get(0)?;
            Ok(data)
        });

        match result {
            Ok(content) => Ok(Some(content)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None), // No content stored for this version
            Err(e) => Err(StorageError::DatabaseError(format!(
                "Content query error: {}",
                e
            ))),
        }
    }

    async fn get_versions(
        &self,
        path_prefix: Option<PathBuf>,
        since: Option<SystemTime>,
        until: Option<SystemTime>,
        operation_types: Option<Vec<OperationType>>,
    ) -> StorageResult<Vec<FileVersion>> {
        let conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_ref()
            .ok_or_else(|| StorageError::StorageError("Database not initialized".to_string()))?;

        // Build query with filters
        let mut query = String::from(
            "SELECT v.id, v.operation_type, v.timestamp, v.size, v.content_hash, v.description, fp.path
             FROM versions v
             JOIN file_paths fp ON v.file_path_id = fp.id
             WHERE 1=1"
        );

        let mut params = Vec::new();

        // Add path prefix filter
        if let Some(prefix) = path_prefix {
            query.push_str(" AND fp.path LIKE ?");
            params.push(format!("{}%", prefix.to_string_lossy()));
        }

        // Add time range filters
        if let Some(since_time) = since {
            query.push_str(" AND v.timestamp >= ?");
            params.push(Self::time_to_sqlite(since_time).to_string());
        }

        if let Some(until_time) = until {
            query.push_str(" AND v.timestamp <= ?");
            params.push(Self::time_to_sqlite(until_time).to_string());
        }

        // Add operation type filter
        if let Some(ops) = operation_types {
            if !ops.is_empty() {
                query.push_str(" AND v.operation_type IN (");
                query.push_str(&vec!["?"; ops.len()].join(", "));
                query.push(')');

                for op in ops {
                    params.push(op.to_string());
                }
            }
        }

        // Order by timestamp descending
        query.push_str(" ORDER BY v.timestamp DESC");

        // Convert params to rusqlite params
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();

        // Execute query
        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| StorageError::DatabaseError(format!("Query error: {}", e)))?;

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                let path_str: String = row.get(6)?;
                let path = PathBuf::from(path_str);

                let id: i64 = row.get(0)?;
                let operation_type_str: String = row.get(1)?;
                let operation_type = operation_type_str
                    .parse::<OperationType>()
                    .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
                let timestamp: i64 = row.get(2)?;
                let size: i64 = row.get(3)?;
                let content_hash: Option<String> = row.get(4)?;
                let description: Option<String> = row.get(5)?;

                Ok(FileVersion {
                    id,
                    path,
                    operation_type,
                    timestamp: Self::sqlite_to_time(timestamp),
                    size: size as u64,
                    content_hash,
                    description,
                })
            })
            .map_err(|e| StorageError::DatabaseError(format!("Query error: {}", e)))?;

        let mut versions = Vec::new();
        for row_result in rows {
            match row_result {
                Ok(version) => versions.push(version),
                Err(e) => return Err(StorageError::DatabaseError(format!("Row error: {}", e))),
            }
        }

        Ok(versions)
    }
}

#[async_trait]
impl SearchableStorage for SqliteStorage {
    async fn search_versions_by_description(&self, query: &str) -> StorageResult<Vec<FileVersion>> {
        let conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_ref()
            .ok_or_else(|| StorageError::StorageError("Database not initialized".to_string()))?;

        // Search using FTS
        let sql = "
            SELECT v.id, v.operation_type, v.timestamp, v.size, v.content_hash, v.description, fp.path
            FROM versions_fts fts
            JOIN versions v ON fts.id = v.id
            JOIN file_paths fp ON v.file_path_id = fp.id
            WHERE fts.description MATCH ?
            ORDER BY v.timestamp DESC
        ";

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| StorageError::DatabaseError(format!("Query error: {}", e)))?;

        let rows = stmt
            .query_map(params![query], |row| {
                let path_str: String = row.get(6)?;
                let path = PathBuf::from(path_str);

                let id: i64 = row.get(0)?;
                let operation_type_str: String = row.get(1)?;
                let operation_type = operation_type_str
                    .parse::<OperationType>()
                    .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
                let timestamp: i64 = row.get(2)?;
                let size: i64 = row.get(3)?;
                let content_hash: Option<String> = row.get(4)?;
                let description: Option<String> = row.get(5)?;

                Ok(FileVersion {
                    id,
                    path,
                    operation_type,
                    timestamp: Self::sqlite_to_time(timestamp),
                    size: size as u64,
                    content_hash,
                    description,
                })
            })
            .map_err(|e| StorageError::DatabaseError(format!("Query error: {}", e)))?;

        let mut versions = Vec::new();
        for row_result in rows {
            match row_result {
                Ok(version) => versions.push(version),
                Err(e) => return Err(StorageError::DatabaseError(format!("Row error: {}", e))),
            }
        }

        Ok(versions)
    }

    async fn update_description(&self, version_id: i64, description: String) -> StorageResult<()> {
        let conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_ref()
            .ok_or_else(|| StorageError::StorageError("Database not initialized".to_string()))?;

        // Check if version exists
        let mut stmt = conn
            .prepare("SELECT id FROM versions WHERE id = ?")
            .map_err(|e| StorageError::DatabaseError(format!("Query error: {}", e)))?;

        let version_exists = stmt.query_row(params![version_id], |_| Ok(())).is_ok();

        if !version_exists {
            return Err(StorageError::VersionNotFound(version_id));
        }

        // Update description
        conn.execute(
            "UPDATE versions SET description = ? WHERE id = ?",
            params![description, version_id],
        )
        .map_err(|e| StorageError::DatabaseError(format!("Update error: {}", e)))?;

        Ok(())
    }
}

/// Factory for creating SQLite storage instances
pub struct SqliteStorageFactory {
    db_path: PathBuf,
    use_wal: bool,
    synchronous: bool,
}

impl SqliteStorageFactory {
    /// Create a new SQLite storage factory
    pub fn new<P: AsRef<Path>>(db_path: P) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
            use_wal: true,
            synchronous: true,
        }
    }

    /// Create a new SQLite storage factory with custom options
    pub fn with_options<P: AsRef<Path>>(db_path: P, use_wal: bool, synchronous: bool) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
            use_wal,
            synchronous,
        }
    }
}

impl super::StorageFactory for SqliteStorageFactory {
    fn create_storage(&self) -> StorageResult<Box<dyn VersionStorage>> {
        Ok(Box::new(SqliteStorage::with_options(
            &self.db_path,
            self.use_wal,
            self.synchronous,
        )))
    }
}

impl super::SearchableStorageFactory for SqliteStorageFactory {
    fn create_searchable_storage(&self) -> StorageResult<Box<dyn SearchableStorage>> {
        Ok(Box::new(SqliteStorage::with_options(
            &self.db_path,
            self.use_wal,
            self.synchronous,
        )))
    }
}
