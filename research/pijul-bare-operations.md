# Pijul Bare Repository Operations Research

## Executive Summary

**Can Pijul operate without a working directory?** Yes, but with caveats.

Pijul's architecture separates:
1. **Pristine** (`sanakirja` database) - The graph-based representation of file history
2. **Changes** (patch storage) - The actual change files  
3. **Working Copy** - The filesystem representation

The pristine database IS the source of truth - you can reconstruct any file state from it. However, **recording new changes** (the `record` operation) fundamentally requires comparing a working copy against the pristine state to generate diffs.

## Key Findings

### 1. Pristine is Self-Contained

The pristine database stores the complete file graph. You can:
- Read file contents at any state via `output_file()`
- Apply existing changes via `apply_change()`
- Query file/directory structure

```rust
// From libpijul/src/output module
libpijul::output::output_file(
    &changes,      // ChangeStore
    &txn,          // Transaction
    &channel,      // Channel reference
    pos,           // Position in graph
    &mut writer,   // Output destination
)?;
```

### 2. Recording Requires Working Copy Comparison

The `record` module **requires** a `WorkingCopy` trait implementation:

```rust
// From libpijul/src/record.rs
pub fn record<
    T,
    W: WorkingCopyRead + Clone + Send + Sync + 'static,  // <-- Required!
    C: ChangeStore + Clone + Send + 'static,
>(
    &mut self,
    txn: ArcTxn<T>,
    diff_algorithm: diff::Algorithm,
    // ...
    working_copy: &W,  // <-- Must provide working copy
    changes: &C,
    prefix: &str,
    // ...
) -> Result<(), RecordError<C::Error, W::Error, T>>
```

### 3. In-Memory Working Copy Exists

Pijul provides `working_copy::memory::Memory` - an in-memory implementation:

```rust
// From libpijul/src/working_copy/memory.rs
impl Memory {
    pub fn new() -> Self { /* ... */ }
    pub fn add_file(&self, file: &str, file_contents: Vec<u8>) { /* ... */ }
    pub fn add_dir(&self, file: &str) { /* ... */ }
}
```

This is used in tests to create changes programmatically without touching the filesystem.

## Architecture for Ize

### The Challenge

For Ize's use case:
1. User writes to `working/` via PassthroughFS (immediate)
2. OpQueue records operations for async persistence to `.pijul/`

The problem: By the time OpQueue processes operation N, the working directory may have changed (ops N+1, N+2, etc.). We **cannot** use the working directory to record changes.

### Solution: Virtual Working Copy Per-Operation

Instead of using the real working directory, construct a **virtual working copy** that represents the state **at the time of the operation**:

```
Op Processing Flow:
                                                     
  OpQueue receives:                                  
  ┌─────────────────────────────────────────┐       
  │ Op { path: "foo.txt",                   │       
  │      type: Write { offset: 0,           │       
  │                    data: "new content"} │       
  │      timestamp: T }                     │       
  └──────────────────┬──────────────────────┘       
                     │                              
                     ▼                              
  ┌─────────────────────────────────────────┐       
  │ 1. Get current content from pristine    │       
  │    at channel HEAD                      │       
  │    (output_file → Vec<u8>)              │       
  └──────────────────┬──────────────────────┘       
                     │                              
                     ▼                              
  ┌─────────────────────────────────────────┐       
  │ 2. Apply op to get "new" content        │       
  │    old_content[offset..] = data         │       
  └──────────────────┬──────────────────────┘       
                     │                              
                     ▼                              
  ┌─────────────────────────────────────────┐       
  │ 3. Create Memory working copy with      │       
  │    the "new" content                    │       
  └──────────────────┬──────────────────────┘       
                     │                              
                     ▼                              
  ┌─────────────────────────────────────────┐       
  │ 4. Record change (diff pristine vs      │       
  │    Memory working copy)                 │       
  └──────────────────┬──────────────────────┘       
                     │                              
                     ▼                              
  ┌─────────────────────────────────────────┐       
  │ 5. Apply change to pristine             │       
  └─────────────────────────────────────────┘       
```

### Implementation Sketch

```rust
use libpijul::working_copy::memory::Memory;
use libpijul::pristine::sanakirja::Pristine;
use libpijul::changestore::filesystem::FileSystem as ChangeStore;
use libpijul::{MutTxnTExt, TxnTExt, ArcTxn, ChannelRef};

pub struct PijulBackend {
    pristine: Pristine,
    changes: ChangeStore,
    channel_name: String,
}

impl PijulBackend {
    /// Apply an operation and create a change
    pub fn apply_op(&self, op: &Op) -> Result<Hash, Error> {
        let txn = self.pristine.arc_txn_begin()?;
        let channel = {
            let t = txn.read();
            t.load_channel(&self.channel_name)?.unwrap()
        };
        
        match &op.op_type {
            OpType::Write { offset, data } => {
                // 1. Get current file content from pristine
                let current_content = self.get_file_content(&txn, &channel, &op.path)?;
                
                // 2. Apply the write operation
                let mut new_content = current_content;
                let offset = *offset as usize;
                let end = offset + data.len();
                if end > new_content.len() {
                    new_content.resize(end, 0);
                }
                new_content[offset..end].copy_from_slice(data);
                
                // 3. Create in-memory working copy with new content
                let memory_wc = Memory::new();
                memory_wc.add_file(&op.path, new_content);
                
                // 4. Ensure file is tracked
                {
                    let mut t = txn.write();
                    if !t.is_tracked(&op.path)? {
                        t.add_file(&op.path, 0)?;
                    }
                }
                
                // 5. Record the change
                let mut builder = libpijul::RecordBuilder::new();
                builder.record(
                    txn.clone(),
                    libpijul::Algorithm::default(),
                    false,
                    &libpijul::DEFAULT_SEPARATOR,
                    channel.clone(),
                    &memory_wc,
                    &self.changes,
                    &op.path,  // prefix - only record this file
                    1,
                )?;
                
                let rec = builder.finish();
                if rec.actions.is_empty() {
                    // No changes detected (content was same)
                    return Ok(Hash::None);
                }
                
                // 6. Create and save the change
                let changes = rec.actions
                    .into_iter()
                    .map(|r| r.globalize(&*txn.read()).unwrap())
                    .collect();
                    
                let mut change = libpijul::change::Change::make_change(
                    &*txn.read(),
                    &channel,
                    changes,
                    std::mem::take(&mut *rec.contents.lock()),
                    libpijul::change::ChangeHeader {
                        message: format!("Auto: write to {}", op.path),
                        authors: vec![],
                        description: None,
                        timestamp: jiff::Timestamp::from_second(op.timestamp as i64)?,
                    },
                    Vec::new(),
                )?;
                
                let hash = self.changes.save_change(&mut change, |_, _| Ok::<_, Error>(()))?;
                
                // 7. Apply to local pristine
                libpijul::apply::apply_local_change(
                    &mut *txn.write(),
                    &channel,
                    &change,
                    &hash,
                    &rec.updatables,
                )?;
                
                txn.commit()?;
                Ok(hash)
            }
            
            OpType::Create { data, mode } => {
                // Similar pattern: create Memory wc with new file, record
                let memory_wc = Memory::new();
                memory_wc.add_file(&op.path, data.clone());
                // ... record and apply
            }
            
            OpType::Unlink => {
                // Mark file as deleted in pristine
                let mut t = txn.write();
                t.remove_file(&op.path)?;
                // Record the deletion
                // ...
            }
            
            // ... other op types
        }
    }
    
    /// Get file content from pristine at HEAD
    fn get_file_content(
        &self,
        txn: &ArcTxn<impl libpijul::MutTxnT>,
        channel: &ChannelRef<impl libpijul::MutTxnT>,
        path: &str,
    ) -> Result<Vec<u8>, Error> {
        let t = txn.read();
        let c = channel.read();
        
        // Find the file's position in the graph
        let (pos, _) = t.follow_oldest_path(&self.changes, &*c, path)?;
        
        // Output to buffer
        let mut buffer = Vec::new();
        libpijul::output::output_file(
            &self.changes,
            txn,
            channel,
            pos,
            &mut libpijul::vertex_buffer::Writer::new(&mut buffer),
        )?;
        
        Ok(buffer)
    }
}
```

## Performance Considerations

### Reading from Pristine

`output_file()` reconstructs a file by traversing the graph. For small-to-medium files, this is fast. For very large files with many changes, it may become slower.

**Mitigation:** Cache recently-read file contents keyed by (path, channel_state).

### Recording Changes

The `record` operation diffs the working copy against the pristine. With an in-memory working copy containing only the changed file(s), this should be fast.

**Optimization:** Use `prefix` parameter to scope recording to just the affected file.

### Coalescing Operations

Multiple rapid writes to the same file could be coalesced:
- Buffer ops for the same file within a time window
- Apply all buffered ops to get final content
- Record single change

## Key libpijul Types

| Type | Purpose |
|------|---------|
| `Pristine` | Sanakirja database handle |
| `ArcTxn<T>` | Thread-safe transaction wrapper |
| `ChannelRef<T>` | Reference to a branch/channel |
| `Memory` | In-memory working copy |
| `ChangeStore` | Trait for storing changes |
| `RecordBuilder` | Builder for recording changes |
| `Hash` | Change identifier |

## Key Operations

| Operation | Function |
|-----------|----------|
| Begin transaction | `pristine.arc_txn_begin()` |
| Load channel | `txn.read().load_channel(name)` |
| Track file | `txn.write().add_file(path, mode)` |
| Record change | `builder.record(...)` + `builder.finish()` |
| Save change | `changes.save_change(&mut change, ...)` |
| Apply locally | `apply_local_change(...)` |
| Output file | `output::output_file(...)` |
| Commit transaction | `txn.commit()` |

## Conclusion

The proposed architecture is **achievable and performant**:

1. ✅ **Pristine can be queried** - We can read file contents without a working directory
2. ✅ **In-memory working copy exists** - `Memory` type allows synthetic working copies
3. ✅ **Recording is scoped** - The `prefix` parameter limits recording to specific paths
4. ✅ **Changes can be applied** - `apply_local_change` updates pristine atomically

The key insight is that we don't need the *actual* working directory to record changes - we just need *something that implements `WorkingCopy`*. By constructing a virtual working copy from the op data, we can record changes that represent exactly the user's operations, regardless of what the real working directory looks like at processing time.