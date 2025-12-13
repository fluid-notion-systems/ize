//! Application state management

use anyhow::Result;
use ize_lib::project::{ProjectInfo, ProjectManager};

use crate::event::Event;

/// Application running mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Normal browsing mode
    Normal,
    /// Command input mode
    Command,
    /// Help screen
    Help,
}

/// Available main tabs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Projects list - all tracked Ize projects
    Projects,
    /// Channels - view changes in different channels
    Channels,
}

/// Available channels within a project
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Stream channel - automatically ingested changes
    Stream,
    // Main - checkpointed/coalesced changes (future)
}

impl Tab {
    /// Get display name for the tab
    pub fn name(&self) -> &'static str {
        match self {
            Tab::Projects => "Projects",
            Tab::Channels => "Channels",
        }
    }

    /// Get all available tabs
    pub fn all() -> &'static [Tab] {
        &[Tab::Projects, Tab::Channels]
    }

    /// Get next tab
    pub fn next(&self) -> Tab {
        match self {
            Tab::Projects => Tab::Channels,
            Tab::Channels => Tab::Projects,
        }
    }

    /// Get previous tab
    pub fn prev(&self) -> Tab {
        match self {
            Tab::Projects => Tab::Channels,
            Tab::Channels => Tab::Projects,
        }
    }
}

impl Channel {
    /// Get display name for the channel
    pub fn name(&self) -> &'static str {
        match self {
            Channel::Stream => "Stream",
        }
    }

    /// Get all available channels
    pub fn all() -> &'static [Channel] {
        &[Channel::Stream]
    }

    /// Get next channel
    pub fn next(&self) -> Channel {
        match self {
            Channel::Stream => Channel::Stream, // Only one for now
        }
    }

    /// Get previous channel
    pub fn prev(&self) -> Channel {
        match self {
            Channel::Stream => Channel::Stream, // Only one for now
        }
    }
}

/// Represents a change/commit in a channel
#[derive(Debug, Clone)]
pub struct ChangeEntry {
    /// Unique identifier (hash or timestamp)
    pub id: String,
    /// Short description/summary
    pub summary: String,
    /// Timestamp of the change
    pub timestamp: String,
    /// Files affected
    pub files_changed: usize,
}

impl ChangeEntry {
    /// Create a placeholder change entry
    pub fn placeholder(id: &str, summary: &str, timestamp: &str, files: usize) -> Self {
        Self {
            id: id.to_string(),
            summary: summary.to_string(),
            timestamp: timestamp.to_string(),
            files_changed: files,
        }
    }
}

/// Main application state
pub struct App {
    /// Whether the application is running
    running: bool,
    /// Current mode
    pub mode: Mode,
    /// Active tab
    pub active_tab: Tab,
    /// Active channel (when on Channels tab)
    pub active_channel: Channel,
    /// Path to the repository (if opened directly)
    pub repo_path: String,
    /// Status message to display
    pub status_message: Option<String>,
    /// Command input buffer
    pub command_buffer: String,
    /// Selected index in projects list
    pub projects_index: usize,
    /// Selected index in the change list
    pub changes_index: usize,
    /// Project manager instance
    project_manager: Option<ProjectManager>,
    /// Cached list of projects
    pub projects: Vec<ProjectInfo>,
    /// Changes in the current channel (placeholder for now)
    pub changes: Vec<ChangeEntry>,
    /// Currently selected project (for Stream view)
    pub selected_project: Option<ProjectInfo>,
}

impl App {
    /// Create a new application instance
    pub fn new(repo_path: String) -> Self {
        // Try to create project manager
        let project_manager = ProjectManager::new().ok();

        // Load projects if manager available
        let projects = project_manager
            .as_ref()
            .and_then(|pm| pm.list_projects().ok())
            .unwrap_or_default();

        // Placeholder changes for demo
        let changes = vec![
            ChangeEntry::placeholder("a1b2c3d", "Auto-save: modified src/main.rs", "2 min ago", 1),
            ChangeEntry::placeholder(
                "e4f5g6h",
                "Auto-save: modified src/lib.rs, Cargo.toml",
                "5 min ago",
                2,
            ),
            ChangeEntry::placeholder(
                "i7j8k9l",
                "Auto-save: created src/utils.rs",
                "12 min ago",
                1,
            ),
            ChangeEntry::placeholder("m0n1o2p", "Auto-save: modified README.md", "25 min ago", 1),
            ChangeEntry::placeholder(
                "q3r4s5t",
                "Auto-save: deleted old_config.toml",
                "1 hour ago",
                1,
            ),
        ];

        Self {
            running: true,
            mode: Mode::Normal,
            active_tab: Tab::Projects,
            active_channel: Channel::Stream,
            repo_path,
            status_message: Some("Press '?' for help, 'q' to quit".to_string()),
            command_buffer: String::new(),
            projects_index: 0,
            changes_index: 0,
            project_manager,
            projects,
            changes,
            selected_project: None,
        }
    }

    /// Check if the application is still running
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Quit the application
    pub fn quit(&mut self) {
        self.running = false;
    }

    /// Set a status message
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = Some(message.into());
    }

    /// Clear the status message
    #[allow(dead_code)]
    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    /// Refresh the projects list
    pub fn refresh_projects(&mut self) {
        if let Some(pm) = &self.project_manager {
            if let Ok(projects) = pm.list_projects() {
                self.projects = projects;
                self.set_status(format!("Loaded {} projects", self.projects.len()));
            }
        }
    }

    /// Get the currently selected project
    pub fn selected_project_info(&self) -> Option<&ProjectInfo> {
        self.projects.get(self.projects_index)
    }

    /// Get the currently selected change, if any
    pub fn selected_change(&self) -> Option<&ChangeEntry> {
        self.changes.get(self.changes_index)
    }

    /// Get current list length based on active tab
    pub fn current_list_len(&self) -> usize {
        match self.active_tab {
            Tab::Projects => self.projects.len(),
            Tab::Channels => self.changes.len(),
        }
    }

    /// Get current selected index based on active tab
    pub fn current_index(&self) -> usize {
        match self.active_tab {
            Tab::Projects => self.projects_index,
            Tab::Channels => self.changes_index,
        }
    }

    /// Handle an input event
    pub fn handle_event(&mut self, event: Event) -> Result<()> {
        match self.mode {
            Mode::Normal => self.handle_normal_event(event),
            Mode::Command => self.handle_command_event(event),
            Mode::Help => self.handle_help_event(event),
        }
    }

    fn handle_normal_event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Quit => self.quit(),
            Event::Key(key) => match key {
                'q' => self.quit(),
                '?' | 'h' => self.mode = Mode::Help,
                ':' => {
                    self.mode = Mode::Command;
                    self.command_buffer.clear();
                }
                'j' | 'J' => self.move_down(),
                'k' | 'K' => self.move_up(),
                'g' => self.move_to_top(),
                'G' => self.move_to_bottom(),
                'r' | 'R' => self.refresh_projects(),
                '\t' => self.next_tab(),
                '[' => self.prev_channel(),
                ']' => self.next_channel(),
                _ => {}
            },
            Event::Tab => self.next_tab(),
            Event::AltP => {
                self.active_tab = Tab::Projects;
                self.set_status("Jumped to Projects");
            }
            Event::AltC => {
                self.active_tab = Tab::Channels;
                self.set_status("Jumped to Channels");
            }
            Event::Up => self.move_up(),
            Event::Down => self.move_down(),
            Event::Left => self.prev_channel(),
            Event::Right => self.next_channel(),
            Event::PageUp => self.page_up(),
            Event::PageDown => self.page_down(),
            Event::Home => self.move_to_top(),
            Event::End => self.move_to_bottom(),
            Event::Enter => self.select_item(),
            _ => {}
        }
        Ok(())
    }

    fn handle_command_event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Quit | Event::Escape => {
                self.mode = Mode::Normal;
                self.command_buffer.clear();
            }
            Event::Enter => {
                self.execute_command();
                self.mode = Mode::Normal;
            }
            Event::Backspace => {
                self.command_buffer.pop();
            }
            Event::Key(c) if !c.is_control() => {
                self.command_buffer.push(c);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_help_event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Quit | Event::Key('q') | Event::Key('?') | Event::Escape => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
        Ok(())
    }

    fn next_tab(&mut self) {
        self.active_tab = self.active_tab.next();
    }

    fn prev_tab(&mut self) {
        self.active_tab = self.active_tab.prev();
    }

    fn next_channel(&mut self) {
        if self.active_tab == Tab::Channels {
            self.active_channel = self.active_channel.next();
        }
    }

    fn prev_channel(&mut self) {
        if self.active_tab == Tab::Channels {
            self.active_channel = self.active_channel.prev();
        }
    }

    fn move_up(&mut self) {
        match self.active_tab {
            Tab::Projects => {
                self.projects_index = self.projects_index.saturating_sub(1);
            }
            Tab::Channels => {
                self.changes_index = self.changes_index.saturating_sub(1);
            }
        }
    }

    fn move_down(&mut self) {
        match self.active_tab {
            Tab::Projects => {
                if !self.projects.is_empty() {
                    self.projects_index = (self.projects_index + 1).min(self.projects.len() - 1);
                }
            }
            Tab::Channels => {
                if !self.changes.is_empty() {
                    self.changes_index = (self.changes_index + 1).min(self.changes.len() - 1);
                }
            }
        }
    }

    fn move_to_top(&mut self) {
        match self.active_tab {
            Tab::Projects => self.projects_index = 0,
            Tab::Channels => self.changes_index = 0,
        }
    }

    fn move_to_bottom(&mut self) {
        match self.active_tab {
            Tab::Projects => {
                if !self.projects.is_empty() {
                    self.projects_index = self.projects.len() - 1;
                }
            }
            Tab::Channels => {
                if !self.changes.is_empty() {
                    self.changes_index = self.changes.len() - 1;
                }
            }
        }
    }

    fn page_up(&mut self) {
        match self.active_tab {
            Tab::Projects => {
                self.projects_index = self.projects_index.saturating_sub(10);
            }
            Tab::Channels => {
                self.changes_index = self.changes_index.saturating_sub(10);
            }
        }
    }

    fn page_down(&mut self) {
        match self.active_tab {
            Tab::Projects => {
                if !self.projects.is_empty() {
                    self.projects_index = (self.projects_index + 10).min(self.projects.len() - 1);
                }
            }
            Tab::Channels => {
                if !self.changes.is_empty() {
                    self.changes_index = (self.changes_index + 10).min(self.changes.len() - 1);
                }
            }
        }
    }

    fn select_item(&mut self) {
        match self.active_tab {
            Tab::Projects => {
                if let Some(project) = self.selected_project_info().cloned() {
                    self.selected_project = Some(project.clone());
                    self.set_status(format!("Selected: {}", project.source_dir.display()));
                    // Switch to Channels tab to view this project's changes
                    self.active_tab = Tab::Channels;
                    // TODO: Load actual changes for this project
                }
            }
            Tab::Channels => {
                if let Some(change) = self.selected_change() {
                    self.set_status(format!("Viewing change: {}", change.id));
                    // TODO: Open detail view for the selected change
                }
            }
        }
    }

    fn execute_command(&mut self) {
        let cmd = self.command_buffer.trim();
        match cmd {
            "q" | "quit" => self.quit(),
            "help" => self.mode = Mode::Help,
            "projects" => {
                self.active_tab = Tab::Projects;
                self.set_status("Switched to Projects");
            }
            "channels" | "stream" => {
                self.active_tab = Tab::Channels;
                self.set_status("Switched to Channels");
            }
            "refresh" | "r" => {
                self.refresh_projects();
            }
            _ => {
                self.set_status(format!("Unknown command: {}", cmd));
            }
        }
        self.command_buffer.clear();
    }
}
