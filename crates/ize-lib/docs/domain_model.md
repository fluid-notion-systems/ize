# Domain Model Architecture for Ize

## Overview

The domain model represents the core business logic of the Ize filesystem. It separates the business rules from the technical implementation details through a layered architecture.

## Key Components

### Models

The models represent the core domain objects and value objects in the system:

- **FileVersion**: Represents a single version of a file at a point in time
- **VersionedFile**: A collection of versions for a file
- **FileChange**: Represents a change operation performed on a file
- **FileMetadata**: Contains metadata about a file at a specific version
- **OperationType**: Enum of possible operations (Create, Write, Delete, etc.)
- **VersionQuery**: A query object for searching versions with filters

### Repositories

The repositories define interfaces for data access, following the Repository pattern:

- **VersionRepository**: Interface for accessing file versions
- **SearchableVersionRepository**: Extended interface with search capabilities
- **FileSystemRepository**: Interface for basic filesystem operations
- **RepositoryFactory**: Factory for creating repositories

### Services

The services encapsulate business logic and orchestrate operations:

- **VersionService**: Service for managing file versions
- **SearchService**: Service for searching file versions
- **DefaultVersionService**: Default implementation of VersionService
- **DefaultSearchService**: Default implementation of SearchService
- **ServiceFactory**: Factory for creating services

## Architecture Diagram

```
+-------------------+     +-------------------+     +-------------------+
|                   |     |                   |     |                   |
|  Domain Models    |<----+  Domain Services  |<----+     FUSE          |
|                   |     |                   |     |   Filesystem      |
+-------------------+     +-------------------+     +-------------------+
           ^                       ^
           |                       |
           |                       |
           v                       v
+-------------------+     +-------------------+
|                   |     |                   |
|   Repositories    +---->+  Storage Layer    |
|                   |     |                   |
+-------------------+     +-------------------+
```

## Design Principles

### Clean Architecture

The domain model follows the principles of Clean Architecture:

1. **Independence of frameworks**: The domain model doesn't depend on external frameworks.
2. **Testability**: All components can be tested in isolation.
3. **Independence of UI**: The domain logic is separated from presentation concerns.
4. **Independence of database**: The domain doesn't know about the storage mechanism.
5. **Independence of external agencies**: The domain doesn't depend on external systems.

### Domain-Driven Design

Key DDD concepts applied:

- **Entities**: Objects with identity (FileVersion)
- **Value Objects**: Immutable objects without identity (FileMetadata)
- **Repositories**: Collection-like interfaces for retrieving domain objects
- **Services**: Operations that don't belong to a single entity
- **Factories**: Creation of complex objects or groups of objects

## Implementation Details

### Error Handling

The domain model uses a custom `DomainError` type that represents all possible error conditions in the domain. Each repository method returns a `RepositoryResult<T>` which is an alias for `Result<T, DomainError>`.

### Asynchronous API

All repository and service methods are asynchronous, using the `async`/`await` pattern with the `async_trait` macro.

### Standard Filesystem Implementation

The `StandardFileSystem` provides a concrete implementation of the `FileSystemRepository` interface using the standard library's filesystem operations.

## Usage Examples

### Creating a Version

```rust
let version_service = service_factory.create_version_service();
let version_id = version_service.create_version(
    "/path/to/file.txt",
    OperationType::Write,
    Some(file_content),
    None,
).await?;
```

### Retrieving File History

```rust
let version_service = service_factory.create_version_service();
let versioned_file = version_service.get_file_history("/path/to/file.txt").await?;

// Access versions
if let Some(latest) = versioned_file.latest_version() {
    println!("Latest version: {}", latest.id);
}
```

### Searching Versions

```rust
let search_service = service_factory.create_search_service();
let versions = search_service.search_by_text("important change").await?;

for version in versions {
    println!("Found version {} for file {}", version.id, version.path.display());
}
```

## Extending the Model

To extend the domain model:

1. Add new domain models or enrich existing ones in `models.rs`
2. Add new repository methods in the appropriate repository interfaces
3. Implement the new repository methods in concrete classes
4. Add new service methods to encapsulate business logic
5. Update the service implementations to use the new repository methods
