use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use std::io;

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool, PooledConnection};
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use log::{debug, info, warn};
use tokio::sync::Mutex;

use crate::storage::{
    FileVersion, OperationType, SearchableStorage, StorageBackend, StorageError,
    StorageResult, VersionStorage, VersionedFile,
};

pub mod models;
pub mod schema;

use self::models::*;
use self::schema::*;

// Define embedded migrations at compile time
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

type DbPool = Pool<ConnectionManager<SqliteConnection>>;

/// SQLite storage backend implementation using Diesel ORM
pub struct DieselSqliteStorage {
    /// Path to the SQLite database file
    db_path: PathBuf,
    
    /// Connection pool for SQLite
    connection_pool: Arc<Mutex<Option<DbPool>>>,
    
    /// Maximum number of connections in the pool
    max_pool_size: u32,
}

impl DieselSqliteStorage {
    /// Create a new DieselSqliteStorage with default settings
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
    async fn get_conn(&self) -> StorageResult<PooledConnection<ConnectionManager<SqliteConnection>>> {
        let pool_guard = self.connection_pool.lock().await;
        let pool = pool_guard.as_ref()
            .ok_or_else(|| StorageError::DatabaseError("Database not initialized".to_string()))?;
            
        pool.get().map_err(|e| StorageError::DatabaseError(format!("Failed to get database connection: {}", e)))
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
        s.parse().map_err(|e| StorageError::DatabaseError(format!("Invalid operation type: {}", e)))
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
    fn db_version_to_file_version(&self, db_version: DbVersion, path: PathBuf) -> StorageResult<FileVersion> {
        let operation_type = Self::string_to_operation_type(&db_version.operation_type)?;
        
        Ok(FileVersion {
            id: db_version.id,
            path,
            operation_type,
            timestamp: Self::timestamp_to_system_time(db_version.timestamp),
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
            .map_err(|e| StorageError::DatabaseError(format!("Failed to create connection pool: {}", e)))?;
            
        // Get a connection to run migrations
        let mut conn = pool.get()
            .map_err(|e| StorageError::DatabaseError(format!("Failed to connect to database: {}", e)))?;
            
        // Run migrations
        Self::run_migrations(&mut conn)?;
            
        // Store the pool
        *pool_guard = Some(pool);
        
        info!("DieselSqliteStorage initialized at {}", self.db_path.display());
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
        path: PathBuf,
        operation_type: OperationType,
        content: Option<Vec<u8>>,
    ) -> StorageResult<i64> {
        use self::schema::file_paths::dsl::*;
        use self::schema::versions::dsl::*;
        use self::schema::contents::dsl::*;
        
        let conn = self.get_conn().await?;
        
        // Run in a transaction
        let result = conn.transaction::<_, diesel::result::Error, _>(|conn| {
            // Get or create file path
            let path_str = path.to_string_lossy().to_string();
            let now = Self::system_time_to_timestamp(SystemTime::now());
            
            let file_path_entry = file_paths
                .filter(path.eq(&path_str))
                .first::<DbFilePath>(conn)
                .optional()?;
                
            let file_path_id = match file_path_entry {
                Some(fp) => fp.id,
                None => {
                    // Insert new file path
                    let new_file_path = NewDbFilePath {
                        path: &path_str,
                        created_at: now,
                        last_modified: now,
                    };
                    
                    diesel::insert_into(file_paths)
                        .values(&new_file_path)
                        .execute(conn)?;
                        
                    file_paths
                        .filter(path.eq(&path_str))
                        .select(id)
                        .first(conn)?
                }
            };
            
            // Calculate content hash and size
            let (content_hash, size) = match &content {
                Some(content_data) => {
                    use sha2::{Digest, Sha256};
                    let mut hasher = Sha256::new();
                    hasher.update(content_data);
                    let hash = format!("{:x}", hasher.finalize());
                    (Some(hash), content_data.len() as i64)
                },
                None => (None, 0),
            };
            
            // Insert version record
            let new_version = NewDbVersion {
                file_path_id,
                operation_type: &Self::operation_type_to_string(&operation_type),
                timestamp: now,
                size,
                content_hash: content_hash.as_deref(),
                description: None,
            };
            
            diesel::insert_into(versions)
                .values(&new_version)
                .execute(conn)?;
                
            let version_id: i64 = diesel::select(diesel::dsl::sql("last_insert_rowid()"))
                .first(conn)?;
                
            // Store content if provided
            if let Some(content_data) = &content {
                let new_content = NewDbContent {
                    version_id,
                    data: content_data,
                };
                
                diesel::insert_into(contents)
                    .values(&new_content)
                    .execute(conn)?;
            }
            
            Ok(version_id)
        });
        
        match result {
            Ok(version_id) => {
                debug!("Recorded version {} for {}", version_id, path.display());
                Ok(version_id)
            },
            Err(e) => Err(StorageError::DatabaseError(format!("Failed to record version: {}", e))),
        }
    }
    
    async fn get_file_versions(&self, path: &PathBuf) -> StorageResult<VersionedFile> {
        use self::schema::file_paths::dsl::*;
        use self::schema::versions::dsl::*;
        
        let conn = self.get_conn().await?;
        let path_str = path.to_string_lossy().to_string();
        
        // Find file path ID
        let file_path_entry = file_paths
            .filter(path.eq(&path_str))
            .first::<DbFilePath>(&conn)
            .optional()
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?;
            
        let file_path_id = match file_path_entry {
            Some(fp) => fp.id,
            None => return Err(StorageError::FileNotFound(path.clone())),
        };
        
        // Get all versions for this file path
        let db_versions = versions
            .filter(file_path_id.eq(file_path_id))
            .order_by(timestamp.desc())
            .load::<DbVersion>(&conn)
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?;
            
        // Convert to domain models
        let mut file_versions = Vec::new();
        for db_version in db_versions {
            let file_version = self.db_version_to_file_version(db_version, path.clone())?;
            file_versions.push(file_version);
        }
        
        Ok(VersionedFile {
            path: path.clone(),
            versions: file_versions,
        })
    }
    
    async fn get_version(&self, version_id: i64) -> StorageResult<FileVersion> {
        use self::schema::file_paths::dsl::*;
        use self::schema::versions::dsl::*;
        
        let conn = self.get_conn().await?;
        
        // Get version and associated file path
        let (db_version, db_file_path): (DbVersion, DbFilePath) = versions
            .find(version_id)
            .inner_join(file_paths)
            .first(&conn)
            .map_err(|_| StorageError::VersionNotFound(version_id))?;
            
        // Convert to domain model
        self.db_version_to_file_version(db_version, PathBuf::from(db_file_path.path))
    }
    
    async fn get_version_content(&self, version_id: i64) -> StorageResult<Option<Vec<u8>>> {
        use self::schema::contents::dsl::*;
        use self::schema::versions::dsl::*;
        
        let conn = self.get_conn().await?;
        
        // Check if version exists
        let version_exists: bool = versions
            .find(version_id)
            .select(id)
            .first::<i64>(&conn)
            .optional()
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?
            .is_some();
            
        if !version_exists {
            return Err(StorageError::VersionNotFound(version_id));
        }
        
        // Get content if it exists
        let content_result = contents
            .find(version_id)
            .select(data)
            .first::<Vec<u8>>(&conn)
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
        use self::schema::file_paths::dsl as fp_dsl;
        use self::schema::versions::dsl as v_dsl;
        
        let conn = self.get_conn().await?;
        let mut query = v_dsl::versions.inner_join(fp_dsl::file_paths).into_boxed();
        
        // Apply filters
        if let Some(prefix) = path_prefix {
            let prefix_str = prefix.to_string_lossy().to_string() + "%";
            query = query.filter(fp_dsl::path.like(prefix_str));
        }
        
        if let Some(since_time) = since {
            let since_timestamp = Self::system_time_to_timestamp(since_time);
            query = query.filter(v_dsl::timestamp.ge(since_timestamp));
        }
        
        if let Some(until_time) = until {
            let until_timestamp = Self::system_time_to_timestamp(until_time);
            query = query.filter(v_dsl::timestamp.le(until_timestamp));
        }
        
        if let Some(op_types) = operation_types {
            if !op_types.is_empty() {
                let op_strings: Vec<String> = op_types.iter()
                    .map(|op| Self::operation_type_to_string(op))
                    .collect();
                    
                query = query.filter(v_dsl::operation_type.eq_any(op_strings));
            }
        }
        
        // Execute query ordered by timestamp
        let results: Vec<(DbVersion, DbFilePath)> = query
            .order_by(v_dsl::timestamp.desc())
            .load(&conn)
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?;
            
        // Convert to domain models
        let mut versions = Vec::new();
        for (db_version, db_file_path) in results {
            let file_version = self.db_version_to_file_version(
                db_version, 
                PathBuf::from(db_file_path.path)
            )?;
            versions.push(file_version);
        }
        
        Ok(versions)
    }
}

#[async_trait]
impl SearchableStorage for DieselSqliteStorage {
    async fn search_versions_by_description(&self, query: &str) -> StorageResult<Vec<FileVersion>> {
        let conn = self.get_conn().await?;
        
        // Use raw SQL for the FTS query
        let sql = r#"
            SELECT v.id, v.operation_type, v.timestamp, v.size, v.content_hash, v.description, fp.path
            FROM versions_fts fts
            JOIN versions v ON fts.id = v.id
            JOIN file_paths fp ON v.file_path_id = fp.id
            WHERE fts.description MATCH ?
            ORDER BY v.timestamp DESC
        "#;
        
        let results: Vec<(i64, String, i64, i64, Option<String>, Option<String>, String)> = 
            diesel::sql_query(sql)
                .bind::<diesel::sql_types::Text, _>(query)
                .load(&conn)
                .map_err(|e| StorageError::DatabaseError(format!("Search query error: {}", e)))?;
        
        let mut versions = Vec::new();
        for (id, op_type, timestamp, size, content_hash, description, path_str) in results {
            let db_version = DbVersion {
                id,
                file_path_id: 0, // Not used in conversion
                operation_type: op_type,
                timestamp,
                size,
                content_hash,
                description,
            };
            
            let file_version = self.db_version_to_file_version(
                db_version, 
                PathBuf::from(path_str)
            )?;
            
            versions.push(file_version);
        }
        
        Ok(versions)
    }
    
    async fn update_description(&self, version_id: i64, description: String) -> StorageResult<()> {
        use self::schema::versions::dsl::*;
        
        let conn = self.get_conn().await?;
        
        // Check if version exists
        let version_exists: bool = versions
            .find(version_id)
            .select(id)
            .first::<i64>(&conn)
            .optional()
            .map_err(|e| StorageError::DatabaseError(format!("Database query error: {}", e)))?
            .is_some();
            
        if !version_exists {
            return Err(StorageError::VersionNotFound(version_id));
        }
        
        // Update description
        diesel::update(versions.find(version_id))
            .set(self::schema::versions::description.eq(description))
            .execute(&conn)
            .map_err(|e| StorageError::DatabaseError(format!("Failed to update description: {}", e)))?;
            
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