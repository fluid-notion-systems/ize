use diesel::prelude::*;
use chrono::{DateTime, NaiveDateTime, Utc};
use super::schema::*;

/// Database model that directly maps to the file_paths table
#[derive(Queryable, Identifiable, Debug, Clone)]
#[diesel(table_name = file_paths)]
pub struct DbFilePath {
    pub id: i64,
    pub path: String,
    pub created_at: i64,
    pub last_modified: i64,
}

/// Insertable struct for creating new file_paths records
#[derive(Insertable, Debug)]
#[diesel(table_name = file_paths)]
pub struct NewDbFilePath<'a> {
    pub path: &'a str,
    pub created_at: i64,
    pub last_modified: i64,
}

/// Database model that directly maps to the versions table
#[derive(Queryable, Identifiable, Associations, Debug, Clone)]
#[diesel(table_name = versions)]
#[diesel(belongs_to(DbFilePath, foreign_key = file_path_id))]
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
    let naive = NaiveDateTime::from_timestamp_opt(timestamp, 0)
        .unwrap_or_else(|| NaiveDateTime::from_timestamp_opt(0, 0).unwrap());
    DateTime::from_naive_utc_and_offset(naive, Utc)
}

/// Convert a DateTime<Utc> to UNIX timestamp
pub fn datetime_to_timestamp(datetime: DateTime<Utc>) -> i64 {
    datetime.timestamp()
}