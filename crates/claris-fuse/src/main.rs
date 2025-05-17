use anyhow::{Context, Result};
use clap::Parser;
use claris_fuse_lib::cli::commands::{Cli, Commands};
use claris_fuse_lib::filesystems::passthrough::PassthroughFS;
use claris_fuse_lib::storage::StorageManager;
use env_logger::Env;
use log::{error, info};
use std::process::Command;

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
        Commands::Init { directory } => {
            info!("Initializing directory {:?} for version control", directory);

            // Check if directory exists
            if !directory.exists() {
                error!("Directory does not exist: {:?}", directory);
                return Err(anyhow::anyhow!("Directory does not exist"));
            }

            // Check if directory is actually a directory
            if !directory.is_dir() {
                error!("Path is not a directory: {:?}", directory);
                return Err(anyhow::anyhow!("Path is not a directory"));
            }

            // Initialize the database
            StorageManager::init(&directory)
                .with_context(|| format!("Failed to initialize storage in {:?}", directory))?;

            println!("Successfully initialized directory for version control");
            println!("You can now mount the filesystem with:");
            println!("claris-fuse mount {:?} <mountpoint>", directory);
        }
        Commands::Mount {
            source_dir,
            mountpoint,
            read_only,
        } => {
            info!(
                "Mounting filesystem from {:?} to mount point {:?}{}",
                source_dir,
                mountpoint,
                if read_only { " (read-only)" } else { "" }
            );

            // Check if the directory was initialized
            if !StorageManager::is_valid(&source_dir).with_context(|| {
                format!(
                    "Failed to check if {:?} is a valid Claris-FUSE directory",
                    source_dir
                )
            })? {
                error!(
                    "Directory {:?} has not been initialized for version control",
                    source_dir
                );
                error!("Run 'claris-fuse init {:?}' first", source_dir);
                return Err(anyhow::anyhow!("Directory not initialized"));
            }

            // Construct DB path from source directory
            let db_path = source_dir.join("claris-fuse.db");

            // Save mountpoint for cleanup on exit
            let mp_copy = mountpoint.clone();

            // Create and mount the passthrough filesystem
            let fs = if read_only {
                PassthroughFS::new_read_only(db_path, mp_copy.clone())?
            } else {
                PassthroughFS::new(db_path, mp_copy.clone())?
            };

            if unmount_on_exit {
                info!("Will unmount filesystem on exit");

                // Set up signal handler for SIGINT and SIGTERM
                ctrlc::set_handler(move || {
                    info!("Received interrupt signal, unmounting filesystem");
                    // Use fusermount to unmount the filesystem
                    match Command::new("fusermount").arg("-u").arg(&mp_copy).status() {
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

            fs.mount()?;
        }
        Commands::History {
            file_path,
            limit,
            verbose,
        } => {
            info!("Viewing history for file {:?}", file_path);

            // TODO: Implement history viewing logic
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

            // TODO: Implement restoration logic
            println!("File restoration not yet implemented");

            if force {
                println!("Force mode enabled, skipping confirmation");
            }
        }
    }

    // Check for any pending background operations (will be implemented later)
    // For now, this ensures we don't exit immediately after certain operations

    Ok(())
}
