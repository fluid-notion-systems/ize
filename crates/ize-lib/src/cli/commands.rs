use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Ize - Version-Controlled Filesystem
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
    /// Initialize a directory for version control
    ///
    /// This creates a new Ize project in the central store (~/.local/share/ize/projects/)
    /// and copies the directory contents for tracking.
    Init {
        /// Directory to initialize for version control
        #[arg(value_name = "DIRECTORY")]
        directory: PathBuf,

        /// Custom channel name (default: "main")
        #[arg(long, short)]
        channel: Option<String>,
    },

    /// Mount a tracked directory with version control
    ///
    /// Mounts a FUSE filesystem at the source directory location,
    /// backed by the versioned working copy in the central store.
    Mount {
        /// Directory to mount (must be initialized with `ize init`)
        #[arg(value_name = "DIRECTORY")]
        directory: PathBuf,

        /// Mount filesystem in read-only mode
        #[arg(long)]
        read_only: bool,

        /// Run in foreground (don't daemonize)
        #[arg(long, short)]
        foreground: bool,
    },

    /// Unmount a tracked directory
    Unmount {
        /// Directory to unmount
        #[arg(value_name = "DIRECTORY")]
        directory: PathBuf,
    },

    /// Show status of a tracked directory
    Status {
        /// Directory to show status for (default: current directory)
        #[arg(value_name = "DIRECTORY")]
        directory: Option<PathBuf>,

        /// Show detailed information
        #[arg(long, short)]
        verbose: bool,
    },

    /// List all tracked projects
    List {
        /// Output format (table, json)
        #[arg(long, default_value = "table")]
        format: String,
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

    /// Create a new channel (branch)
    Channel {
        #[command(subcommand)]
        action: ChannelAction,
    },

    /// Remove a project from tracking (does not delete source files)
    Remove {
        /// Directory to stop tracking
        #[arg(value_name = "DIRECTORY")]
        directory: PathBuf,

        /// Don't prompt for confirmation
        #[arg(long)]
        force: bool,
    },

    /// Export the pijul repository to a directory for inspection
    ///
    /// This copies the .pijul directory and working copy to a target directory,
    /// allowing you to use pijul commands directly to inspect the repository.
    ExportPijul {
        /// Source directory (must be initialized with `ize init`)
        #[arg(value_name = "SOURCE")]
        source: PathBuf,

        /// Target directory to export to
        #[arg(value_name = "TARGET")]
        target: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum ChannelAction {
    /// Create a new channel
    Create {
        /// Name of the new channel
        #[arg(value_name = "NAME")]
        name: String,

        /// Directory of the project (default: current directory)
        #[arg(long, short)]
        directory: Option<PathBuf>,
    },

    /// List all channels
    List {
        /// Directory of the project (default: current directory)
        #[arg(long, short)]
        directory: Option<PathBuf>,
    },

    /// Switch to a different channel
    Switch {
        /// Name of the channel to switch to
        #[arg(value_name = "NAME")]
        name: String,

        /// Directory of the project (default: current directory)
        #[arg(long, short)]
        directory: Option<PathBuf>,
    },

    /// Fork a channel
    Fork {
        /// Name of the new channel
        #[arg(value_name = "NAME")]
        name: String,

        /// Channel to fork from (default: current channel)
        #[arg(long)]
        from: Option<String>,

        /// Directory of the project (default: current directory)
        #[arg(long, short)]
        directory: Option<PathBuf>,
    },
}
