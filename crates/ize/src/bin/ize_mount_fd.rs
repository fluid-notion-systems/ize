//! ize-mount-fd — fd-based FUSE passthrough mount
//!
//! Mounts a FUSE passthrough filesystem backed by a pre-opened directory fd.
//! All underlying I/O goes through `LibcBackingFs` + `FdPassthroughFS` from
//! the library — no inline syscall wrappers.
//!
//! The directory fd is opened **before** the FUSE mount is established, so
//! `*at()` syscalls resolve against the underlying filesystem's inode and
//! never re-enter FUSE.
//!
//! Usage:
//!   ize_mount_fd <DIRECTORY>
//!   ize_mount_fd <DIRECTORY> --read-only
//!   ize_mount_fd <DIRECTORY> --log-level debug
//!
//! Press Ctrl+C to unmount and exit.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use env_logger::Env;
use fuser::MountOption;
use ize_lib::backing_fs::LibcBackingFs;
use ize_lib::filesystems::{FdPassthroughFS, ObservingFS};
use ize_lib::operations::DumpObserver;
use ize_lib::vcs::{GitBackend, IgnoreFilter, JujutsuBackend, PijulBackend};
use log::{error, info};

/// Mount a directory with an fd-based FUSE passthrough filesystem.
#[derive(Parser, Debug)]
#[command(name = "ize-mount-fd", version, about)]
struct Cli {
    /// Directory to mount over (will be both mount point and backing store)
    #[arg(value_name = "DIRECTORY")]
    directory: PathBuf,

    /// Mount in read-only mode
    #[arg(long)]
    read_only: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, value_name = "LEVEL", default_value = "info")]
    log_level: String,

    /// Dump filesystem operations to stdout (opcode recording)
    #[arg(long)]
    dump: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    env_logger::Builder::from_env(Env::default().filter_or("RUST_LOG", &cli.log_level))
        .format_timestamp_millis()
        .init();

    // Canonicalize the target directory.
    let target_dir = std::fs::canonicalize(&cli.directory)
        .with_context(|| format!("Failed to canonicalize path: {:?}", cli.directory))?;

    if !target_dir.is_dir() {
        anyhow::bail!("Not a directory: {:?}", target_dir);
    }

    info!("Target directory: {:?}", target_dir);

    // ------------------------------------------------------------------
    // 1. Open the directory fd BEFORE mounting — this is the critical step.
    //    LibcBackingFs::open_dir owns the fd and closes it on drop.
    // ------------------------------------------------------------------

    let backing = LibcBackingFs::open_dir(&target_dir)
        .with_context(|| format!("Failed to open backing directory: {:?}", target_dir))?;

    info!(
        "Opened base directory fd={} for {:?} (BEFORE mount)",
        backing.base_fd(),
        target_dir
    );

    // ------------------------------------------------------------------
    // 2. Create FdPassthroughFS from the backing store
    // ------------------------------------------------------------------

    let mut fs = FdPassthroughFS::new(backing, target_dir.clone());

    if cli.read_only {
        fs.set_read_only(true);
    }

    let vcs = fs.detected_vcs();
    if !vcs.is_empty() {
        info!("Detected VCS directories: {:?}", vcs);
    }

    // ------------------------------------------------------------------
    // 3. Set up signal handler for clean unmount
    // ------------------------------------------------------------------

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    let target_for_handler = target_dir.clone();

    ctrlc::set_handler(move || {
        info!("Received interrupt, shutting down...");
        r.store(false, Ordering::SeqCst);

        // fusermount -u to cleanly unmount
        let status = std::process::Command::new("fusermount")
            .arg("-u")
            .arg(&target_for_handler)
            .status();
        match status {
            Ok(s) if s.success() => info!("Unmounted successfully"),
            Ok(s) => error!("fusermount exited with: {}", s),
            Err(e) => error!("Failed to run fusermount: {}", e),
        }
        std::process::exit(0);
    })
    .context("Failed to set signal handler")?;

    // ------------------------------------------------------------------
    // 4. Mount
    // ------------------------------------------------------------------

    let mount_point = cli.directory.clone();
    let mount_display = mount_point.display().to_string();

    let mut options = vec![
        MountOption::FSName("ize-mount-fd".to_string()),
        MountOption::DefaultPermissions,
    ];
    if cli.read_only {
        options.push(MountOption::RO);
    }

    println!("✓ Mounting '{}' with ize-mount-fd", mount_display);
    if cli.read_only {
        println!("  Mode: read-only");
    }
    // Determine dump log path OUTSIDE the mount to avoid recursive FUSE writes
    let dump_log_path = if cli.dump {
        let path = std::env::temp_dir().join("ize-dump.log");
        println!("  Logging opcodes to {}", path.display());
        Some(path)
    } else {
        None
    };
    println!("  Press Ctrl+C to unmount");
    println!();

    info!(
        "Mounting FUSE on {:?} (*at() calls bypass FUSE via pre-opened fd)",
        mount_point
    );

    if cli.dump {
        // ------------------------------------------------------------------
        // Dump mode: wrap with ObservingFS and attach a DumpObserver that
        // writes directly to the log file — no queue or consumer thread.
        // ------------------------------------------------------------------

        let inode_map = fs.inode_map();

        // Build ignore filters from detected VCS directories
        let ignore_filters: Vec<Box<dyn IgnoreFilter>> = {
            let mut filters: Vec<Box<dyn IgnoreFilter>> = Vec::new();
            for name in fs.detected_vcs() {
                match name.as_str() {
                    ".git" => filters.push(Box::new(GitBackend)),
                    ".jj" => filters.push(Box::new(JujutsuBackend)),
                    ".pijul" => filters.push(Box::new(PijulBackend)),
                    _ => {}
                }
            }
            filters
        };

        let filter_names: Vec<&str> = ignore_filters.iter().map(|f| f.name()).collect();
        info!("DumpObserver ignore filters: {:?}", filter_names);

        let dump_path = dump_log_path.as_ref().unwrap();
        let dump_observer = DumpObserver::open(inode_map, target_dir.clone(), dump_path)
            .with_context(|| format!("Failed to open dump log: {}", dump_path.display()))?;
        let dump_observer = dump_observer.with_ignore_filters(ignore_filters);

        let mut observing_fs = ObservingFS::new(fs);
        observing_fs.add_observer(Arc::new(dump_observer));

        // Mount the observing filesystem
        if let Err(e) = fuser::mount2(observing_fs, &mount_point, &options) {
            anyhow::bail!(
                "FUSE mount failed: {}\n\n\
                 Hints:\n  \
                 • You may need to run as root\n  \
                 • Add 'user_allow_other' to /etc/fuse.conf\n  \
                 • Ensure 'fuse' kernel module is loaded (modprobe fuse)",
                e
            );
        }
    } else {
        // mount2 blocks until the filesystem is unmounted.
        // When it returns, `fs` is dropped, which drops `LibcBackingFs`,
        // which closes the base_fd automatically.
        if let Err(e) = fuser::mount2(fs, &mount_point, &options) {
            anyhow::bail!(
                "FUSE mount failed: {}\n\n\
                 Hints:\n  \
                 • You may need to run as root\n  \
                 • Add 'user_allow_other' to /etc/fuse.conf\n  \
                 • Ensure 'fuse' kernel module is loaded (modprobe fuse)",
                e
            );
        }
    }

    // ------------------------------------------------------------------
    // 5. Cleanup (reached after unmount)
    //    base_fd was already closed by LibcBackingFs::drop
    // ------------------------------------------------------------------

    println!("✓ Unmounted '{}'", mount_display);

    Ok(())
}
