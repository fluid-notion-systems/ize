# VCS Backend System

The VCS backend system provides trait-based detection and filtering of version control system directories (Git, Jujutsu, Pijul) to prevent filesystem observers from recording changes inside VCS metadata directories.

## Architecture

```text
┌─────────────────────────────────────────────────────────┐
│                     User Application                     │
│                                                           │
│  1. Detect VCS systems in directory                      │
│  2. Create FdPassthroughFS with VCS backends             │
│  3. Wrap in ObservingFS with VCS filtering               │
└─────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────┐
│                     ObservingFS<F>                       │
│                                                           │
│  • Queries VCS backends before notifying observers       │
│  • Skips notifications for VCS directories               │
└─────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────┐
│                  VcsBackend Trait                        │
│                                                           │
│  • GitBackend      → filters .git/*                      │
│  • JujutsuBackend  → filters .jj/*                       │
│  • PijulBackend    → filters .pijul/*                    │
└─────────────────────────────────────────────────────────┘
```

## VcsBackend Trait

Each VCS implements the `VcsBackend` trait:

```rust
pub trait VcsBackend: Send + Sync {
    fn name(&self) -> &str;
    fn vcs_dir_name(&self) -> &str;
    fn is_present(&self, base_path: &Path) -> bool;
    fn should_ignore(&self, rel_path: &Path) -> bool;
}
```

### Implementations

- **GitBackend**: Filters `.git` directory
- **JujutsuBackend**: Filters `.jj` directory  
- **PijulBackend**: Filters `.pijul` directory

## Usage

### Basic Detection

```rust
use ize_lib::vcs::detect_all_vcs;
use std::path::Path;

let repo_path = Path::new("/path/to/repo");
let vcs_backends = detect_all_vcs(repo_path);

for backend in &vcs_backends {
    println!("Detected: {}", backend.name());
}
```

### With FdPassthroughFS

`FdPassthroughFS` automatically detects VCS directories:

```rust
use ize_lib::backing_fs::LibcBackingFs;
use ize_lib::filesystems::FdPassthroughFS;
use std::path::PathBuf;

let backing = LibcBackingFs::open_dir(&repo_path)?;
let fs = FdPassthroughFS::new(backing, repo_path.clone());

// Query detected VCS systems
let vcs_names = fs.detected_vcs();
println!("Detected VCS: {:?}", vcs_names);

// Check if a path is inside VCS directory
assert!(fs.is_vcs_path(Path::new(".git/objects/abc123")));
assert!(!fs.is_vcs_path(Path::new("src/main.rs")));
```

### With ObservingFS and Filtering

Wire VCS filtering into `ObservingFS` to prevent recording VCS operations:

```rust
use ize_lib::backing_fs::LibcBackingFs;
use ize_lib::filesystems::{FdPassthroughFS, ObservingFS};
use ize_lib::vcs::detect_all_vcs;
use std::path::PathBuf;

let repo_path = PathBuf::from("/path/to/repo");

// 1. Detect VCS systems
let vcs_backends = detect_all_vcs(&repo_path);

// 2. Create backing filesystem
let backing = LibcBackingFs::open_dir(&repo_path)?;
let passthrough = FdPassthroughFS::new(backing, repo_path.clone());

// 3. Wrap in ObservingFS
let mut observing = ObservingFS::new(passthrough);

// 4. Configure VCS filtering
observing.set_vcs_backends(vcs_backends);

// 5. Add your observers
// observing.add_observer(my_observer);

// Now mount the filesystem - VCS directories won't trigger observer notifications
```

## Complete Example

```rust
use std::path::PathBuf;
use std::sync::Arc;
use ize_lib::backing_fs::LibcBackingFs;
use ize_lib::filesystems::{FdPassthroughFS, ObservingFS};
use ize_lib::vcs::detect_all_vcs;

fn setup_observed_filesystem(repo_path: PathBuf) -> anyhow::Result<ObservingFS<FdPassthroughFS<LibcBackingFs>>> {
    // Detect VCS systems present in the directory
    let vcs_backends = detect_all_vcs(&repo_path);
    
    if !vcs_backends.is_empty() {
        let vcs_names: Vec<&str> = vcs_backends.iter().map(|b| b.name()).collect();
        println!("Detected VCS systems: {:?}", vcs_names);
    }

    // Create the backing filesystem with pre-opened fd
    let backing = LibcBackingFs::open_dir(&repo_path)?;
    
    // Create passthrough filesystem
    let passthrough = FdPassthroughFS::new(backing, repo_path.clone());
    
    // Wrap in ObservingFS with VCS filtering
    let mut observing = ObservingFS::new(passthrough);
    observing.set_vcs_backends(vcs_backends);
    
    // Add observers here
    // let my_observer = Arc::new(MyObserver::new());
    // observing.add_observer(my_observer);
    
    Ok(observing)
}
```

## How Filtering Works

When an operation occurs inside a VCS directory:

1. **FUSE receives operation** (e.g., write to `.git/index`)
2. **ObservingFS checks path** against VCS backends
3. **VCS backend returns `should_ignore() = true`**
4. **Notification is skipped** - observers never called
5. **Operation proceeds** to inner filesystem normally

Operations on regular files proceed with full observation.

## Multiple VCS Support

Multiple VCS systems can coexist in the same directory:

```rust
// A repository with both Git and Jujutsu
let vcs_backends = detect_all_vcs(Path::new("/repo"));
// Returns: [GitBackend, JujutsuBackend]

// Both .git/* and .jj/* paths will be filtered
observing.set_vcs_backends(vcs_backends);
```

## Custom VCS Backends

To add support for a new VCS, implement the `VcsBackend` trait:

```rust
use ize_lib::vcs::VcsBackend;
use std::path::Path;

pub struct FossilBackend;

impl VcsBackend for FossilBackend {
    fn name(&self) -> &str {
        "Fossil"
    }

    fn vcs_dir_name(&self) -> &str {
        ".fslckout"
    }

    fn is_present(&self, base_path: &Path) -> bool {
        base_path.join(".fslckout").exists()
    }

    fn should_ignore(&self, rel_path: &Path) -> bool {
        // Filter .fslckout and _FOSSIL_ files
        rel_path.starts_with(".fslckout") || 
        rel_path.file_name()
            .and_then(|n| n.to_str())
            .map(|s| s == "_FOSSIL_")
            .unwrap_or(false)
    }
}
```

## Design Rationale

### Why Trait-Based?

- **Extensibility**: Easy to add new VCS systems
- **Composability**: Multiple VCS can coexist
- **Testability**: Each backend independently testable
- **Separation of Concerns**: VCS logic separate from filesystem logic

### Why Filter at ObservingFS Level?

- **Performance**: Avoid unnecessary observer calls
- **Cleanliness**: VCS operations never recorded
- **Flexibility**: Observers don't need VCS-aware logic

### Why Not Filter in Observers?

Filtering in `ObservingFS` is superior because:

1. Single filtering point for all observers
2. Observers remain simple and focused
3. VCS logic centralized and reusable
4. No observer notification overhead for VCS paths

## Related

- [`FdPassthroughFS`](../filesystems/passthrough_fd.rs) - FUSE passthrough implementation
- [`ObservingFS`](../filesystems/observing.rs) - Observer pattern wrapper
- [`BackingFs`](../backing_fs/mod.rs) - Filesystem abstraction trait