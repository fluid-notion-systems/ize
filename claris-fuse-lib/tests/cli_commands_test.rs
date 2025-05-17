use claris_fuse_lib::cli::commands::{Cli, Commands};
use std::path::PathBuf;
use clap::Parser;

#[test]
fn test_cli_init_command() {
    let args = vec!["claris-fuse", "init", "/path/to/directory"];
    let cli = Cli::parse_from(args);
    
    match cli.command {
        Commands::Init { directory } => {
            assert_eq!(directory, PathBuf::from("/path/to/directory"));
        }
        _ => panic!("Expected Init command"),
    }
}

#[test]
fn test_cli_mount_command() {
    let args = vec!["claris-fuse", "mount", "/source/dir", "/mount/point"];
    let cli = Cli::parse_from(args);
    
    match cli.command {
        Commands::Mount { source_dir, mountpoint, read_only } => {
            assert_eq!(source_dir, PathBuf::from("/source/dir"));
            assert_eq!(mountpoint, PathBuf::from("/mount/point"));
            assert!(!read_only);
        }
        _ => panic!("Expected Mount command"),
    }
}

#[test]
fn test_cli_mount_read_only() {
    let args = vec!["claris-fuse", "mount", "--read-only", "/source/dir", "/mount/point"];
    let cli = Cli::parse_from(args);
    
    match cli.command {
        Commands::Mount { source_dir, mountpoint, read_only } => {
            assert_eq!(source_dir, PathBuf::from("/source/dir"));
            assert_eq!(mountpoint, PathBuf::from("/mount/point"));
            assert!(read_only);
        }
        _ => panic!("Expected Mount command"),
    }
}

#[test]
fn test_cli_history_command() {
    let args = vec!["claris-fuse", "history", "/path/to/file.txt"];
    let cli = Cli::parse_from(args);
    
    match cli.command {
        Commands::History { file_path, limit, verbose } => {
            assert_eq!(file_path, PathBuf::from("/path/to/file.txt"));
            assert!(limit.is_none());
            assert!(!verbose);
        }
        _ => panic!("Expected History command"),
    }
}

#[test]
fn test_cli_history_with_options() {
    let args = vec!["claris-fuse", "history", "--limit", "10", "--verbose", "/path/to/file.txt"];
    let cli = Cli::parse_from(args);
    
    match cli.command {
        Commands::History { file_path, limit, verbose } => {
            assert_eq!(file_path, PathBuf::from("/path/to/file.txt"));
            assert_eq!(limit, Some(10));
            assert!(verbose);
        }
        _ => panic!("Expected History command"),
    }
}

#[test]
fn test_cli_restore_command() {
    let args = vec!["claris-fuse", "restore", "/path/to/file.txt", "--version", "3"];
    let cli = Cli::parse_from(args);
    
    match cli.command {
        Commands::Restore { file_path, version, force } => {
            assert_eq!(file_path, PathBuf::from("/path/to/file.txt"));
            assert_eq!(version, 3);
            assert!(!force);
        }
        _ => panic!("Expected Restore command"),
    }
}

#[test]
fn test_cli_restore_force() {
    let args = vec!["claris-fuse", "restore", "--force", "/path/to/file.txt", "--version", "3"];
    let cli = Cli::parse_from(args);
    
    match cli.command {
        Commands::Restore { file_path, version, force } => {
            assert_eq!(file_path, PathBuf::from("/path/to/file.txt"));
            assert_eq!(version, 3);
            assert!(force);
        }
        _ => panic!("Expected Restore command"),
    }
}

#[test]
fn test_cli_global_options() {
    let args = vec!["claris-fuse", "--log-level", "debug", "--unmount-on-exit", "init", "/path/to/directory"];
    let cli = Cli::parse_from(args);
    
    assert_eq!(cli.log_level, Some("debug".to_string()));
    assert!(cli.unmount_on_exit);
    
    match cli.command {
        Commands::Init { directory } => {
            assert_eq!(directory, PathBuf::from("/path/to/directory"));
        }
        _ => panic!("Expected Init command"),
    }
}