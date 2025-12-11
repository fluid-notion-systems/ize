//! Dump Opcode Queue - Debug tool for Ize
//!
//! Mounts a filesystem with opcode recording and prints all captured
//! opcodes to stdout. Useful for debugging and understanding what
//! filesystem operations are being captured.
//!
//! Usage:
//!   ize_dump_opcode_queue <source_dir> <mount_point>
//!
//! Example:
//!   mkdir -p /tmp/source /tmp/mount
//!   ize_dump_opcode_queue /tmp/source /tmp/mount
//!
//! Then in another terminal:
//!   echo "hello" > /tmp/mount/test.txt
//!   cat /tmp/mount/test.txt
//!   rm /tmp/mount/test.txt
//!
//! Press Ctrl+C to unmount and exit.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use env_logger::Env;
use fuser::MountOption;
use log::{error, info};

use ize_lib::filesystems::observing::ObservingFS;
use ize_lib::filesystems::passthrough::PassthroughFS;
use ize_lib::operations::{OpcodeQueue, OpcodeRecorder, Operation};

/// Dump Opcode Queue - Debug tool for Ize
///
/// Mounts a filesystem with opcode recording and prints all captured
/// opcodes to stdout.
#[derive(Parser, Debug)]
#[command(name = "ize_dump_opcode_queue")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Source directory to expose through the filesystem
    #[arg(value_name = "SOURCE_DIR")]
    source_dir: PathBuf,

    /// Mount point where the filesystem will be mounted
    #[arg(value_name = "MOUNT_POINT")]
    mount_point: PathBuf,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    /// Show raw bytes for data (not just utf8)
    #[arg(long)]
    raw: bool,

    /// Maximum bytes to show for content/data fields
    #[arg(long, default_value = "100")]
    max_bytes: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logger
    env_logger::init_from_env(Env::default().filter_or("RUST_LOG", &args.log_level));

    let source_dir = &args.source_dir;
    let mount_point = &args.mount_point;

    // Validate paths
    if !source_dir.exists() {
        error!("Source directory does not exist: {:?}", source_dir);
        std::process::exit(1);
    }
    if !source_dir.is_dir() {
        error!("Source path is not a directory: {:?}", source_dir);
        std::process::exit(1);
    }
    if !mount_point.exists() {
        error!("Mount point does not exist: {:?}", mount_point);
        std::process::exit(1);
    }
    if !mount_point.is_dir() {
        error!("Mount point is not a directory: {:?}", mount_point);
        std::process::exit(1);
    }

    info!("Source directory: {:?}", source_dir);
    info!("Mount point: {:?}", mount_point);

    // Create the opcode queue
    let queue = OpcodeQueue::new();

    // Create the passthrough filesystem
    let passthrough =
        PassthroughFS::new(source_dir, mount_point).context("Failed to create PassthroughFS")?;

    // Get the inode map for the recorder
    let inode_map = passthrough.inode_map();

    // Create the opcode recorder
    let recorder = OpcodeRecorder::new(inode_map, source_dir.clone(), queue.sender());

    // Wrap with observing filesystem
    let mut observing_fs = ObservingFS::new(passthrough);
    observing_fs.add_observer(Arc::new(recorder));

    // Flag for shutdown
    let running = Arc::new(AtomicBool::new(true));

    // Spawn the opcode consumer thread
    let consumer_running = running.clone();
    let consumer_queue = queue.clone();
    let show_raw = args.raw;
    let max_bytes = args.max_bytes;
    let consumer_handle = thread::spawn(move || {
        info!("Opcode consumer thread started");

        while consumer_running.load(Ordering::SeqCst) {
            // Try to pop with a short timeout by polling
            if let Some(opcode) = consumer_queue.try_pop() {
                println!("═══════════════════════════════════════════════════════");
                println!(
                    "Opcode #{} (timestamp: {})",
                    opcode.seq(),
                    opcode.timestamp()
                );
                println!("───────────────────────────────────────────────────────");

                match opcode.op() {
                    Operation::FileCreate {
                        path,
                        mode,
                        content,
                    } => {
                        println!("  Type: FileCreate");
                        println!("  Path: {:?}", path);
                        println!("  Mode: {:o}", mode);
                        println!("  Content: {} bytes", content.len());
                        print_bytes(content, max_bytes, show_raw);
                    }
                    Operation::FileWrite { path, offset, data } => {
                        println!("  Type: FileWrite");
                        println!("  Path: {:?}", path);
                        println!("  Offset: {}", offset);
                        println!("  Data: {} bytes", data.len());
                        print_bytes(data, max_bytes, show_raw);
                    }
                    Operation::FileTruncate { path, new_size } => {
                        println!("  Type: FileTruncate");
                        println!("  Path: {:?}", path);
                        println!("  New Size: {}", new_size);
                    }
                    Operation::FileDelete { path } => {
                        println!("  Type: FileDelete");
                        println!("  Path: {:?}", path);
                    }
                    Operation::FileRename { old_path, new_path } => {
                        println!("  Type: FileRename");
                        println!("  Old Path: {:?}", old_path);
                        println!("  New Path: {:?}", new_path);
                    }
                    Operation::DirCreate { path, mode } => {
                        println!("  Type: DirCreate");
                        println!("  Path: {:?}", path);
                        println!("  Mode: {:o}", mode);
                    }
                    Operation::DirDelete { path } => {
                        println!("  Type: DirDelete");
                        println!("  Path: {:?}", path);
                    }
                    Operation::DirRename { old_path, new_path } => {
                        println!("  Type: DirRename");
                        println!("  Old Path: {:?}", old_path);
                        println!("  New Path: {:?}", new_path);
                    }
                    Operation::SetPermissions { path, mode } => {
                        println!("  Type: SetPermissions");
                        println!("  Path: {:?}", path);
                        println!("  Mode: {:o}", mode);
                    }
                    Operation::SetTimestamps { path, atime, mtime } => {
                        println!("  Type: SetTimestamps");
                        println!("  Path: {:?}", path);
                        println!("  Atime: {:?}", atime);
                        println!("  Mtime: {:?}", mtime);
                    }
                    Operation::SetOwnership { path, uid, gid } => {
                        println!("  Type: SetOwnership");
                        println!("  Path: {:?}", path);
                        println!("  UID: {:?}", uid);
                        println!("  GID: {:?}", gid);
                    }
                    Operation::SymlinkCreate { path, target } => {
                        println!("  Type: SymlinkCreate");
                        println!("  Path: {:?}", path);
                        println!("  Target: {:?}", target);
                    }
                    Operation::SymlinkDelete { path } => {
                        println!("  Type: SymlinkDelete");
                        println!("  Path: {:?}", path);
                    }
                    Operation::HardLinkCreate {
                        existing_path,
                        new_path,
                    } => {
                        println!("  Type: HardLinkCreate");
                        println!("  Existing Path: {:?}", existing_path);
                        println!("  New Path: {:?}", new_path);
                    }
                }
                println!();
            } else {
                // No opcode available, sleep briefly
                thread::sleep(Duration::from_millis(10));
            }
        }

        info!("Opcode consumer thread stopping");
    });

    // Set up signal handler for Ctrl+C
    let signal_running = running.clone();
    let signal_mount_point = mount_point.clone();
    ctrlc::set_handler(move || {
        info!("Received interrupt signal, unmounting filesystem...");
        signal_running.store(false, Ordering::SeqCst);

        // Unmount the filesystem
        match Command::new("fusermount")
            .arg("-u")
            .arg(&signal_mount_point)
            .status()
        {
            Ok(status) if status.success() => {
                info!("Successfully unmounted filesystem");
            }
            Ok(status) => {
                error!("Failed to unmount filesystem, exit code: {}", status);
            }
            Err(e) => {
                error!("Failed to execute fusermount: {}", e);
            }
        }
    })
    .context("Failed to set signal handler")?;

    // Mount options
    let options = vec![
        MountOption::FSName("ize-dump".to_string()),
        MountOption::AutoUnmount,
        MountOption::AllowOther,
    ];

    info!("Mounting filesystem...");
    info!("Press Ctrl+C to unmount and exit");
    println!();
    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║           Ize Opcode Queue Dump                       ║");
    println!("║                                                       ║");
    println!("║  Perform filesystem operations on the mount point     ║");
    println!("║  to see opcodes printed below.                        ║");
    println!("║                                                       ║");
    println!("║  Press Ctrl+C to unmount and exit.                    ║");
    println!("╚═══════════════════════════════════════════════════════╝");
    println!();

    // Mount the filesystem (this blocks until unmounted)
    fuser::mount2(observing_fs, mount_point, &options).context("Failed to mount filesystem")?;

    // Signal consumer to stop and wait for it
    running.store(false, Ordering::SeqCst);
    let _ = consumer_handle.join();

    info!("Exiting");
    Ok(())
}

/// Print bytes as utf8 or raw hex
fn print_bytes(data: &[u8], max_bytes: usize, show_raw: bool) {
    if data.is_empty() {
        return;
    }

    let truncated = data.len() > max_bytes;
    let display_data = if truncated { &data[..max_bytes] } else { data };

    if show_raw {
        println!(
            "  Bytes: {:?}{}",
            display_data,
            if truncated { "..." } else { "" }
        );
    } else if let Ok(s) = std::str::from_utf8(display_data) {
        println!(
            "  Content (utf8): {:?}{}",
            s,
            if truncated { "..." } else { "" }
        );
    } else {
        println!(
            "  Bytes (non-utf8): {:?}{}",
            display_data,
            if truncated { "..." } else { "" }
        );
    }
}
