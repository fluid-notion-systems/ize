## File Operations to Support

The following FUSE filesystem operations will need to be implemented and tracked for version history:

### Core File Operations (Version Tracked)
1. **create** - Creating new files
2. **write** - Modifying file content
3. **unlink** - Deleting files
4. **rename** - Moving/renaming files
5. **truncate** - Resizing files (via setattr)
6. **mkdir** - Creating directories
7. **rmdir** - Removing directories
8. **symlink** - Creating symbolic links
9. **link** - Creating hard links
10. **setattr** - Setting file attributes (especially when changing size)

### Additional Operations (Metadata Only)
1. **chmod** - Changing permissions
2. **chown** - Changing ownership
3. **utimens** - Changing timestamps
4. **setxattr** - Setting extended attributes
5. **removexattr** - Removing extended attributes

### Read-Only Operations (No Versioning Required)
1. **lookup** - Looking up directory entries
2. **getattr** - Getting file attributes
3. **open** - Opening files
4. **read** - Reading file data
5. **readdir** - Reading directory contents
6. **readlink** - Reading symbolic link targets
7. **access** - Checking file permissions
8. **getxattr** - Getting extended attributes
9. **listxattr** - Listing extended attributes
10. **flush/fsync/release** - Managing file handles
