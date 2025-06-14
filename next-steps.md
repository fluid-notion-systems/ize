# Claris-FUSE Next Steps

## Current Status (Storage Refactor Branch)

- ✅ Removed Diesel and SQLite dependencies
- ✅ Created fresh test harness infrastructure in `tests/`
- ✅ Set up Nu shell development scripts
- ✅ Cleaned up old test directories
- 🔄 Storage backend awaiting implementation

## Immediate Next Steps

### 1. Storage Backend Implementation
- [ ] Design storage trait for versioning operations
- [ ] Implement in-memory storage for testing
- [ ] Add operation logging (create, update, delete, rename)
- [ ] Design schema for file version tracking
- [ ] Consider Sanakirja integration from Pijul

### 2. Core Filesystem Operations
- [ ] Hook PassthroughFS operations to storage layer
- [ ] Implement operation capture for:
  - [ ] File creation
  - [ ] File writes
  - [ ] File deletion
  - [ ] File moves/renames
  - [ ] Directory operations
- [ ] Add version querying interface

### 3. Testing Infrastructure
- [ ] Write unit tests for storage trait
- [ ] Add integration tests for version tracking
- [ ] Property-based tests for filesystem invariants
- [ ] Benchmark tests for performance baseline

### 4. CLI Commands
- [ ] Implement `claris-fuse history <path>`
- [ ] Implement `claris-fuse restore <path> --version=N`
- [ ] Add `claris-fuse status` to show tracked changes
- [ ] Create `claris-fuse diff` for comparing versions

## Development Roadmap

### Phase 1: Basic Versioning (Current)
1. Simple storage backend
2. Operation logging
3. Basic history viewing

### Phase 2: Advanced Storage
1. Evaluate Sanakirja from Pijul
2. Implement copy-on-write optimizations
3. Add compression for storage efficiency

### Phase 3: Performance & Features
1. Async operation queue optimization
2. Configurable retention policies
3. Export/import functionality

### Phase 4: Advanced Features
1. Branching/merging support
2. Distributed sync capabilities
3. AI-powered change descriptions

## Operational Protocols

### Development Workflow
1. **Make change** - Implement feature/fix in focused scope
2. **Test change** - Run `cargo test` to verify
3. **Commit** - Keep commits ATOMIC with clear messages

### Commit Guidelines
- One logical change per commit
- Clear, descriptive commit messages
- Format: `<type>: <description>`
  - `feat:` New feature
  - `fix:` Bug fix
  - `refactor:` Code restructuring
  - `test:` Test additions/changes
  - `docs:` Documentation updates

### Testing Protocol
- Write tests BEFORE implementing features when possible
- Run full test suite before committing
- Add regression tests for any bugs fixed

### Code Review Checklist
- [ ] Tests pass (`cargo test`)
- [ ] Code formatted (`cargo fmt`)
- [ ] No clippy warnings (`cargo clippy`)
- [ ] Documentation updated
- [ ] Commit is atomic
- [ ] Commit message is clear

## Technical Decisions Pending

### Storage Format
- **Option A**: Custom binary format
- **Option B**: Sanakirja B-tree
- **Option C**: Simple append-only log + index

### Operation Granularity
- Block-level changes vs file-level snapshots
- Deduplication strategy
- Metadata versioning approach

### API Design
- Synchronous vs async storage trait
- Error handling strategy
- Version identifier format (timestamps vs sequential IDs)

## Research Items

1. Study Pijul's Sanakirja usage
2. Investigate FUSE notification mechanisms
3. Explore incremental hashing strategies
4. Review other versioned filesystems (ZFS, Btrfs)

## Notes for Next Session

- Review storage trait design in `src/storage/mod.rs`
- Consider starting with simple append-only log
- Focus on correctness first, optimize later
- Keep version 1 scope minimal and focused

---

*Last updated: [Update this when making changes]*
*Current focus: Storage backend implementation*