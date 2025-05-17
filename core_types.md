# Core Types for Claris FUSE Filesystem

## Architecture
Initially, the system will just store the full content on each change.

For the moment, we will just create the initial directory structure, files and metadata.

## File System Entities

### Directory
- `id`: BigInt (Primary Key)
- `path`: Text (Unique)
- `created_at`: BigInt (Unix timestamp)
- `metadata_id`: Foreign Key to Metadata

### File
- `id`: BigInt (Primary Key)
- `directory_id`: Bigint (Foreign Key to Directory)
- `created_at`: BigInt (Unix timestamp)
- `metadata_id`: Bigint (Foreign Key to Metadata)

### Metadata (shared between files and directories)
- `id`: BigInt (Primary Key)
- `mode`: Integer (File permissions)
- `uid`: Integer (User ID)
- `gid`: Integer (Group ID)
- `atime`: BigInt (Access time)
- `mtime`: BigInt (Modification time)
- `ctime`: BigInt (Change time - when metadata or content was last changed)

### Content
- `id`: BigInt (Primary Key)
<!-- - `version_id`: BigInt (Foreign Key to Version) -->
- `file_id`: BigInt (Foreign Key to File)
- `data`: Binary (The actual file content as raw bytes)

## Version Control (Do not implement this yet, i'm still working on the data model!)

### DirectoryVersion
- `id`: BigInt (Primary Key)
- `directory_id`: BigInt (Foreign Key to Directory)
- `metadata_id`: BigInt (Foreign Key to Metadata)

### FileVersion
- `id`: BigInt (Primary Key)
- `file_id`: BigInt (Foreign Key to File)
- `operation_type`: Text (Type of operation performed)
- `delta`: Binary (The binary delta representing the change)
- `timestamp`: BigInt (When operation was performed)
- `size`: BigInt (Size of the file at this version)

### Operation Types
- File Operations:
  - `FileCreate`: Create a new file
  - `FileWrite`: Write to an existing file
  - `FileDelete`: Delete a file
  - `FileTruncate`: Truncate a file to a specific size

- Directory Operations:
  - `DirCreate`: Create a new directory
  - `DirDelete`: Delete a directory

- Metadata Operations:
  - `Chmod`: Change file permissions
  - `Chown`: Change file ownership
  - `Utimens`: Change file times
  - `SetXattr`: Set extended attributes
  - `RemoveXattr`: Remove extended attributes
  - `Rename`: Rename a file or directory

- Link Operations: (not supported yet)
  - `Symlink`: Create a symbolic link
  - `Hardlink`: Create a hard link
