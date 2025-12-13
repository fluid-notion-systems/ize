# Pijul Querying Operations Research

## Overview

This document explores the query capabilities available in libpijul and how to expose them through our `PijulBackend` for use in the Izev TUI.

## Current PijulBackend Query Methods

### Existing Methods

| Method | Returns | Description |
|--------|---------|-------------|
| `list_channels()` | `Vec<String>` | List all channel names |
| `current_channel()` | `&str` | Get current channel name |
| `list_changes()` | `Vec<Hash>` | List change hashes in current channel |
| `file_exists(path)` | `bool` | Check if file exists |
| `get_file_content(path)` | `Vec<u8>` | Get file contents |
| `list_files()` | `Vec<String>` | **Stub** - currently returns empty vec |

### Missing Capabilities

1. **Change metadata** - No way to get message, timestamp, author for a change
2. **File listing** - `list_files()` is not implemented
3. **Change details** - No way to see what files a change touched
4. **Path history** - No way to query changes that affected a specific path
5. **Diff generation** - No way to get diff for a change

## libpijul Query Capabilities

### ChangeHeader Structure

From `vendor/pijul/libpijul/src/change.rs`:

```rust
pub struct ChangeHeader_<Author> {
    pub message: String,
    pub description: Option<String>,
    pub timestamp: Timestamp,
    pub authors: Vec<Author>,
}

pub struct Author(pub std::collections::BTreeMap<String, String>);
```

### ChangeStore Trait

From `vendor/pijul/libpijul/src/changestore/mod.rs`:

```rust
pub trait ChangeStore {
    fn get_header(&self, h: &Hash) -> Result<ChangeHeader, Self::Error>;
    fn get_change(&self, h: &Hash) -> Result<Change, Self::Error>;
    fn get_dependencies(&self, hash: &Hash) -> Result<Vec<Hash>, Self::Error>;
    fn get_changes(&self, hash: &Hash) -> Result<Vec<Hunk>, Self::Error>;
}
```

### TxnTExt Trait (Transaction Extensions)

From `vendor/pijul/libpijul/src/lib.rs`:

```rust
pub trait TxnTExt {
    // Log traversal
    fn log(channel, from: u64) -> Log;           // Forward chronological
    fn reverse_log(channel, from) -> RevLog;     // Reverse chronological
    
    // Path-specific history
    fn log_for_path(channel, pos, from) -> PathChangeset;
    fn rev_log_for_path(channel, pos, from) -> RevPathChangeset;
    
    // Change lookup
    fn get_changes(channel, n: u64) -> Option<(Hash, Merkle)>;
    fn get_revchanges(channel, hash) -> Option<u64>;
    
    // File queries
    fn touched_files(hash) -> Option<Touched>;
    fn find_oldest_path(changes, channel, path);
    fn follow_oldest_path(changes, channel, path);
    
    // State queries
    fn is_tracked(path) -> bool;
    fn is_directory(path) -> bool;
    fn iter_working_copy() -> Iterator;
    fn current_state(channel) -> Merkle;
}
```

### Log Iterator

Returns entries in chronological order with `(timestamp, (hash, merkle))` tuples.

## Proposed Query Operations for Izev

### Essential (Phase 1)

| Operation | Purpose | libpijul Source |
|-----------|---------|-----------------|
| `get_change_header(hash)` | Get message, timestamp, author | `ChangeStore::get_header` |
| `list_changes_with_metadata()` | List changes with full info | `log()` + `get_header` |
| `list_files()` | Proper file listing | `iter_working_copy` or tree traversal |
| `get_touched_files(hash)` | Files changed by a change | `touched_files()` |

### Important (Phase 2)

| Operation | Purpose | libpijul Source |
|-----------|---------|-----------------|
| `get_file_history(path)` | Changes affecting a path | `log_for_path` |
| `get_change_diff(hash)` | Human-readable diff | `get_changes` + format |
| `get_change_dependencies(hash)` | Change dependencies | `get_dependencies` |

### Nice to Have (Phase 3)

| Operation | Purpose | libpijul Source |
|-----------|---------|-----------------|
| `search_changes(query)` | Search by message/author | iterate + filter |
| `get_file_at_change(path, hash)` | File content at specific change | output with state |
| `compare_channels(a, b)` | Diff between channels | compare logs |

## Architecture Decision: Backend Methods vs Query Class

### Option A: Extend PijulBackend

**Pros:**
- Single point of access
- Consistent API surface
- Easier to maintain transaction lifecycle

**Cons:**
- Backend grows large
- Mixing mutation and query concerns

### Option B: Separate PijulQuery Class

```rust
pub struct PijulQuery<'a> {
    backend: &'a PijulBackend,
}

impl<'a> PijulQuery<'a> {
    pub fn changes(&self) -> ChangeQuery<'a> { ... }
    pub fn files(&self) -> FileQuery<'a> { ... }
}
```

**Pros:**
- Clean separation of concerns
- Can compose complex queries
- Backend stays focused on mutations

**Cons:**
- More types to manage
- Need to share pristine/txn access

### Recommendation: Hybrid Approach

1. Add **simple queries** directly to `PijulBackend`:
   - `get_change_header(hash)`
   - `list_files()`
   - `get_touched_files(hash)`

2. Create **`ChangeInfo` struct** for rich change data:
   ```rust
   pub struct ChangeInfo {
       pub hash: Hash,
       pub message: String,
       pub description: Option<String>,
       pub timestamp: DateTime<Utc>,
       pub authors: Vec<String>,
       pub files_changed: usize,
   }
   ```

3. Add **`list_changes_detailed()`** method returning `Vec<ChangeInfo>`

4. Future: Consider `PijulQuery` builder if queries become complex

## Implementation Priority

### For Izev Stream View (Immediate Need)

```rust
impl PijulBackend {
    /// Get detailed information about all changes in current channel
    pub fn list_changes_detailed(&self) -> Result<Vec<ChangeInfo>, PijulError> {
        let txn = self.txn_begin()?;
        let channel = txn.load_channel(&self.current_channel)?;
        let change_store = self.get_change_store();
        
        let mut changes = Vec::new();
        for entry in txn.log(&channel.read(), 0)? {
            let (_, (hash_ref, _)) = entry?;
            let hash: Hash = (*hash_ref).into();
            let header = change_store.get_header(&hash)?;
            
            changes.push(ChangeInfo {
                hash,
                message: header.message,
                description: header.description,
                timestamp: header.timestamp.into(),
                authors: header.authors.iter()
                    .filter_map(|a| a.0.get("name").cloned())
                    .collect(),
                files_changed: 0, // TODO: count from touched_files
            });
        }
        
        Ok(changes)
    }
}
```

## Open Questions

1. **Performance**: Should we cache change headers? They're immutable once created.

2. **Pagination**: For large histories, should queries support offset/limit?

3. **Filtering**: Should we support filtering at the query level or in the UI?

4. **Real-time updates**: How to efficiently detect new changes for live refresh?

## Related Files

- `crates/ize-lib/src/pijul/backend.rs` - Current backend implementation
- `vendor/pijul/libpijul/src/change.rs` - Change/ChangeHeader structs
- `vendor/pijul/libpijul/src/changestore/mod.rs` - ChangeStore trait
- `vendor/pijul/libpijul/src/lib.rs` - TxnTExt query methods

## Next Steps

1. [ ] Implement `ChangeInfo` struct
2. [ ] Add `get_change_header()` to PijulBackend
3. [ ] Implement `list_changes_detailed()`
4. [ ] Fix `list_files()` implementation
5. [ ] Connect Izev Stream view to real data
6. [ ] Add `get_touched_files()` for file count