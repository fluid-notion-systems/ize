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
        ├── working/         # State after last processed op
        ├── dirty/           # Current user-visible state (latest writes)
        └── meta/
            └── project.toml # Project metadata (name, mount path, etc.)
```

**Mount points**: User-specified, contain no Ize data. FUSE mounts here.

## Data Flow

```
User Write                    OpQueue                      Pijul
    │                            │                           │
    ▼                            │                           │
┌────────┐                       │                           │
│ dirty/ │◄──write───────────────┤                           │
└────────┘                       │                           │
    │                            ▼                           │
    │                     ┌────────────┐                     │
    │                     │  Process   │                     │
    │                     │    Op      │                     │
    │                     └─────┬──────┘                     │
    │                           │                            │
    │                           ▼                            ▼
    │                     ┌────────────┐              ┌────────────┐
    │                     │   Commit   │─────────────►│  .pijul/   │
    │                     └─────┬──────┘              └────────────┘
    │                           │
    │                           ▼
    │                     ┌────────────┐
    └────sync────────────►│ working/   │
                          └────────────┘
```

## OverlayFS Model

`working/` and `dirty/` form a logical overlay:

| Read Path | Write Path |
|-----------|------------|
| Check `dirty/` first | Always write to `dirty/` |
| Fall back to `working/` | Never write to `working/` directly |

**Consideration**: Investigate kernel OverlayFS (`overlay` mount type) with:
- `lowerdir=working/`
- `upperdir=dirty/`
- `workdir=.overlay-work/`

This would require pre-mount FD access to `working/` for Pijul operations.

## Operation Queue

Each op is **self-contained** with full data. We cannot reference `dirty/` when processing ops, as subsequent writes may have already modified those files.

```rust
pub enum OpType {
    Create { data: Vec<u8>, mode: u32 },
    Write { offset: i64, data: Vec<u8> },
    Delete,
    Rename { from: String, to: String },
    Truncate { size: u64 },
    SetAttr { mode: Option<u32>, mtime: Option<u64> },
    MakeDir { mode: u32 },
    RemoveDir,
}

pub struct Op {
    id: u64,
    op_type: OpType,
    path: String,
    timestamp: u64,
}
```

**Storage**: In-memory for now. Future: persist to `meta/opqueue/` (ops contain full data, so this could get large - consider content-addressed blob storage).

**Processing**:
1. Pop op from queue
2. Apply op data directly to `working/` (op contains all necessary data)
3. `pijul record` with message: `{op_id:x} {op.to_string()}`
4. Commit to Pijul

**Note**: `dirty/` reflects the latest user-visible state. `working/` is updated by op processing. When queue is empty, `dirty/` and `working/` should be identical.

## Pijul Integration

Use `libpijul` directly:

```rust
// Conceptual API usage
let repo = Repository::open(&project_path.join(".pijul"))?;
let txn = repo.pristine.mut_txn_begin()?;

// Record changes
let changes = repo.record(
    &mut txn,
    &repo.changes,
    &working_copy,
    &message,
)?;

// Apply to working copy
repo.output_repository(&txn, &changes, &working_path)?;

txn.commit()?;
```

**Commit message format**:
```
{op_id:016x}
{op_type}: {path}
```

## FUSE Layer

```rust
pub struct IzeFuse {
    project_id: Uuid,
    working_dir: PathBuf,
    dirty_dir: PathBuf,
    op_queue: OpQueue,
    dirty_refs: DashMap<PathBuf, u32>,
}
```

**Read operations**: Check `dirty/`, fall back to `working/`.

**Write operations**: 
1. Write to `dirty/`
2. Enqueue op
3. Return immediately (async commit)

## CLI

```bash
ize init <name>                    # Create new project
ize mount <name> <mountpoint>      # Mount project
ize unmount <name>                 # Unmount
ize list                           # List projects
ize history <name> [path]          # Show history
ize restore <name> <path> <change> # Restore file version
ize status <name>                  # Show dirty files / pending ops
```

## Future Considerations

- **Multiple branches**: Separate `working-{branch}/` directories per checkout
- **Kernel OverlayFS**: Performance optimization for read path
- **Op batching**: Coalesce rapid writes before commit
- **Content-addressed dirty/**: Hash-based dedup for large files
- **libpijul direct integration**: Skip CLI, use library API