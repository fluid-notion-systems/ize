# fuse-fd-poc — Pre-opened fd FUSE Passthrough Proof-of-Concept

## Problem

When a FUSE filesystem is mounted **over the same directory** it reads from (the
"overlay" or "same-directory passthrough" pattern), any path-based syscall the
FUSE daemon makes against that directory re-enters the FUSE layer, causing
**recursive deadlock**.

This matters for ize because we want to mount a FUSE layer directly over a
user's versioned repository (git/pijul/jj), transparently capture every
mutation as an opcode, and pass the I/O through to the real files underneath —
all without requiring a separate `working/` copy of the directory.

### Two known solutions

1. **Bind-mount the directory elsewhere first**, then have the FUSE daemon read
   from the bind mount.  Works, but adds an extra mount and cleanup step.

2. **Open a directory file descriptor before mounting**, then use `*at()`
   syscalls (`openat`, `fstatat`, `mkdirat`, `unlinkat`, `renameat`, …)
   relative to that fd.  The kernel resolves the fd to the underlying
   filesystem's inode at `open()` time; subsequent `*at()` calls through it
   **never traverse the FUSE mount**.

This crate validates **option 2**.

## How it works

```text
                    BEFORE mount                       AFTER mount
                    ────────────                       ───────────

  /tmp/test-dir/          ← real ext4/btrfs dir    ← FUSE mounted HERE
       │                                                  │
       └─ seed.txt        ← real file                     │
                                                          ▼
                                                   ┌─────────────┐
                                                   │ FUSE daemon  │
                                                   │              │
  fd=3 ──────────────────────────────────────────► │ base_fd=3    │
  (opened BEFORE mount,                            │  openat()    │
   resolves to underlying                          │  fstatat()   │
   inode, bypasses FUSE)                           │  mkdirat()   │
                                                   │  unlinkat()  │
                                                   │  …           │
                                                   └─────────────┘
```

1. A directory fd is opened with `open(path, O_RDONLY | O_DIRECTORY)` **before**
   the FUSE mount is established.
2. FUSE is mounted on the same path.
3. The FUSE daemon's `Filesystem` implementation uses exclusively `*at()`
   syscalls relative to the pre-opened fd — it never constructs absolute paths
   back to the mount point.
4. The kernel resolves the fd against the **underlying** filesystem, so these
   calls never re-enter FUSE.

## What this binary does

1. Creates a temporary directory with a seed file (`seed.txt`).
2. Opens an `O_RDONLY | O_DIRECTORY` fd to it — **the critical step**.
3. Mounts a minimal FUSE passthrough filesystem on the **same** directory.
4. Verifies the fd still resolves to the underlying FS post-mount (no deadlock).
5. Runs 7 validation checks through the FUSE mount:
   - Read the pre-seeded file
   - Create a new file
   - Read it back
   - Create a subdirectory
   - Write and read a nested file
   - List the root directory
   - Remove a file
6. Unmounts and reports results.

## Running

```sh
# From the workspace root:
RUST_LOG=info cargo run -p fuse-fd-poc

# Or with a specific directory:
RUST_LOG=info cargo run -p fuse-fd-poc -- /tmp/my-test-dir

# Debug-level logging shows every FUSE operation:
RUST_LOG=debug cargo run -p fuse-fd-poc
```

### Requirements

- Linux with FUSE support (`/dev/fuse` must exist, `fuse` module loaded)
- `fusermount` or `fusermount3` available
- If `user_allow_other` is **not** set in `/etc/fuse.conf`, the binary avoids
  `AllowOther` / `AutoUnmount` options automatically

## Results

Tested on Linux 6.x with fuser 0.15.1:

```
Opened base directory fd=3 for "/tmp/fuse-fd-poc" (BEFORE mount)
  fstat(base_fd): ino=1971, mode=0o775, size=60
  Pre-mount openat read: "hello from before the mount" (OK)

Mounting FUSE on "/tmp/fuse-fd-poc" ...
  (base_fd=3 was opened BEFORE this mount — *at() calls bypass FUSE)

Verifying base_fd still resolves to underlying FS after mount...
  Post-mount openat(base_fd, "seed.txt") read: "hello from before the mount" (OK — no deadlock!)

[1/7] Reading seed file through FUSE mount...        OK
[2/7] Creating a new file through FUSE mount...      OK
[3/7] Reading back the created file...               OK
[4/7] Creating a subdirectory through FUSE mount...  OK
[5/7] Writing a nested file...                       OK
[6/7] Listing root directory through FUSE mount...   OK
[7/7] Removing the created file through FUSE mount...OK

SUCCESS: fd-based FUSE passthrough works correctly!
```

**Key finding**: The pre-opened directory fd continues to resolve to the
underlying filesystem after FUSE is mounted on top.  All `*at()` operations
through it bypass FUSE entirely.  No deadlock, no recursion.

## Implementation details

### Syscalls used

| FUSE operation | Underlying syscall         | Notes                                    |
|----------------|----------------------------|------------------------------------------|
| `lookup`       | `fstatat(base_fd, rel, …)` | Stat relative path, register inode       |
| `getattr`      | `fstatat` / `fstat`        | `fstat(base_fd)` for root                |
| `readdir`      | `openat` + `fdopendir`     | `dup()` the dir fd to avoid fdopendir stealing it |
| `open`         | `openat(base_fd, rel, …)`  | Returns fd stored in file-handle table   |
| `read`         | `pread(file_fd, …)`        | Positional read on the opened file fd    |
| `write`        | `pwrite(file_fd, …)`       | Positional write on the opened file fd   |
| `create`       | `openat` with `O_CREAT`    | + `fstat` for inode                      |
| `mkdir`        | `mkdirat(base_fd, …)`      |                                          |
| `unlink`       | `unlinkat(base_fd, …, 0)`  |                                          |
| `rmdir`        | `unlinkat(…, AT_REMOVEDIR)` |                                          |
| `rename`       | `renameat(base_fd, …)`     | Both old and new relative to base_fd     |
| `setattr`      | `ftruncate` / `openat`     | Truncate via existing fh or fresh open   |
| `access`       | `faccessat(base_fd, …)`    |                                          |
| `statfs`       | `fstatvfs(base_fd)`        |                                          |
| `flush`        | `fsync(file_fd)`           |                                          |

### File handle management

The FUSE daemon maintains a `HashMap<u64, OpenFile>` mapping FUSE file handles
to underlying raw fds.  Each `open()` / `create()` allocates a monotonic handle
ID and stores the fd returned by `openat()`.  On `release()`, the entry is
removed and the fd is closed via `Drop`.

### Inode mapping

Real inodes from the underlying filesystem are used directly.  A
`HashMap<u64, PathBuf>` maps inodes to relative paths within the mounted
directory.  This is populated lazily during `lookup()` and `readdir()`.

## Implications for ize

### Current architecture

The current `PassthroughFS` in ize-lib uses **path-based operations**
(`fs::metadata(&real_path)`, `OpenOptions::new().open(&real_path)`) against a
separate `working/` directory that lives alongside `.pijul/`.  This avoids the
recursion problem but requires maintaining a full copy of the user's files.

### Proposed change

With the fd-based approach validated here, ize could:

1. **Mount directly over the user's repository** — no separate `working/` copy needed.
2. Open a directory fd to the repo **before** mounting.
3. Replace all path-based I/O in `PassthroughFS` with `*at()` syscalls relative
   to the pre-opened fd.
4. The observer/opcode pipeline remains unchanged — it only sees inode
   notifications and doesn't care how the underlying I/O is performed.

### Fd staleness concern

The main worry with option 2 was whether the fd could "go stale".  This PoC
demonstrates that:

- The fd survives the FUSE mount being placed on top.
- It correctly resolves to the underlying filesystem for all tested operations.
- It remains valid for the entire lifetime of the FUSE session.
- Created/deleted files through the fd are immediately visible through the
  FUSE mount (since the FUSE layer reads from the same underlying storage).

The fd is tied to the **inode** of the directory, not its path.  As long as the
underlying filesystem isn't unmounted or the directory isn't deleted, the fd
remains valid.  This is a fundamental POSIX guarantee — open file descriptors
keep a reference to the inode.

### Migration path

1. Introduce an `FdPassthroughFS` (or refactor `PassthroughFS`) that accepts a
   `base_fd: RawFd` and uses `*at()` syscalls exclusively.
2. The `IzeProject` / mount setup code opens the fd before calling
   `fuser::spawn_mount2`.
3. The `working/` directory concept becomes optional — projects can be
   initialised in "overlay" mode where the FUSE layer sits directly on the
   source directory.
4. The Pijul backend continues to operate on its own `.pijul/` directory
   (which should be outside the mounted tree, or excluded via FUSE-level
   filtering).

## Architecture reference

See [`docs/architecture/index.md`](../../docs/architecture/index.md) for the
full ize-lib architecture, including the data flow from FUSE through the
observer/opcode pipeline to the Pijul backend.