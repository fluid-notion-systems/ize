# Test Reorganization Plan for Ize

## Current Structure
Currently, our tests are spread across several files in the `tests/` directory without clear categorization, and we also have some unit tests embedded in source files using `#[cfg(test)]`. This makes it difficult to identify the purpose of each test, run specific test categories, and maintain proper test isolation.

## Proposed Structure
We'll reorganize tests into the following structure. Since Cargo's test discovery works best with top-level test files, we'll use a tagging system with consistent prefixes for better organization:

```
Ize-lib/
├── src/
│   └── ... (source files with embedded unit tests using #[cfg(test)])
├── tests/
│   ├── unit_*.rs      # Unit tests with 'unit_' prefix
│   ├── integration_*.rs  # Integration tests with 'integration_' prefix
│   ├── functional_*.rs   # Functional/end-to-end tests with 'functional_' prefix
│   ├── common/        # Shared test utilities and helpers
│   │   └── mod.rs
│   └── fixtures/      # Test data and fixtures
```

## Migration Plan

### 1. Create supporting directories
- Create `common/` and `fixtures/` directories under `tests/`

### 2. Establish naming convention
Each test file will be renamed according to its category with a consistent prefix:
- Unit tests: `unit_*.rs`
- Integration tests: `integration_*.rs`
- Functional tests: `functional_*.rs`

### 3. Rename existing tests following the convention
Based on the current test files, we'll rename them as follows:

#### Unit Tests
- `diesel_basic_test.rs` → `unit_diesel_basic_test.rs`
- `diesel_isolated.rs` → `unit_diesel_isolated_test.rs`
- `timestamp_test.rs` → `unit_timestamp_test.rs`
- `test_timestamp.rs` → `unit_timestamp_utils_test.rs`

#### Integration Tests
- `diesel_storage_test.rs` → `integration_diesel_storage_test.rs`
- `passthrough_test.rs` → `integration_passthrough_test.rs`
- `touch_test.rs` → `integration_touch_test.rs`

#### Functional Tests
- `mount_test.rs` → `functional_mount_test.rs`
- `cli_commands_test.rs` → `functional_cli_commands_test.rs`

#### Common Utilities
- `common.rs` → `common/mod.rs` (already moved)
- Add common test utilities and helper functions

### 4. Update imports and references
- Update import paths in all tests to reference the new locations/names
- Fix any broken references due to the restructuring

### 5. Add Documentation
- Add README.md files to the tests directory explaining the naming convention and categories
- Add appropriate module documentation to each test file

## Benefits
- Clear identification of test types through naming conventions
- Maintains compatibility with Cargo's test discovery
- Easier filtering of tests by category (e.g., `cargo test functional_`)
- Better organization for maintainability as the project grows
- Improved test isolation and independence
- Clearer guidelines for adding new tests

## Implementation Note
This approach maintains compatibility with Cargo's test discovery while providing a clear organizational structure. By using consistent prefixes, we get most of the benefits of subdirectories without fighting against Cargo's conventions.
