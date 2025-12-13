//! UI rendering module - Tabbed view with Projects and Channels

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table, Tabs},
    Frame,
};

use crate::app::{App, Channel, Mode, Tab};

/// Main render function
pub fn render(frame: &mut Frame, app: &App) {
    let mut constraints = vec![
        Constraint::Length(3), // Tab bar
    ];

    // Add channel sub-tabs row if on Channels tab
    if app.active_tab == Tab::Channels {
        constraints.push(Constraint::Length(2)); // Channel sub-tabs
    }

    constraints.push(Constraint::Min(0)); // Main content
    constraints.push(Constraint::Length(1)); // Status bar

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    let mut chunk_idx = 0;
    render_tab_bar(frame, chunks[chunk_idx], app);
    chunk_idx += 1;

    // Render channel sub-tabs if on Channels tab
    if app.active_tab == Tab::Channels {
        render_channel_tabs(frame, chunks[chunk_idx], app);
        chunk_idx += 1;
    }

    render_main_content(frame, chunks[chunk_idx], app);
    chunk_idx += 1;
    render_status_bar(frame, chunks[chunk_idx], app);

    // Render help overlay if in help mode
    if app.mode == Mode::Help {
        render_help_overlay(frame);
    }
}

/// Render the tab bar with tabs
fn render_tab_bar(frame: &mut Frame, area: Rect, app: &App) {
    let titles: Vec<Line> = Tab::all()
        .iter()
        .map(|tab| {
            let (base_style, hotkey_style) = if *tab == app.active_tab {
                (
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                )
            } else {
                (
                    Style::default().fg(Color::Gray),
                    Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::UNDERLINED),
                )
            };

            // Highlight the hotkey character in each tab name
            let spans = match tab {
                Tab::Projects => {
                    if let Some(project) = &app.selected_project {
                        vec![Span::raw(format!(" {} ", project.name))]
                    } else {
                        vec![
                            Span::raw(" "),
                            Span::styled("P", hotkey_style),
                            Span::styled("rojects ", base_style),
                        ]
                    }
                }
                Tab::Channels => vec![
                    Span::raw(" "),
                    Span::styled("C", hotkey_style),
                    Span::styled("hannels ", base_style),
                ],
            };

            Line::from(spans)
        })
        .collect();

    let selected = Tab::all()
        .iter()
        .position(|tab| *tab == app.active_tab)
        .unwrap_or(0);

    // Show selected project in title if on Channels tab
    let title = match (&app.active_tab, &app.selected_project) {
        (Tab::Channels, Some(project)) => {
            let name = project
                .source_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Unknown".to_string());
            format!(" Izev - {} ", name)
        }
        _ => format!(" Izev - {} ", app.repo_path),
    };

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .select(selected)
        .style(Style::default())
        .highlight_style(Style::default().fg(Color::Cyan));

    frame.render_widget(tabs, area);
}

/// Render the channel sub-tabs (only shown when on Channels tab)
fn render_channel_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let titles: Vec<Line> = Channel::all()
        .iter()
        .map(|ch| {
            let style = if *ch == app.active_channel {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(Span::styled(format!(" {} ", ch.name()), style))
        })
        .collect();

    let selected = Channel::all()
        .iter()
        .position(|ch| *ch == app.active_channel)
        .unwrap_or(0);

    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::BOTTOM))
        .select(selected)
        .style(Style::default())
        .highlight_style(Style::default().fg(Color::Yellow));

    frame.render_widget(tabs, area);
}

/// Render the main content area based on active tab
fn render_main_content(frame: &mut Frame, area: Rect, app: &App) {
    match app.active_tab {
        Tab::Projects => render_projects_view(frame, area, app),
        Tab::Channels => render_channels_view(frame, area, app),
    }
}

/// Render the projects list view
fn render_projects_view(frame: &mut Frame, area: Rect, app: &App) {
    if app.projects.is_empty() {
        let empty_msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No projects found",
                Style::default().fg(Color::Gray),
            )),
            Line::from(""),
            Line::from("To create a project, use:"),
            Line::from(Span::styled(
                "  ize init /path/to/directory",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press 'r' to refresh",
                Style::default().fg(Color::Gray),
            )),
        ])
        .block(
            Block::default()
                .title(" Projects ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        );
        frame.render_widget(empty_msg, area);
        return;
    }

    let header_cells = ["Name", "Source Directory", "UUID", "Created", "Channel"]
        .iter()
        .map(|h| {
            ratatui::widgets::Cell::from(*h).style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        });
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app
        .projects
        .iter()
        .enumerate()
        .map(|(i, project)| {
            let style = if i == app.projects_index {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            // Truncate UUID for display
            let uuid_short = if project.uuid.len() > 8 {
                format!("{}...", &project.uuid[..8])
            } else {
                project.uuid.clone()
            };

            let cells = vec![
                ratatui::widgets::Cell::from(project.name.clone())
                    .style(Style::default().fg(Color::Cyan)),
                ratatui::widgets::Cell::from(
                    project
                        .source_dir
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| project.source_dir.display().to_string()),
                )
                .style(Style::default().fg(Color::Gray)),
                ratatui::widgets::Cell::from(uuid_short).style(Style::default().fg(Color::Gray)),
                ratatui::widgets::Cell::from(project.created.clone())
                    .style(Style::default().fg(Color::Gray)),
                ratatui::widgets::Cell::from(project.default_channel.clone())
                    .style(Style::default().fg(Color::Green)),
            ];

            Row::new(cells).style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(15),
        Constraint::Min(15),
        Constraint::Length(12),
        Constraint::Length(20),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(format!(" Projects ({}) ", app.projects.len()))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    frame.render_widget(table, area);
}

/// Render the channels view - dispatches to specific channel views
fn render_channels_view(frame: &mut Frame, area: Rect, app: &App) {
    match app.active_channel {
        Channel::Stream => render_stream_channel(frame, area, app),
    }
}

/// Render the stream channel view - list of auto-ingested changes
fn render_stream_channel(frame: &mut Frame, area: Rect, app: &App) {
    // Show project context if selected
    let title = match &app.selected_project {
        Some(project) => {
            format!(" Izev - {} ", project.name)
        }
        None => " Stream (no project selected) ".to_string(),
    };

    if app.selected_project.is_none() {
        let no_project_msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No project selected",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from("Go to the Projects tab and select a project"),
            Line::from("to view its changes."),
            Line::from(""),
            Line::from(Span::styled(
                "Press Alt-P or Tab to switch tabs",
                Style::default().fg(Color::Gray),
            )),
        ])
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        );
        frame.render_widget(no_project_msg, area);
        return;
    }

    if app.changes.is_empty() {
        let empty_msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No changes recorded yet",
                Style::default().fg(Color::Gray),
            )),
            Line::from(""),
            Line::from("Changes will appear here as files are modified."),
        ])
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        );
        frame.render_widget(empty_msg, area);
        return;
    }

    let header_cells = ["ID", "Summary", "Time", "Files"].iter().map(|h| {
        ratatui::widgets::Cell::from(*h).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app
        .changes
        .iter()
        .enumerate()
        .map(|(i, change)| {
            let style = if i == app.changes_index {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let cells = vec![
                ratatui::widgets::Cell::from(change.id.clone())
                    .style(Style::default().fg(Color::Cyan)),
                ratatui::widgets::Cell::from(change.summary.clone()),
                ratatui::widgets::Cell::from(change.timestamp.clone())
                    .style(Style::default().fg(Color::Gray)),
                ratatui::widgets::Cell::from(format!("{}", change.files_changed))
                    .style(Style::default().fg(Color::Green)),
            ];

            Row::new(cells).style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Min(30),
        Constraint::Length(12),
        Constraint::Length(6),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    frame.render_widget(table, area);
}

/// Render the status bar
fn render_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let content = match app.mode {
        Mode::Command => {
            format!(":{}", app.command_buffer)
        }
        _ => app
            .status_message
            .clone()
            .unwrap_or_else(|| String::from("Ready")),
    };

    let mode_indicator = match app.mode {
        Mode::Normal => " NORMAL ",
        Mode::Command => " COMMAND ",
        Mode::Help => " HELP ",
    };

    let list_len = app.current_list_len();
    let position_info = if list_len > 0 {
        format!(" {}/{} ", app.current_index() + 1, list_len)
    } else {
        String::from(" 0/0 ")
    };

    let tab_indicator = format!(" {} ", app.active_tab.name());

    let status_style = Style::default().bg(Color::DarkGray).fg(Color::White);

    // Calculate layout for status bar
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(tab_indicator.len() as u16),
            Constraint::Length(position_info.len() as u16),
            Constraint::Length(mode_indicator.len() as u16),
        ])
        .split(area);

    let status = Paragraph::new(format!(" {}", content)).style(status_style);
    let tab =
        Paragraph::new(tab_indicator).style(Style::default().bg(Color::Magenta).fg(Color::White));
    let position =
        Paragraph::new(position_info).style(Style::default().bg(Color::Gray).fg(Color::Black));
    let mode =
        Paragraph::new(mode_indicator).style(Style::default().bg(Color::Blue).fg(Color::White));

    frame.render_widget(status, chunks[0]);
    frame.render_widget(tab, chunks[1]);
    frame.render_widget(position, chunks[2]);
    frame.render_widget(mode, chunks[3]);
}

/// Render the help overlay
fn render_help_overlay(frame: &mut Frame) {
    let area = centered_rect(60, 70, frame.area());

    let help_text = vec![
        Line::from(Span::styled(
            "Izev Help",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Navigation",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  j/↓        Move down"),
        Line::from("  k/↑        Move up"),
        Line::from("  g/Home     Go to top"),
        Line::from("  G/End      Go to bottom"),
        Line::from("  PgUp/PgDn  Page up/down"),
        Line::from("  Enter      Select item"),
        Line::from(""),
        Line::from(Span::styled(
            "Tabs",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  Tab        Next tab"),
        Line::from("  Alt-P      Jump to Projects"),
        Line::from("  Alt-C      Jump to Channels"),
        Line::from(""),
        Line::from(Span::styled(
            "Channels (within Channels tab)",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  ←/[        Previous channel"),
        Line::from("  →/]        Next channel"),
        Line::from(""),
        Line::from(Span::styled(
            "Projects Tab",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  r          Refresh projects list"),
        Line::from("  Enter      Select project & view channels"),
        Line::from(""),
        Line::from(Span::styled(
            "Commands",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  :          Enter command mode"),
        Line::from("  q          Quit"),
        Line::from("  ?/h        Show this help"),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to close",
            Style::default().fg(Color::Gray),
        )),
    ];

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .title(" Help ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().bg(Color::Black));

    // Clear the area first
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(help, area);
}

/// Helper function to create a centered rect
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
