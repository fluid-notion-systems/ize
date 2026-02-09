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

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use env_logger::Env;
use fuser::MountOption;
use ize_lib::backing_fs::LibcBackingFs;
use ize_lib::filesystems::{FdPassthroughFS, ObservingFS};
use ize_lib::operations::{OpcodeQueue, OpcodeRecorder, Operation};
use ize_lib::vcs::{GitBackend, JujutsuBackend, PijulBackend as PijulVcsBackend};
use log::{error, info, warn};

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

    /// Include VCS directory operations (.git, .jj, .pijul) when dumping
    ///
    /// WARNING: This can cause recursive write amplification if VCS tools or
    /// hooks respond to recorded operations. Only use for debugging VCS internals.
    ///
    /// By default, VCS operations are filtered to prevent feedback loops and
    /// match production behavior.
    #[arg(long, requires = "dump")]
    include_vcs_ops: bool,
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
    if cli.dump {
        println!("  Logging opcodes to tmp/dump.log");
        if cli.include_vcs_ops {
            println!("  Including VCS operations (WARNING: potential for recursive writes)");
        } else {
            println!("  VCS filtering enabled (production mode)");
        }
    }
    println!("  Press Ctrl+C to unmount");
    println!();

    info!(
        "Mounting FUSE on {:?} (*at() calls bypass FUSE via pre-opened fd)",
        mount_point
    );

    if cli.dump {
        // ------------------------------------------------------------------
        // Opcode recording mode: wrap with ObservingFS and log to tmp/dump.log
        // ------------------------------------------------------------------

        let queue = OpcodeQueue::new();
        let inode_map = fs.inode_map();
        let recorder = OpcodeRecorder::new(inode_map, target_dir.clone(), queue.sender());

        let mut observing_fs = ObservingFS::new(fs);
        observing_fs.add_observer(Arc::new(recorder));

        // Add VCS filtering backends unless --include-vcs-ops is specified
        if !cli.include_vcs_ops {
            let vcs_backends: Vec<Box<dyn ize_lib::vcs::VcsBackend>> = vec![
                Box::new(GitBackend),
                Box::new(JujutsuBackend),
                Box::new(PijulVcsBackend),
            ];
            observing_fs.set_vcs_backends(vcs_backends);
        }

        // Open log file
        std::fs::create_dir_all("tmp").context("Failed to create tmp directory")?;
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open("tmp/dump.log")
            .context("Failed to open tmp/dump.log")?;
        let log_file = Arc::new(Mutex::new(log_file));

        // Spawn opcode consumer thread
        let consumer_running = running.clone();
        let consumer_queue = queue.clone();
        let consumer_log = log_file.clone();
        let _consumer_handle = thread::spawn(move || {
            info!("Opcode consumer thread started");

            while consumer_running.load(Ordering::SeqCst) {
                if let Some(opcode) = consumer_queue.try_pop() {
                    if let Err(e) = log_opcode(&opcode, &consumer_log) {
                        warn!("Failed to log opcode: {}", e);
                    }
                } else {
                    thread::sleep(Duration::from_millis(10));
                }
            }

            info!("Opcode consumer thread stopping");
        });

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

        // Signal consumer to stop
        running.store(false, Ordering::SeqCst);
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

/// Log an opcode to file
fn log_opcode(
    opcode: &ize_lib::operations::Opcode,
    log_file: &Arc<Mutex<std::fs::File>>,
) -> Result<()> {
    let mut file = log_file.lock().unwrap();

    writeln!(
        file,
        "═══════════════════════════════════════════════════════"
    )?;
    writeln!(
        file,
        "Opcode #{} (timestamp: {})",
        opcode.seq(),
        opcode.timestamp()
    )?;
    writeln!(
        file,
        "───────────────────────────────────────────────────────"
    )?;

    match opcode.op() {
        Operation::FileCreate {
            path,
            mode,
            content,
        } => {
            writeln!(file, "  Type: FileCreate")?;
            writeln!(file, "  Path: {:?}", path)?;
            writeln!(file, "  Mode: {:o}", mode)?;
            writeln!(file, "  Content: {} bytes", content.len())?;
            write_bytes(&mut file, content, 100, false)?;
        }
        Operation::FileWrite { path, offset, data } => {
            writeln!(file, "  Type: FileWrite")?;
            writeln!(file, "  Path: {:?}", path)?;
            writeln!(file, "  Offset: {}", offset)?;
            writeln!(file, "  Data: {} bytes", data.len())?;
            write_bytes(&mut file, data, 100, false)?;
        }
        Operation::FileTruncate { path, new_size } => {
            writeln!(file, "  Type: FileTruncate")?;
            writeln!(file, "  Path: {:?}", path)?;
            writeln!(file, "  New Size: {}", new_size)?;
        }
        Operation::FileDelete { path } => {
            writeln!(file, "  Type: FileDelete")?;
            writeln!(file, "  Path: {:?}", path)?;
        }
        Operation::FileRename { old_path, new_path } => {
            writeln!(file, "  Type: FileRename")?;
            writeln!(file, "  Old Path: {:?}", old_path)?;
            writeln!(file, "  New Path: {:?}", new_path)?;
        }
        Operation::DirCreate { path, mode } => {
            writeln!(file, "  Type: DirCreate")?;
            writeln!(file, "  Path: {:?}", path)?;
            writeln!(file, "  Mode: {:o}", mode)?;
        }
        Operation::DirDelete { path } => {
            writeln!(file, "  Type: DirDelete")?;
            writeln!(file, "  Path: {:?}", path)?;
        }
        Operation::DirRename { old_path, new_path } => {
            writeln!(file, "  Type: DirRename")?;
            writeln!(file, "  Old Path: {:?}", old_path)?;
            writeln!(file, "  New Path: {:?}", new_path)?;
        }
        Operation::SetPermissions { path, mode } => {
            writeln!(file, "  Type: SetPermissions")?;
            writeln!(file, "  Path: {:?}", path)?;
            writeln!(file, "  Mode: {:o}", mode)?;
        }
        Operation::SetTimestamps { path, atime, mtime } => {
            writeln!(file, "  Type: SetTimestamps")?;
            writeln!(file, "  Path: {:?}", path)?;
            writeln!(file, "  Atime: {:?}", atime)?;
            writeln!(file, "  Mtime: {:?}", mtime)?;
        }
        Operation::SetOwnership { path, uid, gid } => {
            writeln!(file, "  Type: SetOwnership")?;
            writeln!(file, "  Path: {:?}", path)?;
            writeln!(file, "  UID: {:?}", uid)?;
            writeln!(file, "  GID: {:?}", gid)?;
        }
        Operation::SymlinkCreate { path, target } => {
            writeln!(file, "  Type: SymlinkCreate")?;
            writeln!(file, "  Path: {:?}", path)?;
            writeln!(file, "  Target: {:?}", target)?;
        }
        Operation::SymlinkDelete { path } => {
            writeln!(file, "  Type: SymlinkDelete")?;
            writeln!(file, "  Path: {:?}", path)?;
        }
        Operation::HardLinkCreate {
            existing_path,
            new_path,
        } => {
            writeln!(file, "  Type: HardLinkCreate")?;
            writeln!(file, "  Existing Path: {:?}", existing_path)?;
            writeln!(file, "  New Path: {:?}", new_path)?;
        }
    }
    writeln!(file)?;

    Ok(())
}

/// Write bytes as utf8 or raw hex to file
fn write_bytes(
    file: &mut std::fs::File,
    data: &[u8],
    max_bytes: usize,
    show_raw: bool,
) -> Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let truncated = data.len() > max_bytes;
    let display_data = if truncated { &data[..max_bytes] } else { data };

    if show_raw {
        writeln!(
            file,
            "  Bytes: {:?}{}",
            display_data,
            if truncated { "..." } else { "" }
        )?;
    } else if let Ok(s) = std::str::from_utf8(display_data) {
        writeln!(
            file,
            "  Content (utf8): {:?}{}",
            s,
            if truncated { "..." } else { "" }
        )?;
    } else {
        writeln!(
            file,
            "  Bytes (non-utf8): {:?}{}",
            display_data,
            if truncated { "..." } else { "" }
        )?;
    }

    Ok(())
}
