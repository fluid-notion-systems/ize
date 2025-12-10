# Analysis: ebpf-file-monitor and Directory Monitoring Extension

## Overview

The `vendor/ebpf-file-monitor` project is a Rust-based file monitoring tool. Despite its name suggesting eBPF usage, it actually uses **inotify** for file system event monitoring. This document analyzes the current implementation and proposes extensions for directory monitoring.

## Current Implementation Analysis

### Technology Stack

| Component | Technology | Notes |
|-----------|------------|-------|
| Event Source | **inotify** (not eBPF!) | Linux file notification API |
| Async Runtime | Tokio | Non-blocking I/O |
| Process Info | procfs | `/proc` filesystem parsing |
| Serialization | serde/serde_json | JSON output support |
| CLI | clap | Argument parsing |

### Key Limitation: Single File Only

The current implementation monitors **only a single file**, not directories:

```rust
// From main.rs line 27
#[arg(short, long)]
file: Option<String>,  // Single file path

// From monitor() - line 667
let path_buf = PathBuf::from(&self.file_path);
let _watch_descriptor = inotify
    .watches()
    .add(&path_buf, watch_mask)?;  // Watches ONE path
```

### Events Currently Monitored

```rust
// From main.rs lines 669-680
let watch_mask = WatchMask::ACCESS
    | WatchMask::MODIFY
    | WatchMask::ATTRIB
    | WatchMask::CLOSE_WRITE
    | WatchMask::CLOSE_NOWRITE
    | WatchMask::OPEN
    | WatchMask::MOVED_FROM
    | WatchMask::MOVED_TO
    | WatchMask::CREATE
    | WatchMask::DELETE
    | WatchMask::DELETE_SELF
    | WatchMask::MOVE_SELF;
```

### Data Structures

The implementation captures rich metadata:

```rust
struct FileEvent {
    timestamp: String,
    timestamp_unix: i64,
    event_type: String,
    file_path: String,
    file_metadata: Option<FileMetadata>,
    process_details: Option<ProcessDetails>,
    operation_details: Option<OperationDetails>,
    content_preview: Option<ContentPreview>,
}

struct ProcessDetails {
    pid: u32,
    ppid: u32,
    name: String,
    exe_path: String,
    cmdline: String,
    cwd: String,
    uid: u32,
    gid: u32,
    username: String,
    // ... more fields
}
```

---

## Why the Name "eBPF" is Misleading

The project name suggests eBPF usage, but it **does not use eBPF at all**. It uses:

1. **inotify** - For file system events
2. **procfs** - For process information (via `/proc`)

The README mentions eBPF dependencies (`libbpf`, `bcc`) but they are **not actually used** in the code. This appears to be either:
- Aspirational (planned but not implemented)
- Legacy from a different version
- Misleading naming

---

## Extending to Directory Monitoring

### Challenge 1: inotify Doesn't Recurse

inotify watches are **not recursive** by default. To watch a directory tree, you must:

1. Add a watch to each directory
2. When a new directory is created, add a watch to it
3. Track all watch descriptors

### Challenge 2: Watch Limits

inotify has system-wide limits:

```bash
# Check current limits
cat /proc/sys/fs/inotify/max_user_watches
# Default is often 8192, can be increased

# Increase limit (temporary)
sudo sysctl fs.inotify.max_user_watches=524288

# Permanent (add to /etc/sysctl.conf)
fs.inotify.max_user_watches=524288
```

### Proposed Architecture for Directory Monitoring

```
┌─────────────────────────────────────────────────────────────────┐
│                    DirectoryMonitor                              │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                    Watch Manager                            │ │
│  │                                                             │ │
│  │  watches: HashMap<WatchDescriptor, PathBuf>                │ │
│  │  paths: HashMap<PathBuf, WatchDescriptor>                  │ │
│  │                                                             │ │
│  │  + add_recursive(path)                                     │ │
│  │  + handle_create(path)  // Add watch for new dirs          │ │
│  │  + handle_delete(path)  // Remove watch for deleted dirs   │ │
│  └────────────────────────────────────────────────────────────┘ │
│                              │                                   │
│                              ▼                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                    Event Processor                          │ │
│  │                                                             │ │
│  │  + resolve_path(wd, name) -> full_path                     │ │
│  │  + filter_events(patterns)                                 │ │
│  │  + debounce_events()                                       │ │
│  └────────────────────────────────────────────────────────────┘ │
│                              │                                   │
│                              ▼                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                    Output Handler                           │ │
│  │  (existing FileEvent logging)                              │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

---

## Implementation Plan

### Phase 1: Refactor for Multiple Paths

#### New Data Structures

```rust
use std::collections::HashMap;
use inotify::WatchDescriptor;

struct WatchManager {
    inotify: Inotify,
    /// Map from watch descriptor to path
    wd_to_path: HashMap<WatchDescriptor, PathBuf>,
    /// Map from path to watch descriptor (for removal)
    path_to_wd: HashMap<PathBuf, WatchDescriptor>,
}

impl WatchManager {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            inotify: Inotify::init()?,
            wd_to_path: HashMap::new(),
            path_to_wd: HashMap::new(),
        })
    }

    fn add_watch(&mut self, path: &Path, mask: WatchMask) -> std::io::Result<()> {
        if self.path_to_wd.contains_key(path) {
            return Ok(()); // Already watching
        }

        let wd = self.inotify.watches().add(path, mask)?;
        self.wd_to_path.insert(wd.clone(), path.to_path_buf());
        self.path_to_wd.insert(path.to_path_buf(), wd);
        Ok(())
    }

    fn remove_watch(&mut self, path: &Path) -> std::io::Result<()> {
        if let Some(wd) = self.path_to_wd.remove(path) {
            self.inotify.watches().remove(wd)?;
            self.wd_to_path.remove(&wd);
        }
        Ok(())
    }

    fn get_path(&self, wd: &WatchDescriptor) -> Option<&PathBuf> {
        self.wd_to_path.get(wd)
    }
}
```

#### Recursive Directory Walking

```rust
use walkdir::WalkDir;

impl WatchManager {
    fn add_recursive(&mut self, root: &Path, mask: WatchMask) -> std::io::Result<usize> {
        let mut count = 0;

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_dir() {
                self.add_watch(path, mask)?;
                count += 1;
            }
        }

        Ok(count)
    }
}
```

### Phase 2: Handle Dynamic Directory Creation

When a new directory is created within a watched tree, we need to add a watch for it:

```rust
async fn process_event(&mut self, event: &inotify::Event<&OsStr>) -> Option<FileEvent> {
    let wd = &event.wd;
    let name = event.name?;
    
    // Resolve full path
    let parent_path = self.watch_manager.get_path(wd)?;
    let full_path = parent_path.join(name);
    
    // If a directory was created, add a watch for it
    if event.mask.contains(EventMask::CREATE) && event.mask.contains(EventMask::ISDIR) {
        if let Err(e) = self.watch_manager.add_recursive(&full_path, self.watch_mask) {
            error!("Failed to add watch for new directory {:?}: {}", full_path, e);
        }
    }
    
    // If a directory was deleted, remove its watch
    if event.mask.contains(EventMask::DELETE) && event.mask.contains(EventMask::ISDIR) {
        if let Err(e) = self.watch_manager.remove_watch(&full_path) {
            error!("Failed to remove watch for deleted directory {:?}: {}", full_path, e);
        }
    }
    
    // Create and return event...
}
```

### Phase 3: CLI Updates

```rust
#[derive(Parser, Debug)]
struct Args {
    /// Path to file or directory to monitor
    #[arg(short, long)]
    path: Option<String>,
    
    /// Watch directory recursively
    #[arg(short = 'r', long)]
    recursive: bool,
    
    /// Glob patterns to include (e.g., "*.rs")
    #[arg(long)]
    include: Vec<String>,
    
    /// Glob patterns to exclude (e.g., "*.log")
    #[arg(long)]
    exclude: Vec<String>,
    
    // ... existing args
}
```

### Phase 4: Event Filtering

```rust
use glob::Pattern;

struct EventFilter {
    include_patterns: Vec<Pattern>,
    exclude_patterns: Vec<Pattern>,
}

impl EventFilter {
    fn should_process(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        
        // Check exclusions first
        for pattern in &self.exclude_patterns {
            if pattern.matches(&path_str) {
                return false;
            }
        }
        
        // If no include patterns, include everything
        if self.include_patterns.is_empty() {
            return true;
        }
        
        // Check inclusions
        for pattern in &self.include_patterns {
            if pattern.matches(&path_str) {
                return true;
            }
        }
        
        false
    }
}
```

---

## Alternative: Use fanotify Instead

For Ize's use case, **fanotify might be better than inotify**:

| Feature | inotify | fanotify |
|---------|---------|----------|
| Per-file watches | Yes (limited) | No (mount-wide) |
| Recursive | Manual | Built-in (FAN_MARK_FILESYSTEM) |
| File descriptor | No | Yes (can read content!) |
| Watch limits | ~8K default | No limit |
| Root required | No | Yes |

### fanotify for Directory Monitoring

```rust
use nix::sys::fanotify::{Fanotify, InitFlags, MarkFlags, MaskFlags, EventFlags};

fn setup_fanotify(path: &Path) -> Result<Fanotify, Box<dyn Error>> {
    let fan = Fanotify::init(
        InitFlags::FAN_CLASS_NOTIF | InitFlags::FAN_REPORT_FID,
        EventFlags::O_RDONLY,
    )?;
    
    // Watch entire filesystem subtree with one call!
    fan.mark(
        MarkFlags::FAN_MARK_ADD | MarkFlags::FAN_MARK_FILESYSTEM,
        MaskFlags::FAN_MODIFY | MaskFlags::FAN_CREATE | MaskFlags::FAN_DELETE,
        None,
        Some(path),
    )?;
    
    Ok(fan)
}
```

**Recommendation**: For Ize, consider switching to fanotify rather than extending inotify, since:
1. No watch limits
2. No manual recursion needed
3. Gets file descriptor (can capture content)
4. Lower overhead for large directory trees

---

## Comparison: Extended inotify vs fanotify

| Aspect | Extended inotify | fanotify |
|--------|------------------|----------|
| Implementation effort | Medium (recursion logic) | Low (built-in) |
| Watch management | Complex (track all dirs) | Simple (one mark) |
| New directory handling | Must add watches | Automatic |
| Content capture | Must open file separately | Gets fd in event |
| Root required | No | Yes |
| Large trees | May hit limits | No limits |
| Overhead | Higher (many watches) | Lower |

---

## Recommended Path Forward

### For ebpf-file-monitor (General Purpose)

Extend with recursive inotify support as described above. This keeps it usable without root.

### For Ize Integration

Consider **two approaches**:

1. **Quick Win**: Extend ebpf-file-monitor with directory support, use as detection layer, open files separately for content capture.

2. **Better Long-term**: Implement fanotify-based monitoring (as described in `filesystem_interception_alternatives.md`) which provides both detection AND file descriptors for content capture.

---

## Code Changes Summary

### Files to Modify

1. **src/main.rs**
   - Add `WatchManager` struct
   - Implement recursive walking
   - Handle CREATE/DELETE for directories
   - Update CLI args
   - Add filtering

### New Dependencies

```toml
[dependencies]
walkdir = "2.4"      # Recursive directory walking
glob = "0.3"         # Pattern matching
```

### Estimated Effort

| Task | Effort |
|------|--------|
| WatchManager implementation | 2-3 hours |
| Recursive walking | 1 hour |
| Dynamic watch management | 2 hours |
| CLI updates | 1 hour |
| Filtering | 1-2 hours |
| Testing | 2-3 hours |
| **Total** | **~10-12 hours** |

---

## Conclusion

The `ebpf-file-monitor` project is a solid foundation but needs significant extension for directory monitoring. The main challenges are:

1. **inotify doesn't recurse** - requires manual watch management
2. **Watch limits** - may be hit with large trees
3. **Dynamic directories** - must track creates/deletes

For Ize specifically, **fanotify is likely a better choice** because it handles directories natively and provides file descriptors for content capture. However, extending the existing inotify-based monitor is also viable for environments where root access is not available.