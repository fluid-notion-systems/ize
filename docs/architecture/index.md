# ize-lib Architecture Overview

> **Purpose**: Minimal-context reference for understanding the `ize-lib` crate.
> Point an LLM or contributor here before working on the codebase.

## One-Sentence Summary

`ize-lib` interposes a FUSE filesystem between the user and their files, transparently capturing every mutation as a replayable **opcode**, which is then applied to a pluggable version-control backend (currently Pijul).

---

## Data Flow

```text
User I/O
   │
   ▼
┌──────────────────────────────────────────────────────┐
│  FUSE kernel module                                  │
└────────────────────┬─────────────────────────────────┘
                     │
                     ▼
┌──────────────────────────────────────────────────────┐
│  ObservingFS<PassthroughFS>        [filesystems]     │
│  ├─ delegates I/O to PassthroughFS (real fs ops)     │
│  └─ notifies Vec<Arc<dyn FsObserver>> on mutations   │
└────────────────────┬─────────────────────────────────┘
                     │  FsObserver callbacks
                     ▼
┌──────────────────────────────────────────────────────┐
│  OpcodeRecorder                    [operations]      │
│  ├─ translates inodes → paths (via shared InodeMap)  │
│  ├─ builds Opcode(seq, timestamp, Operation)         │
│  └─ enqueues onto OpcodeQueue via OpcodeSender       │
└────────────────────┬─────────────────────────────────┘
                     │  Opcode stream
                     ▼
┌──────────────────────────────────────────────────────┐
│  OpcodeQueue                       [operations]      │
│  (bounded, thread-safe VecDeque + Condvar)           │
└────────────────────┬─────────────────────────────────┘
                     │  consumer pops opcodes
                     ▼
┌──────────────────────────────────────────────────────┐
│  OpcodeRecordingBackend            [pijul]           │
│  ├─ maps Operation variants → PijulBackend methods   │
│  └─ returns Option<Hash> per applied change          │
└────────────────────┬─────────────────────────────────┘
                     │
                     ▼
┌──────────────────────────────────────────────────────┐
│  PijulBackend                      [pijul]           │
│  (wraps libpijul: pristine db, change store,         │
│   channels, diff-and-record workflow)                │
└──────────────────────────────────────────────────────┘
```

---

## Module Map

| Module | Path | Role |
|---|---|---|
| **filesystems** | `src/filesystems/` | FUSE filesystem layer: passthrough I/O + observer pattern |
| **operations** | `src/operations/` | Opcode model, thread-safe queue, and observer→opcode bridge |
| **pijul** | `src/pijul/` | Pijul VCS backend: repository management + opcode replay |
| **project** | `src/project/` | Project lifecycle (init/open) and multi-project management |
| **cli** | `src/cli/` | Clap command definitions (consumed by the binary crate) |
| **storage** | `src/storage/` | Placeholder `Storage` trait — not yet implemented |

---

## Module Details

### `filesystems` — FUSE Layer

**`src/filesystems/passthrough.rs`**

| Item | Kind | Description |
|---|---|---|
| `PassthroughFS` | struct | Core FUSE filesystem. Maps a `source_dir` onto a `mount_point`. Uses **real inodes** from the underlying FS, generated file handles (not raw fds), and RAII-based fd lifecycle. Maintains a shared `InodeMap` (`Arc<RwLock<HashMap<u64, PathBuf>>>`) populated during `lookup()`/`readdir()`. Supports read-only mode. |
| `InodeMap` | type alias | `Arc<RwLock<HashMap<u64, PathBuf>>>` — shared inode→relative-path mapping, consumed by `OpcodeRecorder` for path resolution. |
| `FileHandle` | struct (private) | Holds an open `File`, its real path, and open flags. Dropped on `release()`. |

Key `impl Filesystem` methods: `lookup`, `getattr`, `setattr`, `readdir`, `open`, `read`, `write`, `create`, `mkdir`, `unlink`, `rmdir`, `rename`, `access`, `statfs`, `flush`, `release`, `fsync`.

**`src/filesystems/observing.rs`**

| Item | Kind | Description |
|---|---|---|
| `FsObserver` | trait (`Send + Sync`) | Callback interface for filesystem mutations. Methods: `on_write`, `on_create`, `on_unlink`, `on_mkdir`, `on_rmdir`, `on_rename`, `on_setattr`, `on_symlink`, `on_link`. All have default no-op impls so observers opt-in to events they care about. |
| `ObservingFS<F: Filesystem>` | struct | Decorator wrapping any `Filesystem`. Holds `inner: F` and `observers: Vec<Arc<dyn FsObserver>>`. For mutations, notifies all observers *before* delegating to `inner`. Read-only ops pass straight through. |

**`src/filesystems/error.rs`**

| Item | Kind | Description |
|---|---|---|
| `FsError` | enum | Typed filesystem errors (Io, InvalidPath, PathNotFound, PermissionDenied, ReadOnlyFs, InodeNotFound, etc.). |
| `FsErrorCode` | trait | Converts `FsError` → `libc` errno for FUSE replies. |
| `IoErrorExt` | trait | Extension on `io::Error` to convert into `FsError` using path context. |
| `FsResult<T>` | type alias | `Result<T, FsError>` |

---

### `operations` — Opcode System

**`src/operations/opcode.rs`**

| Item | Kind | Description |
|---|---|---|
| `Opcode` | struct | A single captured mutation: `seq: u64` (monotonic), `timestamp: u64` (nanos since epoch), `op: Operation`. Immutable, append-only, self-contained. |
| `Operation` | enum (14 variants) | The specific mutation. **File ops**: `FileCreate`, `FileWrite`, `FileTruncate`, `FileDelete`, `FileRename`. **Dir ops**: `DirCreate`, `DirDelete`, `DirRename`. **Metadata ops**: `SetPermissions`, `SetTimestamps`, `SetOwnership`. **Link ops**: `SymlinkCreate`, `SymlinkDelete`, `HardLinkCreate`. |

`Operation` helpers: `path()`, `affects_path()`, `is_file_op()`, `is_dir_op()`, `is_metadata_op()`, `is_link_op()`, `modifies_content()`, `is_destructive()`.

Design principles: paths are always **relative** to the working root (inodes are ephemeral); operations on different paths are commutative; same-path operations must be applied in sequence order.

**`src/operations/queue.rs`**

| Item | Kind | Description |
|---|---|---|
| `OpcodeQueue` | struct | Thread-safe bounded queue (`Mutex<VecDeque<Opcode>>` + `Condvar`). Default capacity 10,000. `try_push()` enforces capacity; `push()` allows overflow. `pop()` blocks; `try_pop()` / `drain()` are non-blocking. `peek_all()` clones contents for inspection. Always created behind `Arc`. |
| `OpcodeSender` | struct (`Clone`) | Clonable producer handle holding `Arc<OpcodeQueue>`. Methods: `send()`, `try_send()`, `len()`, `is_empty()`. |

**`src/operations/recorder.rs`**

| Item | Kind | Description |
|---|---|---|
| `OpcodeRecorder` | struct | Implements `FsObserver`. Bridges the filesystem layer to the opcode queue. Holds a shared `InodeMap` (from `PassthroughFS`), a `source_dir` for path resolution, an `AtomicU64` sequence counter, and an `OpcodeSender`. Each observer callback resolves inodes to paths, builds an `Operation`, wraps it in an `Opcode`, and enqueues via `try_send()` (with a log warning on backpressure). |

---

### `pijul` — Version Control Backend

**`src/pijul/backend.rs`**

| Item | Kind | Description |
|---|---|---|
| `PijulBackend` | struct | Wraps `libpijul`. Owns `pijul_dir`, `working_dir`, `pristine: Pristine`, `current_channel: String`. Uses Ize's custom layout where `.pijul/` and `working/` are siblings (unlike standard Pijul where `.pijul/` is inside the working dir). |
| `PijulError` | enum | Typed errors: Sanakirja, Io, NotInitialized, AlreadyExists, ChannelNotFound, Transaction, ChangeStore, Fork, FileNotFound, Recording, Diff, PathConversion. |

Key methods:

- **Lifecycle**: `init()` (creates pristine db, changes dir, config, default channel), `open()`.
- **Channel management**: `create_channel()`, `switch_channel()`, `list_channels()`, `fork_channel()`.
- **Recording**: `record_file_create()`, `record_file_write()`, `record_file_truncate()`, `record_file_delete()`, `record_file_rename()`. Each mutates the working copy on disk then runs `diff_and_record()` to produce a Pijul change.
- **Queries**: `get_file_content()`, `file_exists()`, `list_files()`, `list_changes()`.
- **Internal**: `diff_and_record()` (diff working copy against pristine, build and apply a change), `record_with_memory()` (in-memory recording variant), `load_channel_ref()`, `get_file_position()`, `get_change_store()`.

**`src/pijul/operations.rs`**

| Item | Kind | Description |
|---|---|---|
| `OpcodeRecordingBackend` | struct | Thin adapter: translates `Opcode` → `PijulBackend` method calls. `apply_opcode(&Opcode) → Result<Option<Hash>>`. Currently supports file operations (`FileCreate`, `FileWrite`, `FileTruncate`, `FileDelete`, `FileRename`); dir/metadata/link ops return `UnsupportedOperation`. |
| `OpcodeError` | enum | Pijul, Io, PathConversion, UnsupportedOperation. |

Access to inner backend via `pijul()` / `pijul_mut()` for queries and channel management.

---

### `project` — Project Lifecycle

**`src/project/mod.rs`**

| Item | Kind | Description |
|---|---|---|
| `IzeProject` | struct | Represents a single tracked directory. Fields: `project_dir`, `pijul: PijulBackend`, `meta_dir`, `source_dir`, `uuid`. |
| `ProjectError` | enum | Io, Pijul, NotFound, AlreadyExists, InvalidMetadata, TomlParse, TomlSerialize. |
| `ProjectMetadata` | struct (crate-private) | Serde model for `meta/project.toml`: `[project]` (uuid, source_dir, created) + `[pijul]` (default_channel). |

Key methods:

- `IzeProject::init(project_dir, source_dir)` — creates project layout (`{project_dir}/.pijul/`, `working/`, `meta/`), copies source contents into working dir, writes `project.toml`, inits Pijul.
- `IzeProject::open(project_dir)` — reads metadata, opens existing `PijulBackend`.
- Channel delegation: `list_channels()`, `create_channel()`, `switch_channel()`.

**`src/project/manager.rs`**

| Item | Kind | Description |
|---|---|---|
| `ProjectManager` | struct | Manages multiple projects in a central store (default: `~/.local/share/ize/projects/`). Each project gets a UUID-named subdirectory. |
| `ProjectInfo` | struct | Lightweight project summary: uuid, source_dir, project_path, created, default_channel. |

Key methods: `create_project()`, `find_by_source_dir()`, `find_by_uuid()`, `list_projects()`, `delete_project()`, `delete_project_by_uuid()`.

---

### `cli` — Command Definitions

**`src/cli/commands.rs`**

| Item | Kind | Description |
|---|---|---|
| `Cli` | struct (clap `Parser`) | Top-level CLI: `--log-level`, `--unmount-on-exit`, subcommand. |
| `Commands` | enum (clap `Subcommand`) | `Init`, `Mount`, `Unmount`, `Status`, `List`, `History`, `Restore`, `Channel`, `Remove`, `ExportPijul`. |
| `ChannelAction` | enum (clap `Subcommand`) | `Create`, `List`, `Switch`, `Fork`. |

These are pure data definitions — the binary crate consumes them.

---

### `storage` — Placeholder

**`src/storage/mod.rs`**

| Item | Kind | Description |
|---|---|---|
| `Storage` | trait | `write()`, `read()`, `delete()` — not yet implemented. |
| `StorageManager` | struct | Stub with `init()`, `is_valid()`, `open()` — all return errors/false. |

---

## On-Disk Layout

```text
~/.local/share/ize/
└── projects/
    └── {uuid}/                  # One per tracked directory
        ├── .pijul/
        │   ├── pristine/db      # Sanakirja database
        │   ├── changes/         # Pijul change files
        │   └── config
        ├── working/             # Mirror of tracked files (passthrough source)
        └── meta/
            └── project.toml     # uuid, source_dir, created, default_channel
```

## Key Dependencies

| Crate | Role |
|---|---|
| `fuser` | FUSE bindings (filesystem trait + mount) |
| `libpijul` | Pijul VCS core (pristine, changes, recording, channels) |
| `parking_lot` | Fast synchronisation primitives |
| `nix` / `libc` | Low-level POSIX syscalls (utimensat, chown, statvfs) |
| `serde` / `toml` | Project metadata serialisation |
| `clap` | CLI argument parsing |
| `uuid` / `chrono` | Project identification and timestamps |

## Public API Surface (`lib.rs` re-exports)

```text
ize_lib::PijulBackend
ize_lib::PijulError
ize_lib::OpcodeRecordingBackend
ize_lib::OpcodeError
ize_lib::IzeProject
ize_lib::ProjectError
ize_lib::ProjectInfo
ize_lib::ProjectManager
```

## Design Notes

1. **Observer, not interceptor** — `ObservingFS` does *not* fan-out I/O. The real operation happens once in `PassthroughFS`; observers only receive notification data.
2. **Inode→path translation is deferred** — `PassthroughFS` populates `InodeMap` lazily during `lookup()`/`readdir()`. `OpcodeRecorder` reads it at notification time. If an inode can't be resolved, the opcode is silently skipped (with a log warning).
3. **Queue backpressure** — `OpcodeQueue` has a soft 10k capacity. `try_push` fails at capacity; the recorder logs a warning but does not block the FUSE thread.
4. **Backend-agnostic opcodes** — `Operation` is VCS-agnostic. Only `OpcodeRecordingBackend` (and future adapters for git/jj) know how to replay them.
5. **Custom Pijul layout** — Ize places `.pijul/` and `working/` as siblings rather than nesting `.pijul/` inside the working directory, enabling clean FUSE mount semantics.