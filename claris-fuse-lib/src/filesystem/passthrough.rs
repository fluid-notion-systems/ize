// Placeholder for passthrough filesystem implementation
// Will be implemented after adding fuser dependency

/// A basic passthrough filesystem
pub struct PassthroughFS {
    source_path: String,
}

impl PassthroughFS {
    pub fn new(source_path: String) -> Self {
        Self { source_path }
    }
    
    pub fn source_path(&self) -> &str {
        &self.source_path
    }
}