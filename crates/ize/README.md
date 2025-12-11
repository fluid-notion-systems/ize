
## ize_dump_opcode_queue

# Basic usage
cargo run --bin ize_dump_opcode_queue -- /tmp/source /tmp/mount

# With debug logging
cargo run --bin ize_dump_opcode_queue -- -l debug /tmp/source /tmp/mount

# Show raw bytes, more data
cargo run --bin ize_dump_opcode_queue -- --raw --max-bytes 500 /tmp/source /tmp/mount
