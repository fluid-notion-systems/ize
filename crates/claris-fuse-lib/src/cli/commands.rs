use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Claris FUSE - Version-Controlled Filesystem
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Optional log level (trace, debug, info, warn, error)
    #[arg(long, value_name = "LEVEL")]
    pub log_level: Option<String>,

    /// Unmount filesystems on exit
    #[arg(long)]
    pub unmount_on_exit: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initialize a directory for version control (creates claris-fuse.db)
    Init {
        /// Directory to initialize for version control
        #[arg(value_name = "DIRECTORY")]
        directory: PathBuf,
    },

    /// Mount a filesystem with version control
    Mount {
        /// Source directory containing claris-fuse.db
        #[arg(value_name = "SOURCE_DIR")]
        source_dir: PathBuf,

        /// Mount point directory
        #[arg(value_name = "MOUNTPOINT")]
        mountpoint: PathBuf,

        /// Mount filesystem in read-only mode
        #[arg(long)]
        read_only: bool,
    },

    /// View version history of a file
    History {
        /// Path to the file to show history for
        #[arg(value_name = "FILE_PATH")]
        file_path: PathBuf,

        /// Number of versions to show (default: all)
        #[arg(long)]
        limit: Option<usize>,

        /// Show detailed information
        #[arg(long)]
        verbose: bool,
    },

    /// Restore a file to a previous version
    Restore {
        /// Path to the file to restore
        #[arg(value_name = "FILE_PATH")]
        file_path: PathBuf,

        /// Version to restore to
        #[arg(long)]
        version: usize,

        /// Don't prompt for confirmation
        #[arg(long)]
        force: bool,
    },
}
