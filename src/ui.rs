use crate::{
    app::{App, InputMode, Screen},
    model::{MediaKind, Post},
};
use anyhow::Result;
use chrono::Utc;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};
use std::{
    io::{self, stdout},
    time::Duration,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// XTUI's visual language: pure-black canvas, crisp right-angle borders and a
// single X-blue accent. Color is spent only on focus, actions and status —
// everything else stays monochrome, like X itself.
const WHITE: Color = Color::Rgb(231, 233, 234);
const GRAY: Color = Color::Rgb(113, 118, 123);
const DIM: Color = Color::Rgb(43, 46, 50);
const SURFACE: Color = Color::Rgb(9, 9, 11);
const SURFACE_RAISED: Color = Color::Rgb(24, 26, 30);
const ACCENT: Color = Color::Rgb(29, 155, 240);
const GREEN: Color = Color::Rgb(0, 186, 124);
const AMBER: Color = Color::Rgb(255, 212, 0);
const RED: Color = Color::Rgb(244, 33, 46);
const BACKGROUND: Color = Color::Rgb(0, 0, 0);

pub async fn run(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let restore = TerminalRestore;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    // Paint the initial frame before bootstrap: a browser-companion connect
    // can take several seconds and must never leave a blank alternate screen.
    terminal.draw(|frame| draw(frame, app))?;
    app.bootstrap().await;
    let result = event_loop(&mut terminal, app).await;
    terminal.show_cursor()?;
    drop(restore);
    result
}

struct TerminalRestore;
impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    terminal.draw(|frame| draw(frame, app))?;
    while !app.should_quit {
        if event::poll(Duration::from_secs(30))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(terminal, app, key).await
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        if app.will_load_more(1) {
                            busy_draw(terminal, app, "Loading more posts…");
                        }
                        app.advance(1).await
                    }
                    MouseEventKind::ScrollUp => app.move_selection(-1),
                    _ => {}
                },
                _ => {}
            }
            if !app.should_quit {
                terminal.draw(|frame| draw(frame, app))?;
            }
        } else {
            // Relative timestamps only need a low-frequency refresh. Idle XTUI
            // otherwise performs no layout or terminal writes.
            terminal.draw(|frame| draw(frame, app))?;
        }
    }
    Ok(())
}

/// Show `status` and repaint before an operation that may block on the network.
/// Without this, a slow browser-mode fetch leaves the last frame on screen with
/// no indication that work is in progress.
fn busy_draw<B: Backend>(terminal: &mut Terminal<B>, app: &mut App, status: &str) {
    app.status = status.to_owned();
    let _ = terminal.draw(|frame| draw(frame, app));
}

async fn handle_key<B: Backend>(terminal: &mut Terminal<B>, app: &mut App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }
    if app.media_preview.is_some() || app.help {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
        ) {
            app.media_preview = None;
            app.help = false;
        }
        return;
    }
    if app.mode == InputMode::Search {
        match key.code {
            KeyCode::Enter => {
                if !app.query.trim().is_empty() {
                    busy_draw(terminal, app, "Searching…");
                }
                app.submit_search().await
            }
            KeyCode::Esc => {
                app.mode = InputMode::Normal;
                if app.history.is_empty() {
                    busy_draw(terminal, app, "Loading…");
                    app.root(Screen::Home).await;
                } else {
                    app.back();
                }
            }
            KeyCode::Left => app.move_search_cursor(-1),
            KeyCode::Right => app.move_search_cursor(1),
            KeyCode::Home => app.search_cursor = 0,
            KeyCode::End => app.search_cursor = app.query.chars().count(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.clear_query()
            }
            KeyCode::Backspace => app.backspace_query(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.insert_query_char(c)
            }
            _ => {}
        }
        return;
    }
    if app.nav_focused {
        match key.code {
            KeyCode::Up => app.nav_move(-1),
            KeyCode::Down => app.nav_move(1),
            KeyCode::Right | KeyCode::Enter => {
                busy_draw(terminal, app, "Loading…");
                app.nav_activate().await
            }
            KeyCode::Left | KeyCode::Esc | KeyCode::Backspace | KeyCode::Tab => {
                app.nav_focused = false
            }
            KeyCode::Char('q') => app.should_quit = true,
            KeyCode::Char('?') => app.help = true,
            KeyCode::Char('/') => {
                app.nav_focused = false;
                app.begin_search()
            }
            KeyCode::Char('1') => {
                app.nav_focused = false;
                busy_draw(terminal, app, "Loading…");
                app.root(Screen::Home).await
            }
            KeyCode::Char('2') => {
                app.nav_focused = false;
                busy_draw(terminal, app, "Loading…");
                app.root(Screen::Explore).await
            }
            KeyCode::Char('3') => {
                app.nav_focused = false;
                busy_draw(terminal, app, "Loading…");
                app.root(Screen::Mentions).await
            }
            KeyCode::Char('4') => {
                app.nav_focused = false;
                busy_draw(terminal, app, "Loading…");
                app.root(Screen::Bookmarks).await
            }
            KeyCode::Char('5') => {
                app.nav_focused = false;
                busy_draw(terminal, app, "Loading…");
                app.root(Screen::Lists).await
            }
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Tab => app.toggle_nav_focus(),
        KeyCode::Down => {
            if app.will_load_more(1) {
                busy_draw(terminal, app, "Loading more posts…");
            }
            app.advance(1).await
        }
        KeyCode::Up => app.move_selection(-1),
        KeyCode::PageDown => {
            if app.will_load_more(5) {
                busy_draw(terminal, app, "Loading more posts…");
            }
            app.advance(5).await
        }
        KeyCode::PageUp => app.move_selection(-5),
        KeyCode::Right | KeyCode::Enter => {
            let will_navigate = match &app.screen {
                Screen::Lists => app.selected_list().is_some(),
                _ => app.selected_post().is_some(),
            };
            if will_navigate {
                busy_draw(terminal, app, "Loading…");
            }
            app.activate().await
        }
        KeyCode::Left | KeyCode::Esc | KeyCode::Backspace => app.back(),
        KeyCode::Char('/') => app.begin_search(),
        KeyCode::Char('1') => {
            busy_draw(terminal, app, "Loading…");
            app.root(Screen::Home).await
        }
        KeyCode::Char('2') => {
            busy_draw(terminal, app, "Loading…");
            app.root(Screen::Explore).await
        }
        KeyCode::Char('3') => {
            busy_draw(terminal, app, "Loading…");
            app.root(Screen::Mentions).await
        }
        KeyCode::Char('4') => {
            busy_draw(terminal, app, "Loading…");
            app.root(Screen::Bookmarks).await
        }
        KeyCode::Char('5') => {
            busy_draw(terminal, app, "Loading…");
            app.root(Screen::Lists).await
        }
        KeyCode::Char('p') => {
            if app.selected_post().is_some() || app.me.is_some() {
                busy_draw(terminal, app, "Loading…");
            }
            app.open_profile().await
        }
        KeyCode::Char('L') => {
            if app.selected_post().is_some() || matches!(app.screen, Screen::Profile(_)) {
                busy_draw(terminal, app, "Loading…");
            }
            app.open_likes().await
        }
        KeyCode::Char('r') => {
            busy_draw(terminal, app, "Loading…");
            app.refresh().await
        }
        KeyCode::Char('m') => {
            if app
                .selected_post()
                .is_some_and(|post| !post.media.is_empty())
            {
                busy_draw(terminal, app, "Rendering media preview…");
            }
            app.preview_media().await
        }
        KeyCode::Char('v') => app.open_external(true),
        KeyCode::Char('o') => app.open_external(false),
        KeyCode::Char('?') => app.help = true,
        KeyCode::Char(' ') if matches!(app.screen, Screen::Thread(_)) => {
            app.thread_expanded = !app.thread_expanded;
            if app.thread_expanded {
                app.selected = 0;
                app.status = "Replies expanded · Space collapses".into();
            } else {
                app.selected = 0;
                app.status = "Replies collapsed · Space expands".into();
            }
        }
        _ => {}
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKGROUND).fg(WHITE)),
        area,
    );
    if area.width >= 106 {
        let cols = Layout::horizontal([
            Constraint::Length(20),
            Constraint::Length(1),
            Constraint::Min(50),
            Constraint::Length(1),
            Constraint::Length(34),
        ])
        .split(area);
        render_nav(frame, cols[0], app, false);
        render_center(frame, cols[2], app);
        render_context(frame, cols[4], app);
    } else if area.width >= 94 {
        let cols = Layout::horizontal([
            Constraint::Length(11),
            Constraint::Length(1),
            Constraint::Min(50),
            Constraint::Length(1),
            Constraint::Length(31),
        ])
        .split(area);
        render_nav(frame, cols[0], app, true);
        render_center(frame, cols[2], app);
        render_context(frame, cols[4], app);
    } else if area.width >= 78 {
        let cols = Layout::horizontal([
            Constraint::Length(11),
            Constraint::Length(1),
            Constraint::Min(50),
        ])
        .split(area);
        render_nav(frame, cols[0], app, true);
        render_center(frame, cols[2], app);
    } else {
        let rows = Layout::vertical([Constraint::Min(10), Constraint::Length(3)]).split(area);
        render_center(frame, rows[0], app);
        render_bottom_nav(frame, rows[1], app);
    }
    if app.help {
        render_help(frame, area);
    }
    if let Some((alt, lines)) = &app.media_preview {
        render_media(frame, area, alt, lines);
    }
}

fn render_nav(frame: &mut Frame, area: Rect, app: &App, compact: bool) {
    let active = app.nav_index();
    let items: &[(&str, &str)] = &[
        ("⌂", "Home"),
        ("◇", "Explore"),
        ("@", "Mentions"),
        ("◆", "Bookmarks"),
        ("≡", "Lists"),
    ];
    let mut lines = Vec::with_capacity(items.len() * 2 + 2);
    lines.push(Line::from(Span::styled(
        if compact { "  𝕏  " } else { "  𝕏  XTUI" },
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    for (index, (icon, label)) in items.iter().enumerate() {
        let is_active = index == active;
        let is_focused = app.nav_focused && index == app.nav_selected;
        // While the sidebar owns focus, only the focused item keeps the accent
        // bar; the current section drops to a neutral marker so focus pops.
        let bar = if is_focused || (is_active && !app.nav_focused) {
            "▍"
        } else {
            " "
        };
        let text = if compact {
            format!("{bar} {icon}")
        } else {
            format!("{bar} {icon} {label}")
        };
        let style = if is_focused {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(GRAY)
        };
        lines.push(Line::from(Span::styled(text, style)));
        lines.push(Line::from(""));
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(BACKGROUND)),
        area,
    );
    if !compact {
        let width = area.width as usize;
        let (name, handle) = match app.me.as_ref() {
            Some(user) => (
                truncate_to_width(&user.name, width.saturating_sub(4)),
                truncate_to_width(&user.username, width.saturating_sub(4)),
            ),
            None => ("Demo".into(), "offline account".into()),
        };
        let a = Rect {
            x: area.x,
            y: area.bottom().saturating_sub(4),
            width: area.width,
            height: 3,
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("●  ", Style::default().fg(GREEN)),
                    Span::styled(
                        name,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    format!("   @{handle}"),
                    Style::default().fg(GRAY),
                )),
            ])
            .style(Style::default().bg(BACKGROUND)),
            a,
        );
    }
}

fn render_bottom_nav(frame: &mut Frame, area: Rect, app: &App) {
    let active = app.nav_index();
    let labels = ["⌂ Home", "◇ Search", "@ Mentions", "◆ Saved", "≡ Lists"];
    let line = Line::from(
        labels
            .iter()
            .enumerate()
            .flat_map(|(i, l)| {
                [
                    Span::styled(
                        format!(" {l} "),
                        if i == active {
                            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(GRAY)
                        },
                    ),
                    Span::raw(" "),
                ]
            })
            .collect::<Vec<_>>(),
    );
    let keys = Line::from(vec![
        Span::styled(
            "↑↓",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" move  ", Style::default().fg(GRAY)),
        Span::styled(
            "→",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" open  ", Style::default().fg(GRAY)),
        Span::styled(
            "←",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" back  / search  ? keys  Q quit", Style::default().fg(GRAY)),
    ])
    .alignment(Alignment::Right);
    frame.render_widget(
        Paragraph::new(vec![line, keys])
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(DIM)),
            ),
        area,
    );
}

fn render_center(frame: &mut Frame, area: Rect, app: &App) {
    let panel = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .style(Style::default().bg(SURFACE));
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    let chunks = Layout::vertical([
        Constraint::Length(header_height(app)),
        Constraint::Min(4),
        Constraint::Length(2),
    ])
    .split(inner);
    render_header(frame, chunks[0], app);
    if matches!(app.screen, Screen::Lists) {
        render_lists(frame, chunks[1], app);
    } else {
        render_posts(frame, chunks[1], app);
    }
    let mut status = app.error.as_deref().unwrap_or(&app.status).to_owned();
    if app.demo {
        status.push_str("  ·  login: xtui login CLIENT_ID");
    }
    let status_style = if app.error.is_some() {
        Style::default().fg(RED).add_modifier(Modifier::BOLD)
    } else if app.demo {
        Style::default().fg(AMBER)
    } else {
        Style::default().fg(GREEN)
    };
    // Long API errors previously vanished behind the status bar's right edge.
    // Truncate by display width with an ellipsis, and drop the shortcut hints
    // entirely on terminals too narrow to fit both.
    const HINTS: &str = "   ↑↓ move  → open  ← back  / search ";
    let hints_width = HINTS.width();
    let width = chunks[2].width as usize;
    let available = width.saturating_sub(3 + hints_width);
    let mut spans = vec![Span::styled(" ● ", status_style)];
    if available >= 24 {
        spans.push(Span::styled(
            truncate_to_width(&status, available),
            Style::default().fg(GRAY),
        ));
        spans.push(Span::styled(HINTS, Style::default().fg(DIM)));
    } else {
        spans.push(Span::styled(
            truncate_to_width(&status, width.saturating_sub(3)),
            Style::default().fg(GRAY),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Left)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(DIM)),
            ),
        chunks[2],
    );
}

fn header_height(app: &App) -> u16 {
    match app.screen {
        Screen::Profile(_) => 7,
        Screen::ListFeed(_) => 5,
        _ => 3,
    }
}
fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            if app.history.is_empty() { "  " } else { "← " },
            Style::default().fg(ACCENT),
        ),
        Span::styled(
            app.title(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    let mut wrap = false;
    match &app.screen {
        Screen::Home => lines.push(Line::from(Span::styled(
            "  ━━━  Following  ━━━",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))),
        Screen::Explore if app.mode == InputMode::Search => lines.push(Line::from(vec![
            Span::styled("  Search X  ", Style::default().fg(ACCENT)),
            Span::styled(
                app.search_display(),
                Style::default().fg(Color::White).bg(SURFACE_RAISED),
            ),
        ])),
        Screen::Explore => lines.push(Line::from(vec![
            Span::styled("  Search: ", Style::default().fg(ACCENT)),
            Span::styled(app.query.clone(), Style::default().fg(Color::White)),
        ])),
        Screen::Profile(u) => {
            wrap = true;
            lines.push(Line::from(Span::styled(
                format!("  @{}{}", u.username, if u.verified { "  ✓" } else { "" }),
                Style::default().fg(GRAY),
            )));
            lines.push(Line::from(Span::styled(
                format!("  {}", u.description),
                Style::default().fg(GRAY),
            )));
            if let Some(m) = &u.public_metrics {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {} ", compact(m.following_count)),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("Following   ", Style::default().fg(GRAY)),
                    Span::styled(
                        format!("{} ", compact(m.followers_count)),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("Followers   L liked posts", Style::default().fg(GRAY)),
                ]));
            }
        }
        Screen::ListFeed(l) => {
            lines.push(Line::from(Span::styled(
                format!("  {}", l.description),
                Style::default().fg(GRAY),
            )));
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} members  ·  {} followers",
                    l.member_count.unwrap_or(0),
                    l.follower_count.unwrap_or(0)
                ),
                Style::default().fg(GRAY),
            )));
        }
        Screen::Thread(_) => {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} posts · Space {}",
                    app.posts.len(),
                    if app.thread_expanded {
                        "collapses replies"
                    } else {
                        "expands replies"
                    }
                ),
                Style::default().fg(GRAY),
            )));
        }
        Screen::Likes(user) => {
            lines.push(Line::from(Span::styled(
                format!("  posts liked by @{}", user.username),
                Style::default().fg(GRAY),
            )));
        }
        _ => {}
    }
    let mut paragraph = Paragraph::new(lines).style(Style::default().fg(WHITE));
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(
        paragraph.block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(DIM)),
        ),
        area,
    );
}

fn render_posts(frame: &mut Frame, area: Rect, app: &App) {
    let collapsed = matches!(app.screen, Screen::Thread(_)) && !app.thread_expanded;
    let selected = if collapsed {
        0
    } else {
        app.selected.min(app.posts.len().saturating_sub(1))
    };
    if app.posts.is_empty() {
        let message: String = if matches!(app.screen, Screen::Explore)
            && app.mode == InputMode::Normal
            && !app.query.trim().is_empty()
        {
            format!(
                "  No results for “{}”.\n  Press / to search again.",
                app.query.trim()
            )
        } else if app.mode == InputMode::Search {
            "  Type a query and press Enter to search X.".into()
        } else if matches!(app.screen, Screen::Thread(_)) {
            "  This conversation has no posts.".into()
        } else {
            "  Nothing here yet.\n  Press r to refresh or / to search.".into()
        };
        frame.render_widget(
            Paragraph::new(message).style(Style::default().fg(GRAY)),
            area,
        );
        return;
    }
    // Cards are stacked manually: each card owns a sharp border, and the
    // focused card gets the X-blue accent frame. The feed window begins at the
    // selected post, so reading position stays pinned while rendering stays
    // proportional to the viewport, not the full (hundreds-post) timeline.
    let width = area.width.saturating_sub(4) as usize;
    let mut y = area.y;
    for (index, post) in app
        .posts
        .iter()
        .enumerate()
        .skip(selected)
        .take(if collapsed { 1 } else { 12 })
    {
        let focused = index == selected;
        let reply = matches!(app.screen, Screen::Thread(_)) && index > 0;
        let lines = post_lines(post, width, reply, focused, if focused { 8 } else { 2 });
        let remaining = area.bottom().saturating_sub(y);
        // Skip cards that cannot fit at least one content row between their
        // borders; a border-only sliver looks like a rendering bug.
        if remaining < 3 {
            break;
        }
        let height = (lines.len() as u16).min(remaining);
        let card = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused { ACCENT } else { DIM }))
            .style(Style::default().bg(if focused { SURFACE_RAISED } else { SURFACE }));
        frame.render_widget(
            Paragraph::new(lines).block(card),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height,
            },
        );
        y += height;
        if y >= area.bottom() {
            break;
        }
    }
}

fn post_lines(
    post: &Post,
    width: usize,
    reply: bool,
    focused: bool,
    body_limit: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if focused {
        lines.push(Line::from(vec![
            Span::styled(
                "  ▶ NOW READING  ",
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("→ open conversation", Style::default().fg(GRAY)),
        ]));
    }
    if post.reposted.is_some() {
        lines.push(Line::from(Span::styled(
            "  ↻  Reposted",
            Style::default().fg(GRAY),
        )));
    }
    let prefix = if reply { "  └─ " } else { "  ● " };
    lines.push(Line::from(vec![
        Span::styled(
            format!("{prefix}{}", post.author.name),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        if post.author.verified {
            Span::styled(
                " ✓",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("")
        },
        Span::styled(
            format!(
                "  @{} · {}",
                post.author.username,
                relative_time(post.created_at)
            ),
            Style::default().fg(GRAY),
        ),
    ]));
    let target = post.reposted.as_deref().unwrap_or(post);
    let wrapped = textwrap::wrap(&target.text, width.max(10));
    let clipped = wrapped.len() > body_limit;
    for line in wrapped.into_iter().take(body_limit) {
        lines.push(Line::from(format!("  {line}")));
    }
    if clipped {
        lines.push(Line::from(Span::styled(
            "  …  → open to read the rest",
            Style::default().fg(GRAY),
        )));
    }
    if focused && let Some(q) = &post.quoted {
        lines.push(Line::from(vec![
            Span::styled("  ┌ ", Style::default().fg(DIM)),
            Span::styled(
                q.author.name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  @{}", q.author.username),
                Style::default().fg(ACCENT),
            ),
        ]));
        for wrapped in textwrap::wrap(&q.text, width.saturating_sub(4).max(10))
            .into_iter()
            .take(2)
        {
            lines.push(Line::from(Span::styled(
                format!("  │ {wrapped}"),
                Style::default().fg(WHITE),
            )));
        }
        lines.push(Line::from(Span::styled("  └", Style::default().fg(DIM))));
    }
    if let Some(m) = target.media.first() {
        let badge = match m.kind {
            MediaKind::Photo => "▧ PHOTO",
            MediaKind::Video => "▶ VIDEO",
            MediaKind::AnimatedGif => "▶ GIF",
        };
        lines.push(Line::from(vec![
            Span::styled("  ▣  ", Style::default().fg(GRAY)),
            Span::styled(
                badge,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  {}",
                    m.alt_text.as_deref().unwrap_or("M preview · V open")
                ),
                Style::default().fg(GRAY),
            ),
        ]));
    }
    let metrics = [
        ("◯", compact(target.metrics.reply_count)),
        ("↻", compact(target.metrics.retweet_count)),
        ("♡", compact(target.metrics.like_count)),
        ("◉", compact(target.metrics.impression_count.unwrap_or(0))),
    ];
    let mut metric_line = String::from("  ");
    for (icon, number) in metrics {
        metric_line.push_str(icon);
        metric_line.push(' ');
        metric_line.push_str(&number);
        metric_line.push_str("   ");
    }
    lines.push(Line::from(Span::styled(
        metric_line,
        Style::default().fg(GRAY),
    )));
    lines
}

fn render_lists(frame: &mut Frame, area: Rect, app: &App) {
    if app.lists.is_empty() {
        frame.render_widget(
            Paragraph::new("  No lists yet.\n  Press r to refresh.")
                .style(Style::default().fg(GRAY)),
            area,
        );
        return;
    }
    let mut y = area.y;
    for (offset, list) in app.lists.iter().skip(app.selected).take(12).enumerate() {
        let focused = offset == 0;
        let lines = vec![
            Line::from(vec![
                Span::styled(
                    "  ",
                    Style::default().fg(if focused { ACCENT } else { GRAY }),
                ),
                Span::styled(
                    format!("{}{}", list.name, if list.private { "  🔒" } else { "" }),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(
                format!("  {}", list.description),
                Style::default().fg(GRAY),
            )),
            Line::from(Span::styled(
                format!(
                    "  {} members · {} followers",
                    list.member_count.unwrap_or(0),
                    list.follower_count.unwrap_or(0)
                ),
                Style::default().fg(GRAY),
            )),
        ];
        let remaining = area.bottom().saturating_sub(y);
        if remaining < 3 {
            break;
        }
        let height = (lines.len() as u16).min(remaining);
        let card = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused { ACCENT } else { DIM }))
            .style(Style::default().bg(if focused { SURFACE_RAISED } else { SURFACE }));
        frame.render_widget(
            Paragraph::new(lines).block(card),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height,
            },
        );
        y += height;
        if y >= area.bottom() {
            break;
        }
    }
}

fn render_context(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::vertical([
        Constraint::Min(8),
        Constraint::Length(1),
        Constraint::Length(22),
    ])
    .split(area);
    let selected = app.selected_post();
    let position = if app.posts.is_empty() {
        "No post selected".into()
    } else {
        format!("POST {} OF {}", app.selected + 1, app.posts.len())
    };
    let summary = vec![
        Line::from(Span::styled(
            position,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            selected
                .map(|post| post.author.name.clone())
                .unwrap_or_else(|| "Navigate with ↑ and ↓".into()),
            Style::default().fg(WHITE).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            selected
                .map(|post| format!("@{}", post.author.username))
                .unwrap_or_default(),
            Style::default().fg(GRAY),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                selected
                    .map(|p| compact(p.metrics.reply_count))
                    .unwrap_or_default(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" replies   ", Style::default().fg(GRAY)),
            Span::styled(
                selected
                    .map(|p| compact(p.metrics.like_count))
                    .unwrap_or_default(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" likes", Style::default().fg(GRAY)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "The selected post stays pinned at the top of the feed.",
            Style::default().fg(GRAY),
        )),
    ];
    frame.render_widget(
        Paragraph::new(summary).wrap(Wrap { trim: false }).block(
            Block::default()
                .title("  ▍ CURRENT  ")
                .title_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DIM))
                .padding(Padding::uniform(1))
                .style(Style::default().bg(SURFACE)),
        ),
        rows[0],
    );

    let shortcuts = vec![
        shortcut_line("Tab", "Sidebar"),
        shortcut_line("↑ / ↓", "Previous / next"),
        shortcut_line("→", "Open post or list"),
        shortcut_line("←", "Go back"),
        shortcut_line("PgUp/Dn", "Jump five posts"),
        shortcut_line("/", "Search X"),
        shortcut_line("1…5", "Switch section"),
        shortcut_line("R", "Refresh"),
        shortcut_line("P / L", "Profile / likes"),
        shortcut_line("M / V", "Preview media"),
        shortcut_line("O", "Open on x.com"),
        shortcut_line("Space", "Collapse replies"),
        shortcut_line("?", "Full help"),
        shortcut_line("Q", "Quit"),
    ];
    frame.render_widget(
        Paragraph::new(shortcuts).block(
            Block::default()
                .title("  KEYBOARD  ")
                .title_style(Style::default().fg(GRAY).add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DIM))
                .padding(Padding::horizontal(1))
                .style(Style::default().bg(SURFACE)),
        ),
        rows[2],
    );
}

fn shortcut_line(key: &'static str, action: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:<8}"),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(action, Style::default().fg(GRAY)),
    ])
}

fn render_help(frame: &mut Frame, area: Rect) {
    // The popup must be wide enough that the two-column layout never wraps;
    // at 66 columns its content area is only 60, so use 72.
    let popup = centered(area, 72, 21);
    frame.render_widget(Clear, popup);
    let text = "Tab        Sidebar focus           ↑ / ↓      Move in sidebar\n→ / Enter  Open post or selection     ← / Esc    Back / leave\nPgUp/PgDn  Jump five posts            /          Search X\n1…5        Switch section             ?          This help\nP / L      Profile / likes            M / V      Media preview\nR          Refresh                    O          Open on x.com\nSpace      Collapse thread replies    Ctrl+U     Clear search\nQ          Quit\n\nMouse wheel also moves the selection. The active post stays pinned\nto the top; previews underneath show what comes next.\n\nQ / Esc / ?   Close this help";
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(WHITE))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title("  KEYBOARD SHORTCUTS  ")
                    .title_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(DIM))
                    .style(Style::default().bg(SURFACE_RAISED))
                    .padding(Padding::uniform(2)),
            ),
        popup,
    );
}
fn render_media(frame: &mut Frame, area: Rect, alt: &str, lines: &[String]) {
    // Size the popup from the artwork's display width (not char count, which
    // miscounts wide glyphs) and cap it to the terminal so tiny windows never
    // produce an overflowing popup.
    let art_width = lines.iter().map(|line| line.width()).max().unwrap_or(20);
    let popup = centered(
        area,
        (art_width as u16 + 6).min(area.width.saturating_sub(2)),
        (lines.len() as u16 + 7).min(area.height.saturating_sub(2)),
    );
    frame.render_widget(Clear, popup);
    let content_width = popup.width.saturating_sub(4) as usize;
    let mut text = lines
        .iter()
        .map(|line| {
            Line::from(Span::styled(
                truncate_to_width(line, content_width),
                Style::default().fg(Color::White),
            ))
        })
        .collect::<Vec<_>>();
    text.push(Line::from(""));
    text.push(Line::from(Span::styled(
        truncate_to_width(alt, content_width),
        Style::default().fg(GRAY),
    )));
    text.push(Line::from(Span::styled(
        "V open in browser · O open post · Esc close",
        Style::default().fg(DIM),
    )));
    frame.render_widget(
        Paragraph::new(text).alignment(Alignment::Center).block(
            Block::default()
                .title("  ▍ MEDIA  · Esc to close  ")
                .title_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DIM))
                .padding(Padding::uniform(1)),
        ),
        popup,
    );
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

/// Cut `text` to at most `max` display columns, ending with an ellipsis when it
/// had to be shortened. Wide glyphs count as two columns.
fn truncate_to_width(text: &str, max: usize) -> String {
    if max == 0 || text.is_empty() {
        return String::new();
    }
    if text.width() <= max {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for character in text.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        // Leave a column for the ellipsis.
        if used + width + 1 > max {
            break;
        }
        out.push(character);
        used += width;
    }
    out.push('…');
    out
}
fn compact(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
fn relative_time(time: chrono::DateTime<Utc>) -> String {
    let d = Utc::now().signed_duration_since(time);
    if d.num_seconds() < 60 {
        "now".into()
    } else if d.num_minutes() < 60 {
        format!("{}m", d.num_minutes())
    } else if d.num_hours() < 24 {
        format!("{}h", d.num_hours())
    } else {
        format!("{}d", d.num_days())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoApi;
    use ratatui::backend::TestBackend;
    use std::sync::Arc;

    #[test]
    fn numbers_are_compact() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1200), "1.2K");
        assert_eq!(compact(2_500_000), "2.5M");
    }
    #[test]
    fn center_stays_inside() {
        let r = centered(Rect::new(0, 0, 20, 10), 100, 100);
        assert_eq!(r, Rect::new(0, 0, 20, 10));
    }

    #[tokio::test]
    async fn layouts_render_at_compact_medium_and_wide_widths() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        for width in [60, 100, 150] {
            let backend = TestBackend::new(width, 32);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            let buffer = terminal.backend().buffer();
            let rendered = (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                rendered.contains("Home"),
                "missing title at {width} columns"
            );
            assert!(
                rendered.contains("Following"),
                "missing feed tab at {width} columns"
            );
            assert!(
                rendered.contains("Ada"),
                "missing post content at {width} columns"
            );
            if width >= 94 {
                assert!(
                    rendered.contains("KEYBOARD") && rendered.contains("Previous / next"),
                    "missing persistent shortcut dock at {width} columns"
                );
            } else {
                assert!(
                    rendered.contains("? keys"),
                    "missing compact shortcut footer at {width} columns"
                );
            }
        }
    }

    #[tokio::test]
    async fn selected_post_is_pinned_and_next_post_is_previewed() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        app.selected = 2;
        let backend = TestBackend::new(150, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rows = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let focused_row = rows
            .iter()
            .position(|row| row.contains("NOW READING"))
            .expect("focused marker should be rendered");
        let next_row = rows
            .iter()
            .position(|row| row.contains("Sam Rivera"))
            .expect("next post should be visible as a preview");
        assert!(
            focused_row <= 5,
            "selected card was not pinned: row {focused_row}"
        );
        assert!(next_row > focused_row);
        assert!(rows.iter().any(|row| row.contains("KEYBOARD")));
        assert!(rows.iter().any(|row| row.contains("Previous / next")));
    }

    #[tokio::test]
    async fn arrow_keys_open_and_restore_the_exact_feed_position() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Down)).await;
        assert_eq!(app.selected, 1);
        let selected_id = app.selected_post().unwrap().id.clone();
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Right)).await;
        assert!(matches!(app.screen, Screen::Thread(_)));
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Left)).await;
        assert!(matches!(app.screen, Screen::Home));
        assert_eq!(app.selected_post().unwrap().id, selected_id);
        assert!(!app.should_quit);
    }

    #[tokio::test]
    async fn collapsing_a_thread_renders_only_the_root_post() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        app.activate().await;
        assert!(app.posts.len() > 1);
        app.thread_expanded = false;
        let backend = TestBackend::new(150, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("NOW READING"));
        assert!(
            !rendered.contains("Mina") && !rendered.contains("Drew"),
            "collapsed thread leaked reply authors"
        );
    }

    #[tokio::test]
    async fn empty_search_shows_a_no_results_message() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        app.begin_search();
        for character in "zzzz-not-in-demo".chars() {
            app.insert_query_char(character);
        }
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Enter)).await;
        assert_eq!(app.mode, InputMode::Normal);
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("No results"),
            "empty search should explain itself"
        );
    }

    #[test]
    fn status_text_is_truncated_with_an_ellipsis() {
        assert_eq!(truncate_to_width("hello world", 5), "hell…");
        assert_eq!(truncate_to_width("hello", 100), "hello");
        assert_eq!(truncate_to_width("", 5), "");
        assert_eq!(truncate_to_width("abcdef", 0), "");
    }

    #[tokio::test]
    async fn busy_draw_flags_work_is_in_progress() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        busy_draw(&mut terminal, &mut app, "Loading more posts…");
        assert_eq!(app.status, "Loading more posts…");
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("Loading more posts…"),
            "busy status must be painted before a blocking fetch"
        );
    }

    async fn handle_key_with_terminal(app: &mut App, key: KeyEvent) {
        let backend = TestBackend::new(150, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        handle_key(&mut terminal, app, key).await;
    }

    #[tokio::test]
    async fn tab_enters_sidebar_and_arrows_drive_it() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Tab)).await;
        assert!(app.nav_focused);
        assert_eq!(app.nav_selected, 0);
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Down)).await;
        assert_eq!(app.nav_selected, 1);
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Right)).await;
        assert!(matches!(app.screen, Screen::Explore));
        assert!(!app.nav_focused);
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Tab)).await;
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Esc)).await;
        assert!(!app.nav_focused);
    }

    #[tokio::test]
    async fn cards_paint_focused_post_with_accent_frame() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        let backend = TestBackend::new(150, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let row = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(row.contains("▶ NOW READING"));
        assert!(row.contains("━"), "focused card frame should render");
        assert!(row.contains("Home"));
    }
}
