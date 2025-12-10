# Pijul Integration Points for Ize

This document identifies the key "rubber meets the road" integration points in libpijul for Ize's use case: versioning filesystem operations into a bare Pijul repository.

## Key Modules

| Module | Purpose |
|--------|---------|
| `libpijul::record` | Compare working copy to pristine, generate changes |
| `libpijul::output` | Write pristine state to working copy |
| `libpijul::alive` | Retrieve file content graph from pristine |
| `libpijul::fs` | Track files/inodes in working copy |
| `libpijul::working_copy` | Trait for filesystem abstraction |
| `libpijul::changestore` | Store and retrieve change data |
| `libpijul::pristine` | Core database (Sanakirja) transactions |

## Core Operations for Ize

### 1. Reading File Content from Pristine

**Function:** `libpijul::output::output_file`

```rust
// vendor/pijul/libpijul/src/output/mod.rs
pub fn output_file<
    T: TreeTxnT + ChannelTxnT,
    C: ChangeStore,
    V: VertexBuffer,
>(
    changes: &C,
    txn: &ArcTxn<T>,
    channel: &ChannelRef<T>,
    v0: Position<ChangeId>,       // File's inode vertex position
    out: &mut V,                   // Output buffer
) -> Result<(), FileError<C::Error, T>>
```

**How it works:**
1. Calls `alive::retrieve()` to build the file's content graph
2. Calls `alive::output_graph()` to serialize graph to buffer
3. Handles conflicts (zombie, cyclic, order conflicts)

**Used in CLI:** `pijul reset --dry-run` outputs a single file to stdout.

### 2. Recording Changes (Working Copy → Pristine)

**Function:** `libpijul::RecordBuilder::record`

```rust
// vendor/pijul/libpijul/src/record.rs
impl Builder {
    pub fn record<T, W: WorkingCopyRead, C: ChangeStore>(
        &mut self,
        txn: ArcTxn<T>,
        diff_algorithm: Algorithm,
        stop_early: bool,
        diff_separator: &regex::bytes::Regex,
        channel: ChannelRef<T>,
        working_copy: &W,           // Reads from filesystem
        changes: &C,
        prefix: &str,               // Path prefix to record
        n_workers: usize,
    ) -> Result<(), RecordError<...>>
}
```

**How it works:**
1. Walks the working copy filesystem
2. Compares each file against pristine state
3. Generates `Hunk` actions for differences
4. Returns `Recorded` struct with actions and content

**Key insight:** Recording compares `WorkingCopyRead` against pristine. For Ize, we can:
- Implement a custom `WorkingCopyRead` that reads from our OpQueue data
- Or write to `working/` first, then call `record()`

### 3. Outputting Repository (Pristine → Working Copy)

**Function:** `libpijul::output::output_repository_no_pending`

```rust
// vendor/pijul/libpijul/src/output/output.rs
pub fn output_repository_no_pending<
    T: ChannelMutTxnT + TreeMutTxnT,
    R: WorkingCopy,
    P: ChangeStore,
>(
    repo: &R,                      // Working copy to write to
    changes: &P,
    txn: &ArcTxn<T>,
    channel: &ChannelRef<T>,
    prefix: &str,
    output_name_conflicts: bool,
    if_modified_since: Option<SystemTime>,
    n_workers: usize,
    salt: u64,
) -> Result<BTreeSet<Conflict>, OutputError<...>>
```

**How it works:**
1. Walks pristine tree structure
2. Compares against working copy tree
3. Outputs files that are new/modified/deleted
4. Handles file renames and conflicts

**Used in CLI:** `pijul reset` resets working copy to pristine state.

### 4. Finding File Position by Path

**Function:** `TxnTExt::follow_oldest_path`

```rust
// Used in reset.rs
let (pos, _ambiguous) = txn.read()
    .follow_oldest_path(&repo.changes, &channel, &path)?;
```

Returns `Position<ChangeId>` for a given path string.

### 5. Applying Changes

**Function:** `TxnTExt::apply_local_change`

```rust
// vendor/pijul/pijul/src/commands/record.rs
txn_.apply_local_change(&mut channel, &change, &hash, &updates)?;
```

Applies a recorded change to the channel (branch).

## WorkingCopy Trait

The `WorkingCopy` trait abstracts filesystem operations:

```rust
// vendor/pijul/libpijul/src/working_copy/mod.rs
pub trait WorkingCopyRead {
    type Error: std::error::Error + Send;
    fn file_metadata(&self, file: &str) -> Result<InodeMetadata, Self::Error>;
    fn read_file(&self, file: &str, buffer: &mut Vec<u8>) -> Result<(), Self::Error>;
    fn modified_time(&self, file: &str) -> Result<SystemTime, Self::Error>;
}

pub trait WorkingCopy: WorkingCopyRead {
    fn create_dir_all(&self, path: &str) -> Result<(), Self::Error>;
    fn remove_path(&self, name: &str, rec: bool) -> Result<(), Self::Error>;
    fn rename(&self, former: &str, new: &str) -> Result<(), Self::Error>;
    fn set_permissions(&self, name: &str, permissions: u16) -> Result<(), Self::Error>;
    type Writer: std::io::Write;
    fn write_file(&self, file: &str, inode: Inode) -> Result<Self::Writer, Self::Error>;
}
```

**For Ize:** We can implement `WorkingCopyRead` to read from our OpQueue data instead of filesystem.

## Database Transactions

```rust
// Start read transaction
let txn = repo.pristine.arc_txn_begin()?;
let txn_read = txn.read();

// Start write transaction  
let mut txn_write = txn.write();

// Load channel (branch)
let channel = txn_read.load_channel("main")?;

// Commit
txn.commit()?;
```

## Ize Integration Strategy

### Option A: Use Standard Record Flow

1. Write to `working/` via PassthroughFS
2. Enqueue op with metadata (not full data)
3. OpProcessor calls `RecordBuilder::record()` on `working/`
4. Changes are computed by comparing working/ to pristine

**Pros:** Uses existing Pijul machinery
**Cons:** Record does full diff, may miss byte-level granularity

### Option B: Custom WorkingCopyRead

1. Write to `working/` via PassthroughFS
2. Enqueue op with full data
3. Implement `WorkingCopyRead` that reads from OpQueue
4. Record compares OpQueue state to pristine

**Pros:** Can track exact byte-level changes
**Cons:** More complex implementation

### Option C: Direct Change Creation (Advanced)

1. Write to `working/` via PassthroughFS
2. Enqueue op with full data
3. Directly construct `Change` structs from op data
4. Apply changes without using record()

**Pros:** Full control over change granularity
**Cons:** Need deep understanding of Pijul's change format

## Recommended Approach for Ize v1

Use **Option A** with enhancements:

1. PassthroughFS writes to `working/`
2. OpQueue stores minimal metadata (path, op type, timestamp)
3. Batch ops by time window (e.g., 100ms)
4. Call `RecordBuilder::record()` for batched paths
5. Apply resulting change to pristine

This leverages existing Pijul infrastructure while allowing future optimization.

## Key Files in vendor/pijul

| File | Lines | Purpose |
|------|-------|---------|
| `libpijul/src/record.rs` | ~2000 | Change recording logic |
| `libpijul/src/output/output.rs` | ~800 | Working copy output |
| `libpijul/src/output/mod.rs` | ~300 | output_file function |
| `libpijul/src/alive/retrieve.rs` | ~130 | Content graph retrieval |
| `libpijul/src/alive/output.rs` | ~280 | Graph to buffer output |
| `libpijul/src/fs.rs` | ~900 | Inode/path tracking |
| `libpijul/src/working_copy/mod.rs` | ~90 | WorkingCopy trait |
| `libpijul/src/working_copy/filesystem.rs` | ~500 | FileSystem implementation |
| `pijul/src/commands/record.rs` | ~450 | CLI record command |
| `pijul/src/commands/reset.rs` | ~300 | CLI reset command |