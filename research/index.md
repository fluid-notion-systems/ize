# Claris-FUSE Research Index

This directory contains research and analysis for the Claris-FUSE project, organized by implementation priority and topic area.

## Implementation Roadmap

### Phase 1: Testing Framework (Current Priority)
- **[Testing Harness Framework](testing_harness_framework.md)** - Clean, DRY test architecture with harness structs
- **[Property-Based Testing Strategy](property_based_testing.md)** - Filesystem invariants and edge case testing

### Phase 2: OpCode Queue Refactoring  
- **[OpCode Queue Design](opcode_queue_design.md)** - Rename Command → OpCode, improve architecture
- **[Async Processing Pipeline](async_processing_pipeline.md)** - Background persistence and performance optimization

### Phase 3: Persistence Layer Research
- **[Rust Persistence Engines Comparison](rust_persistence_engines.md)** - Analysis of Sled, ReDB, Sanakirja, etc.
- **[Pijul Storage Architecture Analysis](pijul_storage_analysis.md)** - How Pijul solves version control storage
- **[Salsa Incremental Computation](salsa_incremental_computation.md)** - Lessons from rust-analyzer's approach

### Phase 4: Advanced Storage Implementation
- **[Custom Storage Engine Design](custom_storage_design.md)** - Pure Rust implementation possibilities
- **[Copy-on-Write Strategies](cow_strategies.md)** - Efficient file versioning approaches
- **[Delta Compression Techniques](delta_compression.md)** - Minimizing storage overhead

## Research Topics by Category

### Testing & Quality Assurance
- Testing Harness Framework
- Property-Based Testing Strategy

### Performance & Architecture  
- OpCode Queue Design
- Async Processing Pipeline
- Salsa Incremental Computation

### Storage & Persistence
- Rust Persistence Engines Comparison
- Pijul Storage Architecture Analysis
- Custom Storage Engine Design
- Copy-on-Write Strategies
- Delta Compression Techniques

## Next Steps

1. **Immediate (Week 1-2)**: Implement clean testing harness framework
2. **Short-term (Week 3-4)**: Refactor Command → OpCode queue system
3. **Medium-term (Month 2)**: Evaluate and integrate Pijul storage components
4. **Long-term (Month 3+)**: Custom pure-Rust persistence layer

## Dependencies & Integration Points

- **Pijul Integration**: Potential for reusing `libpijul` and `sanakirja` components
- **SeaORM Migration**: Plan for moving away from Diesel to SeaORM
- **FUSE Layer**: Maintain clean separation between storage and filesystem operations
- **CLI Interface**: Ensure storage changes don't break user-facing APIs

---

*Research is organized by implementation priority - start with testing framework, then move through the phases systematically.*