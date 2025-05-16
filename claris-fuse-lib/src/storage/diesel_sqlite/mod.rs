use std::ops::DerefMut;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool, PooledConnection};
use diesel::sqlite::SqliteConnection;
use diesel::QueryableByName;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use log::{debug, info};
use tokio::sync::Mutex;

use crate::storage::{
    FileVersion, OperationType, SearchableStorage, StorageBackend, StorageError, StorageResult,
    VersionStorage, VersionedFile,
};

pub mod models;
pub mod schema;

use self::models::*;

// Define embedded migrations at compile time
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

// Type alias for our DB connection pool
type DbPool = Pool<ConnectionManager<SqliteConnection>>;

/// SQLite storage backend using Diesel ORM
pub struct DieselSqliteStorage {
    /// Path to the SQLite database file
    db_path: PathBuf,

    /// Connection pool for SQLite
    connection_pool: Arc<Mutex<Option<DbPool>>>,

    /// Maximum number of connections in the pool
    max_pool_size: u32,
}

impl DieselSqliteStorage {
    /// Create a new DieselSqliteStorage
    pub fn new<P: AsRef<Path>>(db_path: P) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
            connection_pool: Arc::new(Mutex::new(None)),
            max_pool_size: 10,
        }
    }

    /// Create a new DieselSqliteStorage with custom pool size
    pub fn with_pool_size<P: AsRef<Path>>(db_path: P, max_pool_size: u32) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
            connection_pool: Arc::new(Mutex::new(None)),
            max_pool_size,
        }
    }

    /// Get a connection from the pool
    async fn get_conn(
        &self,
    ) -> StorageResult<PooledConnection<ConnectionManager<SqliteConnection>>> {
        let pool_guard = self.connection_pool.lock().await;
        let pool = pool_guard
            .as_ref()
            .ok_or_else(|| StorageError::DatabaseError("Database not initialized".to_string()))?;

        pool.get().map_err(|e| {
            StorageError::DatabaseError(format!("Failed to get database connection: {}", e))
        })
    }

    /// Run the database migrations
    fn run_migrations(conn: &mut SqliteConnection) -> StorageResult<()> {
        conn.run_pending_migrations(MIGRATIONS)
            .map_err(|e| StorageError::DatabaseError(format!("Failed to run migrations: {}", e)))?;
        Ok(())
    }

    /// Convert from domain model to database model
    fn operation_type_to_string(operation_type: &OperationType) -> String {
        operation_type.to_string()
    }

    /// Convert from database model to domain model
    fn string_to_operation_type(s: &str) -> StorageResult<OperationType> {
        s.parse()
            .map_err(|e| StorageError::DatabaseError(format!("Invalid operation type: {}", e)))
    }

    /// Convert Unix timestamp to SystemTime
    fn timestamp_to_system_time(timestamp: i64) -> SystemTime {
        UNIX_EPOCH + std::time::Duration::from_secs(timestamp as u64)
    }

    /// Convert SystemTime to Unix timestamp
    fn system_time_to_timestamp(time: SystemTime) -> i64 {
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs() as i64
    }

    /// Convert a database version to a domain model
    fn db_version_to_file_version(
        &self,
        db_version: DbVersion,
        path: PathBuf,
    ) -> StorageResult<FileVersion> {
        let operation_type =
            DieselSqliteStorage::string_to_operation_type(&db_version.operation_type)?;

        Ok(FileVersion {
            id: db_version.id,
            path,
            operation_type,
            timestamp: DieselSqliteStorage::timestamp_to_system_time(db_version.timestamp),
            size: db_version.size as u64,
            content_hash: db_version.content_hash,
            description: db_version.description,
        })
    }
}

#[async_trait]
impl StorageBackend for DieselSqliteStorage {
    async fn init(&self) -> StorageResult<()> {
        let mut pool_guard = self.connection_pool.lock().await;

        // Check if already initialized
        if pool_guard.is_some() {
            return Ok(());
        }

        // Setup database URL
        let db_url = format!("sqlite:{}", self.db_path.display());
        let manager = ConnectionManager::<SqliteConnection>::new(db_url);

        // Create connection pool
        let pool = Pool::builder()
            .max_size(self.max_pool_size)
            .build(manager)
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to create connection pool: {}", e))
            })?;

        // Get a connection to run migrations
        let mut conn = pool.get().map_err(|e| {
            StorageError::DatabaseError(format!("Failed to connect to database: {}", e))
        })?;

        // Run migrations
        DieselSqliteStorage::run_migrations(&mut conn)?;

        // Store the pool
        *pool_guard = Some(pool);

        info!(
            "DieselSqliteStorage initialized at {}",
            self.db_path.display()
        );
        Ok(())
    }

    async fn close(&self) -> StorageResult<()> {
        let mut pool_guard = self.connection_pool.lock().await;
        *pool_guard = None;
        info!("DieselSqliteStorage closed");
        Ok(())
    }

    fn name(&self) -> &str {
        "Diesel SQLite"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }
}

#[async_trait]
impl VersionStorage for DieselSqliteStorage {
    async fn record_version(
        &self,
        path_buf: PathBuf,
        operation_type_param: OperationType,
        content: Option<Vec<u8>>,
    ) -> StorageResult<i64> {
        use self::schema::contents;
        use self::schema::file_paths;
        use self::schema::file_paths::dsl::path;
        use self::schema::versions;


        let mut conn = self.get_conn().await?;

        // Run in a transaction
        let result = conn.transaction::<_, diesel::result::Error, _>(|conn| {
            // Get or create file path
            let path_str = path_buf.to_string_lossy().to_string();
            let now = DieselSqliteStorage::system_time_to_timestamp(SystemTime::now());

            let file_path_entry = file_paths::table
                .filter(path.eq(&path_str))
                .first::<DbFilePath>(&mut *conn)
                .optional()?;

            let path_id = match file_path_entry {
                Some(fp) => fp.id,
                None => {
                    // Insert new file path
                    let new_file_path = NewDbFilePath {
                        path: &path_str,
                        created_at: now,
                        last_modified: now,
                    };

                    diesel::insert_into(file_paths::table)
                        .values(&new_file_path)
                        .execute(&mut *conn)?;

                    file_paths::table
                        .filter(path.eq(&path_str))
                        .select(file_paths::dsl::id)
                        .first(&mut *conn)?
                }
            };

            // Using the imported file_path_id from the schema

            // Calculate content hash and size
            let (content_hash_value, size_value) = match &content {
                Some(content_data) => {
                    use sha2::{Digest, Sha256};
                    let mut hasher = Sha256::new();
                    hasher.update(content_data);
                    let hash = format!("{:x}", hasher.finalize());
                    (Some(hash), content_data.len() as i64)
                }
                None => (None, 0),
            };

            // Insert version record
            let new_version = NewDbVersion {
                file_path_id: path_id,
                operation_type: &Self::operation_type_to_string(&operation_type_param),
                timestamp: now,
                size: size_value,
                content_hash: content_hash_value.as_deref(),
                description: None,
            };

            diesel::insert_into(versions::table)
                .values(&new_version)
                .execute(conn)?;

            let version_id: i64 =
                diesel::select(diesel::dsl::sql::<diesel::sql_types::BigInt>("last_insert_rowid()")).first(conn)?;

            // Store content if provided
            if let Some(content_data) = &content {
                let new_content = NewDbContent {
                    version_id,
                    data: content_data,
                };

                diesel::insert_into(contents::table)
                    .values(&new_content)
                    .execute(&mut *conn)?;
            }

            Ok(version_id)
        });

        match result {
            Ok(version_id) => {
                debug!("Recorded version {} for {}", version_id, path_buf.display());
                Ok(version_id)
            }
            Err(e) => Err(StorageError::DatabaseError(format!(
                "Failed to record version: {}",
                e
            ))),
        }
    }

    async fn get_file_versions(&self, path_buf: &Path) -> StorageResult<VersionedFile> {
        use self::schema::file_paths::dsl::{file_paths, path};
        use self::schema::versions::dsl::{file_path_id, timestamp, versions};

        let mut conn = self.get_conn().await?;
        let path_str = path_buf.to_string_lossy().to_string();

        // Find file path ID
        let file_path_entry = file_paths
            .filter(path.eq(&path_str))
            .first::<DbFilePath>(conn.deref_mut())
            .optional()
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?;

        let path_id = match file_path_entry {
            Some(fp) => fp.id,
            None => return Err(StorageError::FileNotFound(path_buf.to_path_buf())),
        };

        // Get all versions for this file path
        let db_versions = versions
            .filter(file_path_id.eq(&path_id))
            .order_by(timestamp.desc())
            .load::<DbVersion>(conn.deref_mut())
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?;

        // Convert to domain models
        let mut file_versions = Vec::new();
        for db_version in db_versions {
            let version = self.db_version_to_file_version(db_version, path_buf.to_path_buf())?;
            file_versions.push(version);
        }

        Ok(VersionedFile {
            path: path_buf.to_path_buf(),
            versions: file_versions,
        })
    }

    async fn get_version(&self, version_id_param: i64) -> StorageResult<FileVersion> {
        use self::schema::file_paths::dsl::file_paths;
        use self::schema::versions::dsl::versions;

        let mut conn = self.get_conn().await?;

        // Get version and associated file path
        let (db_version, db_file_path): (DbVersion, DbFilePath) = versions
            .find(version_id_param)
            .inner_join(file_paths)
            .first(&mut *conn)
            .map_err(|_| StorageError::VersionNotFound(version_id_param))?;

        // Convert to domain model
        self.db_version_to_file_version(db_version, PathBuf::from(db_file_path.path))
    }

    async fn get_version_content(&self, version_id_param: i64) -> StorageResult<Option<Vec<u8>>> {
        use self::schema::contents::dsl::{contents, data};
        use self::schema::versions::dsl::{id, versions};

        let mut conn = self.get_conn().await?;

        // Check if version exists
        let version_exists: bool = versions
            .find(version_id_param)
            .select(id)
            .first::<i64>(&mut *conn)
            .optional()
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?
            .is_some();

        if !version_exists {
            return Err(StorageError::VersionNotFound(version_id_param));
        }

        // Get content if it exists
        let content_result = contents
            .find(version_id_param)
            .select(data)
            .first::<Vec<u8>>(&mut *conn)
            .optional()
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?;

        Ok(content_result)
    }

    async fn get_versions(
        &self,
        path_prefix: Option<PathBuf>,
        since: Option<SystemTime>,
        until: Option<SystemTime>,
        operation_types: Option<Vec<OperationType>>,
    ) -> StorageResult<Vec<FileVersion>> {
        use self::schema::file_paths::dsl::{file_paths, path};
        use self::schema::versions::dsl::{operation_type, timestamp, versions};

        let mut conn = self.get_conn().await?;
        let mut query = versions.inner_join(file_paths).into_boxed();

        // Apply filters
        if let Some(prefix) = path_prefix {
            let prefix_str = prefix.to_string_lossy().to_string() + "%";
            query = query.filter(path.like(prefix_str));
        }

        if let Some(since_time) = since {
            let since_timestamp = DieselSqliteStorage::system_time_to_timestamp(since_time);
            query = query.filter(timestamp.ge(since_timestamp));
        }

        if let Some(until_time) = until {
            let until_timestamp = DieselSqliteStorage::system_time_to_timestamp(until_time);
            query = query.filter(timestamp.le(until_timestamp));
        }

        if let Some(op_types) = operation_types {
            if !op_types.is_empty() {
                let op_strings: Vec<String> = op_types
                    .iter()
                    .map(Self::operation_type_to_string)
                    .collect();

                query = query.filter(operation_type.eq_any(op_strings));
            }
        }

        // Execute query ordered by timestamp
        let results: Vec<(DbVersion, DbFilePath)> = query
            .order_by(timestamp.desc())
            .load(&mut *conn)
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?;

        // Convert to domain models
        let mut result_versions = Vec::new();
        for (db_version, db_file_path) in results {
            let file_version =
                self.db_version_to_file_version(db_version, PathBuf::from(db_file_path.path))?;
            result_versions.push(file_version);
        }

        Ok(result_versions)
    }
}

#[async_trait]
impl SearchableStorage for DieselSqliteStorage {
    async fn search_versions_by_description(
        &self,
        query_str: &str,
    ) -> StorageResult<Vec<FileVersion>> {
        let mut conn = self.get_conn().await?;

        // Define a struct for query results
        #[derive(QueryableByName, Debug)]
        struct SearchResult {
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            id: i64,
            #[diesel(sql_type = diesel::sql_types::Text)]
            operation_type: String,
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            timestamp: i64,
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            size: i64,
            #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
            content_hash: Option<String>,
            #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
            description: Option<String>,
            #[diesel(sql_type = diesel::sql_types::Text)]
            path: String,
        }

        // Use raw SQL for the FTS query
        let sql = r#"
            SELECT v.id, v.operation_type, v.timestamp, v.size, v.content_hash, v.description, fp.path
            FROM versions_fts fts
            JOIN versions v ON fts.id = v.id
            JOIN file_paths fp ON v.file_path_id = fp.id
            WHERE fts.description MATCH ?
            ORDER BY v.timestamp DESC
        "#;

        let results: Vec<SearchResult> = diesel::sql_query(sql)
            .bind::<diesel::sql_types::Text, _>(query_str)
            .load(&mut *conn)
            .map_err(|e| StorageError::DatabaseError(format!("Search query error: {}", e)))?;

        let mut result_versions = Vec::new();
        for result in results {
            let db_version = DbVersion {
                id: result.id,
                file_path_id: 0, // Not used in conversion
                operation_type: result.operation_type,
                timestamp: result.timestamp,
                size: result.size,
                content_hash: result.content_hash,
                description: result.description,
            };

            let file_version =
                self.db_version_to_file_version(db_version, PathBuf::from(result.path))?;

            result_versions.push(file_version);
        }

        Ok(result_versions)
    }

    async fn update_description(
        &self,
        version_id_param: i64,
        description_text: String,
    ) -> StorageResult<()> {
        use self::schema::versions::dsl::{description, id, versions};

        let mut conn = self.get_conn().await?;

        // Check if version exists
        let version_exists: bool = versions
            .find(version_id_param)
            .select(id)
            .first::<i64>(&mut *conn)
            .optional()
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?
            .is_some();

        if !version_exists {
            return Err(StorageError::VersionNotFound(version_id_param));
        }

        // Update description
        diesel::update(versions.find(version_id_param))
            .set(description.eq(Some(description_text.as_str())))
            .execute(&mut *conn)
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to update description: {}", e))
            })?;

        Ok(())
    }
}

/// Factory for creating DieselSqliteStorage instances
pub struct DieselSqliteStorageFactory {
    db_path: PathBuf,
    max_pool_size: u32,
}

impl DieselSqliteStorageFactory {
    /// Create a new factory with default pool size
    pub fn new<P: AsRef<Path>>(db_path: P) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
            max_pool_size: 10,
        }
    }

    /// Create a new factory with custom pool size
    pub fn with_pool_size<P: AsRef<Path>>(db_path: P, max_pool_size: u32) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
            max_pool_size,
        }
    }
}

impl crate::storage::StorageFactory for DieselSqliteStorageFactory {
    fn create_storage(&self) -> StorageResult<Box<dyn VersionStorage>> {
        Ok(Box::new(DieselSqliteStorage::with_pool_size(
            &self.db_path,
            self.max_pool_size,
        )))
    }
}

impl crate::storage::SearchableStorageFactory for DieselSqliteStorageFactory {
    fn create_searchable_storage(&self) -> StorageResult<Box<dyn SearchableStorage>> {
        Ok(Box::new(DieselSqliteStorage::with_pool_size(
            &self.db_path,
            self.max_pool_size,
        )))
    }
}
