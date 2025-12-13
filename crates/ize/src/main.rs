use anyhow::{Context, Result};
use clap::Parser;
use env_logger::Env;
use ize_lib::cli::commands::{ChannelAction, Cli, Commands};
use ize_lib::filesystems::observing::ObservingFS;
use ize_lib::filesystems::passthrough::PassthroughFS;
use ize_lib::operations::{OpcodeQueue, OpcodeRecorder};
use ize_lib::{IzeProject, OpcodeRecordingBackend, PijulBackend, ProjectManager};
use log::{error, info, warn};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logger with specified or default log level
    let env = match cli.log_level {
        Some(ref log_level) => Env::default().filter_or("RUST_LOG", log_level),
        None => Env::default().filter_or("RUST_LOG", "info"),
    };
    env_logger::init_from_env(env);

    // Set up signal handler for SIGINT if unmount_on_exit is specified
    let unmount_on_exit = cli.unmount_on_exit;

    match cli.command {
        Commands::Init { directory, channel } => {
            cmd_init(&directory, channel.as_deref())?;
        }
        Commands::Mount {
            directory,
            read_only,
            foreground,
        } => {
            cmd_mount(&directory, read_only, foreground, unmount_on_exit)?;
        }
        Commands::Unmount { directory } => {
            cmd_unmount(&directory)?;
        }
        Commands::Status { directory, verbose } => {
            cmd_status(directory.as_deref(), verbose)?;
        }
        Commands::List { format } => {
            cmd_list(&format)?;
        }
        Commands::History {
            file_path,
            limit,
            verbose,
        } => {
            info!("Viewing history for file {:?}", file_path);
            println!("File history viewing not yet implemented");

            if verbose {
                println!("Verbose mode enabled");
            }

            if let Some(limit_val) = limit {
                println!("Showing up to {} versions", limit_val);
            }
        }
        Commands::Restore {
            file_path,
            version,
            force,
        } => {
            info!("Restoring file {:?} to version {}", file_path, version);
            println!("File restoration not yet implemented");

            if force {
                println!("Force mode enabled, skipping confirmation");
            }
        }
        Commands::Channel { action } => {
            cmd_channel(action)?;
        }
        Commands::Remove { directory, force } => {
            cmd_remove(&directory, force)?;
        }
        Commands::ExportPijul { source, target } => {
            cmd_export_pijul(&source, &target)?;
        }
    }

    Ok(())
}

/// Initialize a directory for version control
fn cmd_init(directory: &PathBuf, channel: Option<&str>) -> Result<()> {
    info!("Initializing directory {:?} for version control", directory);

    // Check if directory exists
    if !directory.exists() {
        error!("Directory does not exist: {:?}", directory);
        return Err(anyhow::anyhow!("Directory does not exist: {:?}", directory));
    }

    // Check if directory is actually a directory
    if !directory.is_dir() {
        error!("Path is not a directory: {:?}", directory);
        return Err(anyhow::anyhow!("Path is not a directory: {:?}", directory));
    }

    let manager = ProjectManager::new().with_context(|| "Failed to create project manager")?;

    // Check if already tracked
    let canonical = std::fs::canonicalize(directory)?;
    if manager.find_by_source_dir(&canonical)?.is_some() {
        return Err(anyhow::anyhow!(
            "Directory is already tracked: {:?}\nUse 'ize status {:?}' to view project info",
            canonical,
            directory
        ));
    }

    let project = manager
        .create_project(directory)
        .with_context(|| format!("Failed to initialize project for {:?}", directory))?;

    // If a custom channel was requested, switch to it
    if let Some(channel_name) = channel {
        if channel_name != "main" {
            // Create and switch to the custom channel
            project
                .pijul
                .create_channel(channel_name)
                .with_context(|| format!("Failed to create channel '{}'", channel_name))?;
        }
    }

    println!("✓ Initialized ize for '{}'", canonical.display());
    println!("  Project UUID: {}", project.uuid());
    println!("  Channel: {}", project.current_channel());
    println!();
    println!("Next steps:");
    println!("  Mount with: ize mount {}", directory.display());
    println!("  View status: ize status {}", directory.display());

    Ok(())
}

/// Mount a tracked directory
fn cmd_mount(
    directory: &PathBuf,
    read_only: bool,
    foreground: bool,
    unmount_on_exit: bool,
) -> Result<()> {
    info!(
        "Mounting filesystem for {:?}{}",
        directory,
        if read_only { " (read-only)" } else { "" }
    );

    let manager = ProjectManager::new().with_context(|| "Failed to create project manager")?;

    // Canonicalize the path to match what was stored
    let source_dir = std::fs::canonicalize(directory)
        .with_context(|| format!("Failed to canonicalize path: {:?}", directory))?;

    let project = manager.find_by_source_dir(&source_dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Directory not tracked: {:?}\nInitialize with: ize init {:?}",
            source_dir,
            directory
        )
    })?;

    // Check if already mounted
    if is_fuse_mounted(&source_dir)? {
        return Err(anyhow::anyhow!(
            "Directory is already mounted: {:?}\nUnmount with: ize unmount {:?}",
            source_dir,
            directory
        ));
    }

    let mountpoint = source_dir.clone();
    let mp_copy = mountpoint.clone();

    // Create the passthrough filesystem
    // Note: We're using the working directory as the source
    let passthrough = if read_only {
        PassthroughFS::new_read_only(project.working_dir().to_path_buf(), mp_copy.clone())?
    } else {
        PassthroughFS::new(project.working_dir().to_path_buf(), mp_copy.clone())?
    };

    if unmount_on_exit || foreground {
        info!("Will unmount filesystem on exit");

        // Set up signal handler for SIGINT and SIGTERM
        let mp_for_handler = mp_copy.clone();
        ctrlc::set_handler(move || {
            info!("Received interrupt signal, unmounting filesystem");
            match Command::new("fusermount")
                .arg("-u")
                .arg(&mp_for_handler)
                .status()
            {
                Ok(status) if status.success() => {
                    info!("Successfully unmounted filesystem")
                }
                Ok(status) => error!("Failed to unmount filesystem, exit code: {}", status),
                Err(e) => error!("Failed to execute unmount command: {}", e),
            }
            std::process::exit(0);
        })
        .expect("Error setting signal handler");
    }

    println!("✓ Mounting '{}' with ize", source_dir.display());
    println!("  Working copy: {}", project.working_dir().display());
    println!("  Channel: {}", project.current_channel());
    if !read_only {
        println!("  Recording changes to Pijul");
    }
    if foreground {
        println!("  Running in foreground (Ctrl+C to unmount)");
    }

    if read_only {
        // Read-only mode: mount passthrough directly, no opcode recording
        passthrough
            .mount()
            .with_context(|| "Failed to mount filesystem")?;
    } else {
        // Read-write mode: set up opcode queue and recording
        let queue = OpcodeQueue::new();

        // Get the inode map for path resolution
        let inode_map = passthrough.inode_map();

        // Create the opcode recorder
        let recorder = OpcodeRecorder::new(
            inode_map,
            project.working_dir().to_path_buf(),
            queue.sender(),
        );

        // Wrap passthrough with observing filesystem
        let mut observing_fs = ObservingFS::new(passthrough);
        observing_fs.add_observer(Arc::new(recorder));

        // Flag for shutdown
        let running = Arc::new(AtomicBool::new(true));

        // Spawn the opcode consumer thread
        let consumer_running = running.clone();
        let consumer_queue = queue.clone();
        let pijul_dir = project.pijul_dir().to_path_buf();
        let working_dir = project.working_dir().to_path_buf();

        let _consumer_handle = thread::spawn(move || {
            info!("Opcode consumer thread started");

            // Open the Pijul backend for recording
            let pijul = match PijulBackend::open(&pijul_dir, &working_dir) {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to open PijulBackend: {}", e);
                    return;
                }
            };
            let backend = OpcodeRecordingBackend::new(pijul);

            while consumer_running.load(Ordering::SeqCst) {
                // Try to pop with a short timeout by polling
                if let Some(opcode) = consumer_queue.try_pop() {
                    match backend.apply_opcode(&opcode) {
                        Ok(Some(hash)) => {
                            info!("Recorded opcode #{} -> {:?}", opcode.seq(), hash);
                        }
                        Ok(None) => {
                            // No change needed (e.g., writing same content)
                            info!("Opcode #{} resulted in no change", opcode.seq());
                        }
                        Err(e) => {
                            warn!("Failed to apply opcode #{}: {}", opcode.seq(), e);
                        }
                    }
                } else {
                    // Brief sleep to avoid busy-waiting
                    thread::sleep(std::time::Duration::from_millis(10));
                }
            }

            info!("Opcode consumer thread shutting down");
        });

        // Mount the observing filesystem (this blocks until unmounted)
        observing_fs
            .mount()
            .with_context(|| "Failed to mount filesystem")?;

        // Signal consumer to stop
        running.store(false, Ordering::SeqCst);
    }

    Ok(())
}

/// Unmount a tracked directory
fn cmd_unmount(directory: &PathBuf) -> Result<()> {
    let source_dir = std::fs::canonicalize(directory)
        .with_context(|| format!("Failed to canonicalize path: {:?}", directory))?;

    // Check if mounted
    if !is_fuse_mounted(&source_dir)? {
        return Err(anyhow::anyhow!(
            "Directory is not mounted: {:?}",
            source_dir
        ));
    }

    // Use fusermount to unmount
    let status = Command::new("fusermount")
        .arg("-u")
        .arg(&source_dir)
        .status()
        .with_context(|| "Failed to execute fusermount")?;

    if status.success() {
        println!("✓ Unmounted '{}'", source_dir.display());
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Failed to unmount '{}' (exit code: {})",
            source_dir.display(),
            status
        ))
    }
}

/// Show status of a tracked directory
fn cmd_status(directory: Option<&Path>, verbose: bool) -> Result<()> {
    let manager = ProjectManager::new().with_context(|| "Failed to create project manager")?;

    let source_dir = if let Some(dir) = directory {
        std::fs::canonicalize(dir)?
    } else {
        std::env::current_dir()?
    };

    let project = manager.find_by_source_dir(&source_dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Directory not tracked: {:?}\nInitialize with: ize init <directory>",
            source_dir
        )
    })?;

    let is_mounted = is_fuse_mounted(&source_dir)?;

    println!("Project: {}", source_dir.display());
    println!("UUID: {}", project.uuid());
    println!("Channel: {}", project.current_channel());
    println!(
        "Status: {}",
        if is_mounted { "mounted" } else { "not mounted" }
    );

    // List available channels
    let channels = project.list_channels()?;
    println!("Channels: {}", channels.join(", "));

    if verbose {
        println!();
        println!("Project directory: {}", project.project_dir.display());
        println!("Working copy: {}", project.working_dir().display());
        println!("Pijul directory: {}", project.pijul_dir().display());
    }

    Ok(())
}

/// List all tracked projects
fn cmd_list(format: &str) -> Result<()> {
    let manager = ProjectManager::new().with_context(|| "Failed to create project manager")?;

    let projects = manager.list_projects()?;

    if projects.is_empty() {
        println!("No projects found.");
        println!("Initialize one with: ize init <directory>");
        return Ok(());
    }

    match format {
        "json" => {
            // Simple JSON output
            println!("[");
            for (i, p) in projects.iter().enumerate() {
                let mounted = is_fuse_mounted(&p.source_dir).unwrap_or(false);
                println!("  {{");
                println!("    \"uuid\": \"{}\",", p.uuid);
                println!("    \"source_dir\": \"{}\",", p.source_dir.display());
                println!("    \"created\": \"{}\",", p.created);
                println!("    \"channel\": \"{}\",", p.default_channel);
                println!("    \"mounted\": {}", mounted);
                print!("  }}");
                if i < projects.len() - 1 {
                    println!(",");
                } else {
                    println!();
                }
            }
            println!("]");
        }
        _ => {
            // Table format (default)
            println!(
                "{:<50} {:<10} {:<36}",
                "SOURCE DIRECTORY", "MOUNTED", "UUID"
            );
            println!("{}", "-".repeat(96));
            for p in projects {
                let mounted = is_fuse_mounted(&p.source_dir).unwrap_or(false);
                let mounted_str = if mounted { "yes" } else { "no" };
                println!(
                    "{:<50} {:<10} {:<36}",
                    truncate_path(&p.source_dir, 48),
                    mounted_str,
                    p.uuid
                );
            }
        }
    }

    Ok(())
}

/// Handle channel subcommands
fn cmd_channel(action: ChannelAction) -> Result<()> {
    let manager = ProjectManager::new().with_context(|| "Failed to create project manager")?;

    match action {
        ChannelAction::Create { name, directory } => {
            let source_dir = get_source_dir(directory)?;
            let project = get_project(&manager, &source_dir)?;

            project
                .pijul
                .create_channel(&name)
                .with_context(|| format!("Failed to create channel '{}'", name))?;

            println!("✓ Created channel '{}'", name);
        }
        ChannelAction::List { directory } => {
            let source_dir = get_source_dir(directory)?;
            let project = get_project(&manager, &source_dir)?;

            let channels = project.list_channels()?;
            let current = project.current_channel();

            println!("Channels:");
            for channel in channels {
                if channel == current {
                    println!("  * {} (current)", channel);
                } else {
                    println!("    {}", channel);
                }
            }
        }
        ChannelAction::Switch { name, directory } => {
            let source_dir = get_source_dir(directory)?;
            let mut project = get_project(&manager, &source_dir)?;

            project
                .switch_channel(&name)
                .with_context(|| format!("Failed to switch to channel '{}'", name))?;

            println!("✓ Switched to channel '{}'", name);
        }
        ChannelAction::Fork {
            name,
            from,
            directory,
        } => {
            let source_dir = get_source_dir(directory)?;
            let project = get_project(&manager, &source_dir)?;

            let from_channel = from.unwrap_or_else(|| project.current_channel().to_string());

            project
                .pijul
                .fork_channel(&from_channel, &name)
                .with_context(|| {
                    format!("Failed to fork channel '{}' to '{}'", from_channel, name)
                })?;

            println!("✓ Forked '{}' to '{}'", from_channel, name);
        }
    }

    Ok(())
}

/// Remove a project from tracking
fn cmd_remove(directory: &PathBuf, force: bool) -> Result<()> {
    let manager = ProjectManager::new().with_context(|| "Failed to create project manager")?;

    let source_dir = std::fs::canonicalize(directory)
        .with_context(|| format!("Failed to canonicalize path: {:?}", directory))?;

    // Check if project exists
    let project = manager
        .find_by_source_dir(&source_dir)?
        .ok_or_else(|| anyhow::anyhow!("Directory not tracked: {:?}", source_dir))?;

    // Check if mounted
    if is_fuse_mounted(&source_dir)? {
        return Err(anyhow::anyhow!(
            "Cannot remove: directory is currently mounted.\nUnmount with: ize unmount {:?}",
            directory
        ));
    }

    if !force {
        println!(
            "This will remove ize tracking for '{}'",
            source_dir.display()
        );
        println!("Project UUID: {}", project.uuid());
        println!();
        println!("The original directory will NOT be modified.");
        println!(
            "The working copy at '{}' will be deleted.",
            project.working_dir().display()
        );
        println!();
        print!("Are you sure? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    manager.delete_project(&source_dir)?;
    println!("✓ Removed tracking for '{}'", source_dir.display());

    Ok(())
}

/// Check if a path is FUSE mounted
fn is_fuse_mounted(path: &PathBuf) -> Result<bool> {
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let path_str = path.to_string_lossy();
    Ok(mounts
        .lines()
        .any(|line| line.contains(&*path_str) && line.contains("fuse")))
}

/// Get the source directory, using current dir if not specified
fn get_source_dir(directory: Option<PathBuf>) -> Result<PathBuf> {
    let dir = directory.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    std::fs::canonicalize(&dir).with_context(|| format!("Failed to canonicalize path: {:?}", dir))
}

/// Get a project by source directory
fn get_project(manager: &ProjectManager, source_dir: &PathBuf) -> Result<IzeProject> {
    manager.find_by_source_dir(source_dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Directory not tracked: {:?}\nInitialize with: ize init <directory>",
            source_dir
        )
    })
}

/// Truncate a path for display
fn truncate_path(path: &PathBuf, max_len: usize) -> String {
    let s = path.display().to_string();
    if s.len() <= max_len {
        s
    } else {
        format!("...{}", &s[s.len() - max_len + 3..])
    }
}

/// Export pijul repository to a directory for inspection
fn cmd_export_pijul(source: &PathBuf, target: &PathBuf) -> Result<()> {
    let manager = ProjectManager::new().with_context(|| "Failed to create project manager")?;

    let source_dir = std::fs::canonicalize(source)
        .with_context(|| format!("Failed to canonicalize path: {:?}", source))?;

    let project = manager.find_by_source_dir(&source_dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Directory not tracked: {:?}\nInitialize with: ize init {:?}",
            source_dir,
            source
        )
    })?;

    // Create target directory
    if target.exists() {
        return Err(anyhow::anyhow!(
            "Target directory already exists: {:?}",
            target
        ));
    }
    fs::create_dir_all(target)
        .with_context(|| format!("Failed to create target directory: {:?}", target))?;

    // Copy .pijul directory
    let pijul_source = project.pijul_dir();
    let pijul_target = target.join(".pijul");
    copy_dir_recursive(&pijul_source, &pijul_target)
        .with_context(|| format!("Failed to copy .pijul directory from {:?}", pijul_source))?;

    // Copy working directory contents to target root
    let working_source = project.working_dir();
    copy_dir_contents(&working_source, target)
        .with_context(|| format!("Failed to copy working directory from {:?}", working_source))?;

    println!("✓ Exported pijul repository to '{}'", target.display());
    println!();
    println!("You can now use pijul commands in this directory:");
    println!("  cd {}", target.display());
    println!("  pijul log");
    println!("  pijul diff");
    println!("  pijul list");

    Ok(())
}

/// Recursively copy a directory
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            fs::copy(&path, &dest_path)?;
        }
    }

    Ok(())
}

/// Copy directory contents (not the directory itself) to destination
fn copy_dir_contents(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            fs::copy(&path, &dest_path)?;
        }
    }

    Ok(())
}
