use anyhow::Result;
use clap::{Parser, Subcommand};
use claris_fuse_lib::filesystem::PassthroughFS;
use env_logger::Env;
use log::info;
use std::path::PathBuf;

/// Claris FUSE - Version-Controlled Filesystem
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Optional log level (trace, debug, info, warn, error)
    #[arg(long, value_name = "LEVEL")]
    log_level: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Mount a filesystem with version control
    Mount {
        /// Database file path
        #[arg(value_name = "DB_PATH")]
        db_path: PathBuf,

        /// Mount point directory
        #[arg(value_name = "MOUNTPOINT")]
        mountpoint: PathBuf,
    },
    // More commands will be added later for history, restore, etc.
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logger with specified or default log level
    let env = match cli.log_level {
        Some(ref log_level) => Env::default().filter_or("RUST_LOG", log_level),
        None => Env::default().filter_or("RUST_LOG", "info"),
    };
    env_logger::init_from_env(env);

    match cli.command {
        Commands::Mount {
            db_path,
            mountpoint,
        } => {
            info!(
                "Mounting filesystem with database {:?} to mount point {:?}",
                db_path, mountpoint
            );

            // Create and mount the passthrough filesystem
            let fs = PassthroughFS::new(db_path, mountpoint)?;
            fs.mount()?;
        }
    }

    Ok(())
}
