# PijulBackend and OpcodeRecordingBackend Architecture Rework

## Executive Summary

Current architecture has **significant overlap and confused responsibilities** between `PijulBackend` and `OpcodeRecordingBackend`. Both directly interact with Pijul's pristine database and change store, leading to:

- Duplicate code for transaction management
- Unclear ownership of Pijul resources
- Difficulty testing Pijul integration in isolation
- Hard-coded coupling between opcode processing and Pijul internals

**Recommendation:** Complete separation where `PijulBackend` is the **only** interface to Pijul, and `OpcodeRecordingBackend` becomes a thin adapter that translates opcodes into `PijulBackend` API calls.

---

## Current Architecture Problems

### Problem 1: Duplicate Pijul Resource Management

**Both structs own the same resources:**

```rust
// PijulBackend (pijul/mod.rs)
pub struct PijulBackend {
    pijul_dir: PathBuf,
    working_dir: PathBuf,
    pristine: Pristine,        // ← Owns pristine DB
    current_channel: String,
}

// OpcodeRecordingBackend (pijul/operations.rs)
pub struct OpcodeRecordingBackend {
    pijul_dir: PathBuf,
    working_dir: PathBuf,
    pristine: Pristine,        // ← ALSO owns pristine DB!
    changes: ChangeStore,      // ← ALSO owns change store!
    current_channel: String,
}
```

**Problem:** Two separate `Pristine` instances can't safely share the same database file. This leads to:
- Potential corruption if both modify simultaneously
- Impossible to share transaction context
- Resource leaks (double file handles)

---

### Problem 2: Overlapping Initialization Code

**Both implement `init()` and `open()`:**

```rust
// PijulBackend::init()
pub fn init(pijul_dir: &Path, working_dir: &Path, channel: Option<&str>) {
    std::fs::create_dir_all(&pristine_dir)?;
    std::fs::create_dir_all(&changes_dir)?;
    let pristine = Pristine::new(&db_path)?;
    // Create default channel...
}

// OpcodeRecordingBackend::init()
pub fn init(pijul_dir: &Path, working_dir: &Path, channel: Option<&str>, cache_size: usize) {
    std::fs::create_dir_all(&pristine_dir)?;
    std::fs::create_dir_all(&changes_dir)?;
    let pristine = Pristine::new(&db_path)?;
    // Create change store...
}
```

**Problem:** Duplicate initialization logic that can diverge. Which one is correct? Who's responsible?

---

### Problem 3: OpcodeRecordingBackend Does Too Much

**Current responsibilities of OpcodeRecordingBackend:**

1. ✅ Translate opcodes to operations (CORRECT - this is its job)
2. ❌ Direct pristine DB access and transaction management (WRONG - PijulBackend's job)
3. ❌ Create change store instances (WRONG - PijulBackend's job)
4. ❌ Execute Pijul record/apply operations (WRONG - PijulBackend's job)
5. ❌ Manage working copy interactions (WRONG - PijulBackend's job)

**It should only do:**
- Consume opcodes from queue
- Call high-level PijulBackend methods

---

### Problem 4: No Clear API Boundary

**OpcodeRecordingBackend directly uses libpijul:**

```rust
impl OpcodeRecordingBackend {
    fn diff_and_record(&self, ...) {
        let txn = self.pristine.arc_txn_begin()?;  // Direct DB access
        let channel = self.load_channel(&txn)?;    // Direct channel ops
        
        // Direct diff operations
        recorded.diff(&self.changes, &txn, &channel, ...)?;
        
        // Direct change creation
        let change = recorded.into_change(&*t, &channel, header)?;
        
        // Direct change store operations
        let hash = self.changes.save_change(&mut change, ...)?;
        
        // Direct pristine operations
        libpijul::apply::apply_local_change(&mut *t, &channel, &change, ...)?;
    }
}
```

**Problem:** This should all go through `PijulBackend` methods. OpcodeRecordingBackend shouldn't know about transactions, channels, or change stores.

---

### Problem 5: Testing Nightmare

**Can't test opcode processing without full Pijul setup:**

```rust
#[test]
fn test_file_create() {
    // Must initialize BOTH backends
    let pijul_backend = PijulBackend::init(...)?;
    let opcode_backend = OpcodeRecordingBackend::init(...)?;  // Redundant!
    
    // Can't query PijulBackend properly
    // Can only count change files, not verify actual state
}
```

**Should be:**

```rust
#[test]
fn test_file_create() {
    let temp = TempDir::new().unwrap();
    let mut pijul = PijulBackend::init(...).unwrap();
    let mut opcode_backend = OpcodeRecordingBackend::new(pijul);
    
    opcode_backend.apply_opcode(create_opcode).unwrap();
    
    // Verify through PijulBackend's query API
    assert!(opcode_backend.pijul().file_exists("test.txt").unwrap());
    assert_eq!(opcode_backend.pijul().get_file_content("test.txt").unwrap(), b"content");
}
```

---

## Proposed Architecture

### Clean Separation of Concerns

```
┌─────────────────────────────────────────────────────────────┐
│                     Application Layer                        │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────┐
│                    OpcodeRecordingBackend                    │
│  Responsibilities:                                           │
│  - Consume opcodes from queue                                │
│  - Translate Operation enum → PijulBackend method calls      │
│  - Handle opcode-level retry/error logic                     │
│  - NO direct Pijul access                                    │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ↓ calls methods on
┌─────────────────────────────────────────────────────────────┐
│                       PijulBackend                           │
│  Responsibilities:                                           │
│  - ONLY interface to libpijul                                │
│  - Owns pristine DB, change store, working copy              │
│  - Transaction management                                    │
│  - Channel operations                                        │
│  - Record changes (high-level)                               │
│  - Query file history                                        │
│  - Provide read views into Pijul state                       │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ↓ uses
┌─────────────────────────────────────────────────────────────┐
│                        libpijul                              │
│  - Pristine DB                                               │
│  - Change store                                              │
│  - Working copy                                              │
│  - Diff algorithms                                           │
└─────────────────────────────────────────────────────────────┘
```

---

## Detailed Refactoring Plan

### Phase 1: Define PijulBackend High-Level API

**Add methods that OpcodeRecordingBackend needs:**

```rust
impl PijulBackend {
    // === File Operations (for opcode processing) ===
    
    /// Record creation of a new file
    pub fn record_file_create(
        &mut self,
        path: &str,
        mode: u32,
        content: &[u8],
        message: &str,
    ) -> Result<Hash, PijulError>;
    
    /// Record modification to existing file
    pub fn record_file_write(
        &mut self,
        path: &str,
        offset: u64,
        data: &[u8],
        message: &str,
    ) -> Result<Hash, PijulError>;
    
    /// Record file truncation
    pub fn record_file_truncate(
        &mut self,
        path: &str,
        new_size: u64,
        message: &str,
    ) -> Result<Hash, PijulError>;
    
    /// Record file deletion
    pub fn record_file_delete(
        &mut self,
        path: &str,
        message: &str,
    ) -> Result<Hash, PijulError>;
    
    /// Record file rename
    pub fn record_file_rename(
        &mut self,
        old_path: &str,
        new_path: &str,
        message: &str,
    ) -> Result<Hash, PijulError>;
    
    // === Query Operations (for reading Pijul state) ===
    
    /// Get file content at current channel head
    pub fn get_file_content(&self, path: &str) -> Result<Vec<u8>, PijulError>;
    
    /// Check if file exists in current channel
    pub fn file_exists(&self, path: &str) -> Result<bool, PijulError>;
    
    /// List all files in current channel
    pub fn list_files(&self) -> Result<Vec<String>, PijulError>;
    
    /// Get change history for a file
    pub fn get_file_history(&self, path: &str) -> Result<Vec<ChangeInfo>, PijulError>;
    
    /// Get details of a specific change
    pub fn get_change(&self, hash: &Hash) -> Result<ChangeDetails, PijulError>;
    
    /// List all changes in current channel
    pub fn list_changes(&self) -> Result<Vec<Hash>, PijulError>;
    
    // === Internal helpers (private) ===
    
    fn begin_transaction(&self) -> Result<ArcTxn<MutTxn<()>>, PijulError>;
    fn load_channel(&self, txn: &Txn) -> Result<ChannelRef, PijulError>;
    fn diff_and_apply(&mut self, ...) -> Result<Hash, PijulError>;
}
```

**Key points:**
- All Pijul operations go through these methods
- No direct pristine/change store access from outside
- Clean, testable API surface
- Methods return domain types (Hash, ChangeInfo), not libpijul internals

---

### Phase 2: Simplify OpcodeRecordingBackend

**Remove all Pijul internals:**

```rust
pub struct OpcodeRecordingBackend {
    /// The underlying Pijul backend (our only interface to Pijul)
    pijul: PijulBackend,  // ← NOT Arc<Mutex<...>>, just owned
}

impl OpcodeRecordingBackend {
    /// Create from existing PijulBackend
    pub fn new(pijul: PijulBackend) -> Self {
        Self { pijul }
    }
    
    /// Apply a single opcode by calling appropriate PijulBackend method
    pub fn apply_opcode(&mut self, opcode: &Opcode) -> Result<Hash, OpcodeError> {
        let message = format!("Opcode #{}", opcode.seq());
        
        match opcode.op() {
            Operation::FileCreate { path, mode, content } => {
                self.pijul.record_file_create(
                    path.to_str().unwrap(),
                    *mode,
                    content,
                    &message,
                )
                .map_err(OpcodeError::from)
            }
            
            Operation::FileWrite { path, offset, data } => {
                self.pijul.record_file_write(
                    path.to_str().unwrap(),
                    *offset,
                    data,
                    &message,
                )
                .map_err(OpcodeError::from)
            }
            
            Operation::FileTruncate { path, new_size } => {
                self.pijul.record_file_truncate(
                    path.to_str().unwrap(),
                    *new_size,
                    &message,
                )
                .map_err(OpcodeError::from)
            }
            
            Operation::FileDelete { path } => {
                self.pijul.record_file_delete(
                    path.to_str().unwrap(),
                    &message,
                )
                .map_err(OpcodeError::from)
            }
            
            Operation::FileRename { old_path, new_path } => {
                self.pijul.record_file_rename(
                    old_path.to_str().unwrap(),
                    new_path.to_str().unwrap(),
                    &message,
                )
                .map_err(OpcodeError::from)
            }
            
            _ => Err(OpcodeError::UnsupportedOperation(
                format!("{:?}", opcode.op())
            )),
        }
    }
    
    /// Get reference to underlying PijulBackend for queries
    pub fn pijul(&self) -> &PijulBackend {
        &self.pijul
    }
}
```

**Result:**
- ~50 lines instead of ~700 lines
- Zero direct Pijul interaction
- Easy to test with mock PijulBackend
- Clear single responsibility: opcode → method call translation

---

### Phase 3: Move Complex Logic to PijulBackend

**All the diffing/recording logic moves into PijulBackend:**

```rust
impl PijulBackend {
    pub fn record_file_create(
        &mut self,
        path: &str,
        mode: u32,
        content: &[u8],
        message: &str,
    ) -> Result<Hash, PijulError> {
        // All the current OpcodeRecordingBackend complexity goes here:
        let txn = self.pristine.arc_txn_begin()?;
        let channel = self.load_channel(&txn)?;
        
        // Add file to tree
        {
            let mut t = txn.write();
            t.add_file(path, mode)?;
        }
        
        // Use Memory working copy for new file
        let mut working_copy = Memory::new();
        working_copy.add_file(path, content.to_vec());
        
        // Record change
        let mut builder = RecordBuilder::new();
        builder.record(
            &mut txn,
            Algorithm::default(),
            false,
            &channel,
            &mut working_copy,
            &self.changes,
            "",
        )?;
        
        let recorded = builder.finish();
        let header = ChangeHeader {
            message: message.to_string(),
            authors: vec![],
            description: None,
            timestamp: jiff::Timestamp::now(),
        };
        
        let mut change = recorded.into_change(&txn, &channel, header)?;
        let hash = self.changes.save_change(&mut change)?;
        
        // Apply to pristine
        {
            let mut t = txn.write();
            libpijul::apply::apply_local_change(&mut *t, &channel, &change, &hash)?;
        }
        
        txn.commit()?;
        Ok(hash)
    }
    
    // Similar for other operations...
}
```

**Benefits:**
- PijulBackend is the expert on Pijul operations
- Can optimize/refactor Pijul usage without touching OpcodeRecordingBackend
- Easier to add features like conflict resolution, merge strategies
- Single source of truth for "how to record to Pijul"

---

## Testing Strategy After Rework

### Testing OpcodeRecordingBackend with Real PijulBackend

**Note:** We use real `PijulBackend` instances in tests, not mocks. Pijul has memory-based working copy support that makes testing reasonably fast without needing mock infrastructure.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    fn setup() -> (TempDir, PijulBackend) {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");
        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();
        (temp, backend)
    }
    
    #[test]
    fn test_file_create_opcode_translation() {
        let (_temp, pijul) = setup();
        let mut backend = OpcodeRecordingBackend::new(pijul);
        
        let opcode = Opcode::new(1, Operation::FileCreate {
            path: PathBuf::from("test.txt"),
            mode: 0o644,
            content: b"Hello, world!".to_vec(),
        });
        
        let result = backend.apply_opcode(&opcode);
        assert!(result.is_ok(), "Failed to apply opcode: {:?}", result.err());
        
        let hash = result.unwrap();
        
        // VERIFY: Through PijulBackend's query API
        assert!(backend.pijul().file_exists("test.txt").unwrap(),
            "File should exist in Pijul");
        
        let content = backend.pijul().get_file_content("test.txt").unwrap();
        assert_eq!(content, b"Hello, world!",
            "File content should match what was recorded");
        
        // VERIFY: Change was recorded
        let changes = backend.pijul().list_changes().unwrap();
        assert_eq!(changes.len(), 1, "Should have exactly 1 change");
        assert_eq!(changes[0], hash, "Change hash should match");
    }
    
    #[test]
    fn test_file_write_opcode_translation() {
        let (_temp, pijul) = setup();
        let mut backend = OpcodeRecordingBackend::new(pijul);
        
        // Create file first
        let create_opcode = Opcode::new(1, Operation::FileCreate {
            path: PathBuf::from("test.txt"),
            mode: 0o644,
            content: b"Initial".to_vec(),
        });
        backend.apply_opcode(&create_opcode).unwrap();
        
        // Write to it
        let write_opcode = Opcode::new(2, Operation::FileWrite {
            path: PathBuf::from("test.txt"),
            offset: 7,
            data: b" content".to_vec(),
        });
        backend.apply_opcode(&write_opcode).unwrap();
        
        // VERIFY: Content was updated
        let content = backend.pijul().get_file_content("test.txt").unwrap();
        assert_eq!(content, b"Initial content");
        
        // VERIFY: Two changes recorded
        let changes = backend.pijul().list_changes().unwrap();
        assert_eq!(changes.len(), 2);
    }
    
    #[test]
    fn test_unsupported_operation_returns_error() {
        let (_temp, pijul) = setup();
        let mut backend = OpcodeRecordingBackend::new(pijul);
        
        let opcode = Opcode::new(1, Operation::DirCreate {
            path: PathBuf::from("dir"),
            mode: 0o755,
        });
        
        let result = backend.apply_opcode(&opcode);
        assert!(matches!(result, Err(OpcodeError::UnsupportedOperation(_))),
            "Should return UnsupportedOperation error");
    }
}
```

**Benefits:**
- Tests actual behavior, not mocked expectations
- Verifies real Pijul state through query API
- Catches integration bugs that mocks would miss
- Still reasonably fast (using TempDir, cleaned up automatically)
- No mock maintenance burden

---

### Integration Testing PijulBackend (Thorough!)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    fn setup() -> (TempDir, PijulBackend) {
        let temp = TempDir::new().unwrap();
        let pijul_dir = temp.path().join(".pijul");
        let working_dir = temp.path().join("working");
        let backend = PijulBackend::init(&pijul_dir, &working_dir, None).unwrap();
        (temp, backend)
    }
    
    #[test]
    fn test_record_and_retrieve_file() {
        let (_temp, mut backend) = setup();
        
        // Record file creation
        let hash = backend.record_file_create(
            "test.txt",
            0o644,
            b"Hello, Pijul!",
            "Create test file"
        ).unwrap();
        
        // VERIFY: File can be retrieved
        let content = backend.get_file_content("test.txt").unwrap();
        assert_eq!(content, b"Hello, Pijul!");
        
        // VERIFY: File exists
        assert!(backend.file_exists("test.txt").unwrap());
        
        // VERIFY: Change exists
        let changes = backend.list_changes().unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0], hash);
        
        // VERIFY: Can get change details
        let change = backend.get_change(&hash).unwrap();
        assert_eq!(change.message, "Create test file");
    }
    
    #[test]
    fn test_file_modification_sequence() {
        let (_temp, mut backend) = setup();
        
        // Create
        let hash1 = backend.record_file_create(
            "file.txt",
            0o644,
            b"v1",
            "initial"
        ).unwrap();
        
        // Modify
        let hash2 = backend.record_file_write(
            "file.txt",
            0,
            b"v2",
            "update"
        ).unwrap();
        
        // Truncate
        let hash3 = backend.record_file_truncate(
            "file.txt",
            1,
            "truncate"
        ).unwrap();
        
        // VERIFY: Current content
        assert_eq!(backend.get_file_content("file.txt").unwrap(), b"v");
        
        // VERIFY: Change history
        let changes = backend.list_changes().unwrap();
        assert_eq!(changes, vec![hash1, hash2, hash3]);
        
        // VERIFY: File history
        let history = backend.get_file_history("file.txt").unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].hash, hash1);
        assert_eq!(history[1].hash, hash2);
        assert_eq!(history[2].hash, hash3);
    }
}
```

**Benefits:**
- Tests actual Pijul behavior
- Verifies end-to-end recording works
- Can test complex scenarios (conflicts, merges)
- Validates query methods work correctly

---

## Benefits of Refactored Architecture

### 1. Clear Ownership
- **PijulBackend** owns all Pijul resources (pristine, changes, working copy)
- **OpcodeRecordingBackend** owns translation logic only
- No shared mutable state
- No resource conflicts

### 2. Testability
- **OpcodeRecordingBackend:** Unit tests with mocks (fast, isolated)
- **PijulBackend:** Integration tests with real Pijul (thorough, complete)
- Clear test boundaries
- Easy to achieve >90% coverage

### 3. Maintainability
- Each component has single responsibility
- Changes to Pijul usage isolated to PijulBackend
- Changes to opcode handling isolated to OpcodeRecordingBackend
- Easier to understand and debug

### 4. Extensibility
- Add new opcode types: just add case to match in OpcodeRecordingBackend
- Add new Pijul features: just add methods to PijulBackend
- Add new query capabilities: just add query methods to PijulBackend
- Easy to add alternative backends (Git, custom)

### 5. Performance
- Single Pristine instance (no duplicate handles)
- Can optimize transaction batching in PijulBackend
- Can cache query results in PijulBackend
- Clear boundary for performance profiling

---

## Migration Path

### Step 1: Add PijulBackend High-Level Methods (1 week)
- Add `record_file_create()`, `record_file_write()`, etc.
- Move complexity from OpcodeRecordingBackend
- Keep old methods working (don't break anything)

### Step 2: Update OpcodeRecordingBackend to Use New API (1 week)
- Change `apply_opcode()` to call PijulBackend methods
- Remove direct pristine/change store access
- Keep tests passing

### Step 3: Remove Duplicate Resources (1 week)
- Remove pristine, changes fields from OpcodeRecordingBackend
- Update all construction code
- Update all tests

### Step 4: Add Comprehensive Tests (1 week)
- Add mock-based unit tests for OpcodeRecordingBackend
- Add integration tests for PijulBackend high-level methods
- Achieve >80% coverage on both

### Step 5: Cleanup (1 week)
- Remove dead code
- Improve documentation
- Add architectural decision records

**Total Estimated Time:** 5 weeks

---

## Alternative Architectures Considered

### Alternative 1: Trait-Based Abstraction

```rust
pub trait VcsBackend {
    fn record_file_create(&mut self, path: &str, content: &[u8]) -> Result<ChangeHash>;
    fn record_file_write(&mut self, path: &str, data: &[u8]) -> Result<ChangeHash>;
    // ...
}

impl VcsBackend for PijulBackend { /* ... */ }

pub struct OpcodeRecordingBackend<B: VcsBackend> {
    backend: B,
}
```

**Pros:**
- Very flexible, can swap backends
- Could support multiple VCS backends (Git, custom, etc.)

**Cons:**
- More complex than needed right now
- Pijul is only backend we have
- Trait object overhead
- Testing would still use real implementations (not mocks)

**Verdict:** Good for future, but YAGNI right now. Start with concrete types.

---

### Alternative 2: Keep Current, Add Query Layer

```rust
pub struct PijulBackend {
    // Keep as-is
}

pub struct PijulQueryService {
    backend: Arc<PijulBackend>,
    // Query-specific caching, etc.
}

pub struct OpcodeRecordingBackend {
    // Keep as-is
}
```

**Pros:**
- Minimal changes
- Separates read from write

**Cons:**
- Doesn't solve resource duplication
- Still complex to test
- Adds another layer

**Verdict:** Doesn't solve root problem.

---

### Alternative 3: OpcodeRecordingBackend Wraps PijulBackend

```rust
pub struct OpcodeRecordingBackend {
    pijul: Arc<Mutex<PijulBackend>>,
}
```

**Pros:**
- Clear ownership hierarchy
- Forces all Pijul access through backend

**Cons:**
- Mutex overhead on every call
- Lock contention potential
- Arc complexity

**Verdict:** Close, but Arc<Mutex<>> is overkill. Just own directly.

---

## Recommended Architecture (Final)

```rust
// pijul/mod.rs
pub struct PijulBackend {
    pijul_dir: PathBuf,
    working_dir: PathBuf,
    pristine: Pristine,
    changes: ChangeStore,
    current_channel: String,
}

impl PijulBackend {
    // High-level operations (shown above)
    pub fn record_file_create(...) -> Result<Hash, PijulError>;
    pub fn record_file_write(...) -> Result<Hash, PijulError>;
    pub fn get_file_content(...) -> Result<Vec<u8>, PijulError>;
    // etc.
}

// pijul/operations.rs
pub struct OpcodeRecordingBackend {
    pijul: PijulBackend,  // Owned, not shared
}

impl OpcodeRecordingBackend {
    pub fn new(pijul: PijulBackend) -> Self;
    pub fn apply_opcode(&mut self, opcode: &Opcode) -> Result<Hash, OpcodeError>;
    pub fn pijul(&self) -> &PijulBackend;
    pub fn pijul_mut(&mut self) -> &mut PijulBackend;
}
```

**Key Principles:**
1. **Single ownership:** OpcodeRecordingBackend owns PijulBackend
2. **Single interface:** All Pijul access through PijulBackend methods
3. **Clear hierarchy:** Application → OpcodeRecording → Pijul → libpijul
4. **No sharing:** No Arc, no Mutex (unless proven necessary)
5. **High-level API:** PijulBackend exposes domain operations, not transactions

---

## Success Criteria

After refactoring, we should be able to:

1. ✅ **Test OpcodeRecordingBackend with real PijulBackend** (fast with memory-based Pijul)
2. ✅ **Integration test PijulBackend thoroughly** (verify actual Pijul state)
3. ✅ **Query Pijul state through clean API** (no direct DB access)
4. ✅ **Add new opcode types in < 10 lines** (just match case + method call)
5. ✅ **Swap Pijul implementation without touching opcodes** (clean separation)
6. ✅ **Achieve >85% test coverage** (testable through query API)
7. ✅ **Zero resource duplication** (single Pristine instance)

---

## Risks and Mitigations

### Risk 1: Breaking Existing Tests
**Mitigation:** Incremental migration, keep old code paths working until new ones proven

### Risk 2: Performance Regression
**Mitigation:** Benchmark before/after, profile hotspots, can inline later if needed

### Risk 3: Underestimating Complexity
**Mitigation:** Start with one operation (FileCreate), prove it works, then generalize

### Risk 4: Pijul API Limitations
**Mitigation:** Research libpijul docs first, prototype complex operations early

---

## Conclusion

Current architecture has PijulBackend and OpcodeRecordingBackend doing overlapping work with confused responsibilities. This makes testing nearly impossible (can only count change files, not verify actual state) and creates resource management problems.

**Recommended solution:** Complete separation where PijulBackend is the sole interface to Pijul with high-level methods, and OpcodeRecordingBackend becomes a thin translation layer that just converts opcodes to method calls.

**Benefits:** Testable (using real Pijul with query API), maintainable, extensible, performant.

**Cost:** ~5 weeks of refactoring work.

**Next step:** Prototype `PijulBackend::record_file_create()` to validate approach before committing to full refactor.