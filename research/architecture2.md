# Ize Architecture

Ize transparently versions file operations into a Pijul backend.

## Configuration

```
~/.config/ize/config.toml
```

```toml
central-dir = "~/.local/share/ize"
```

## Directory Structure

```
~/.local/share/ize/
├── config.toml              # Global Ize configuration
└── projects/
    └── {project-uuid}/
        ├── .pijul/          # Pijul repository (source of truth)
        │   ├── pristine/    # Sanakirja database
        │   ├── changes/     # Patch storage
        │   └── config       # Pijul config
        ├── working/         # The actual files (Pijul working copy)
        └── meta/
            └── project.toml # Project metadata (name, mount path, etc.)
```

**Mount points**: User-specified, contain no Ize data. FUSE mounts here.

## Data Flow

```
User Write              working/                 Pijul
    │                      │                       │
    ▼                      ▼                       │
┌────────┐          ┌────────────┐                 │
│  FUSE  │─────────►│   Write    │                 │
│ write  │          │   File     │                 │
└────────┘          └─────┬──────┘                 │
                          │                        │
                          ▼                        ▼
                    ┌────────────┐          ┌────────────┐
                    │   pijul    │─────────►│  .pijul/   │
                    │   record   │          │  pristine  │
                    └─────┬──────┘          └────────────┘
                          │
                          ▼
                    ┌────────────┐
                    │   Return   │
                    │  to FUSE   │
                    └────────────┘
```

**Synchronous model**: Each write blocks until committed to Pijul.

## FUSE Synchronous Operations

FUSE supports synchronous writes natively. The `write()` handler blocks until we return.

```rust
impl Filesystem for IzeFuse {
    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        // ...
        reply: ReplyWrite,
    ) {
        // 1. Write to working/
        let path = self.inode_to_path(ino);
        let real_path = self.working_dir.join(&path);
        
        let mut file = OpenOptions::new()
            .write(true)
            .open(&real_path)?;
        file.seek(SeekFrom::Start(offset as u64))?;
        let written = file.write(data)?;
        
        // 2. Commit to Pijul (blocking)
        self.pijul_record(&path, OpType::Write { offset, size: written })?;
        
        // 3. Return to caller
        reply.written(written as u32);
    }
}
```

**Key point**: FUSE `write()` is allowed to block. The kernel handles buffering and timeouts. No async complexity needed.

## Operation Types

```rust
pub enum OpType {
    Create { mode: u32 },
    Write { offset: i64, size: usize },
    Delete,
    Rename { to: String },
    Truncate { size: u64 },
    SetAttr { mode: Option<u32>, mtime: Option<u64> },
    MakeDir { mode: u32 },
    RemoveDir,
}
```

**Note**: Ops no longer contain data payloads. Data is already in `working/` when we record.

## Pijul Integration

```rust
impl IzeFuse {
    fn pijul_record(&self, path: &str, op: OpType) -> Result<()> {
        let repo = Repository::open(&self.pijul_dir)?;
        let mut txn = repo.pristine.mut_txn_begin()?;
        
        // Record detects changes in working copy
        let change = repo.record(
            &mut txn,
            &repo.changes,
            &self.working_dir,
            &format!("{}: {}", op, path),
        )?;
        
        // Commit
        txn.commit()?;
        Ok(())
    }
}
```

**Commit message format**:
```
{op_type}: {path}
```

Example: `Write { offset: 0, size: 1024 }: src/main.rs`

## FUSE Layer

```rust
pub struct IzeFuse {
    project_id: Uuid,
    pijul_dir: PathBuf,      // .pijul/
    working_dir: PathBuf,    // working/
    inodes: BiMap<u64, PathBuf>,
}
```

**All operations go directly to `working/`**, then record to Pijul.

| Operation | Action |
|-----------|--------|
| `read` | Read from `working/` |
| `write` | Write to `working/`, then `pijul record` |
| `create` | Create in `working/`, then `pijul record` |
| `unlink` | Delete from `working/`, then `pijul record` |
| `mkdir` | Create dir in `working/`, then `pijul record` |
| `rmdir` | Remove dir from `working/`, then `pijul record` |
| `rename` | Rename in `working/`, then `pijul record` |

## Performance Considerations

Synchronous commits are slower but:
1. **Simpler** - No overlay, no queue, no async
2. **Safer** - Every write is immediately versioned
3. **Correct** - No race conditions between dirty/working

**Future optimizations** (if needed):
- Batch rapid writes within a time window (e.g., 100ms)
- Use `flush()` or `fsync()` as commit points instead of every `write()`
- Background thread for Pijul commits with write-ahead log

## CLI

```bash
ize init <name>                    # Create new project
ize mount <name> <mountpoint>      # Mount project
ize unmount <name>                 # Unmount
ize list                           # List projects
ize history <name> [path]          # Show history
ize restore <name> <path> <change> # Restore file version
```

## Validation: FUSE Sync Behavior

From FUSE documentation and kernel behavior:

1. **`write()` can block** - Kernel expects this, handles timeouts via `conn.time_gran`
2. **No data loss** - Kernel buffers writes, retries on EAGAIN
3. **Ordering guaranteed** - FUSE serializes ops to same inode by default
4. **`fsync()` semantics** - We can optionally defer commits to `fsync()` for apps that batch writes

**Default `fuser` behavior**: Single-threaded unless `spawn()` called. This serializes all ops naturally.

For multi-threaded FUSE, Pijul transactions provide isolation.

## Future Considerations

- **Multiple branches**: Separate `working-{branch}/` directories per checkout
- **Commit batching**: Group writes between `fsync()` calls
- **libpijul optimization**: Keep repo/txn open across operations
- **Content-addressed cache**: Dedup identical file content across versions