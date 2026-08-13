use crate::{
    app::{App, HitAction, HitRegion, InputMode, Screen},
    keys::Action,
    model::{FeedKind, MediaKind, Post},
};
use anyhow::Result;
use chrono::Utc;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};
use std::{
    io::{self, stdout},
    sync::OnceLock,
    time::{Duration, Instant},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

mod landing;
pub mod screenshot;

/// XTUI's visual language is intentionally achromatic: a true-black canvas,
/// white type, and a small grayscale ramp for hierarchy. Semantic state is
/// communicated through copy, glyphs, weight, and inversion rather than hue.
#[derive(Clone, Debug)]
pub struct Theme {
    pub white: Color,
    pub gray: Color,
    pub dim: Color,
    pub surface: Color,
    pub surface_raised: Color,
    pub accent: Color,
    pub green: Color,
    pub amber: Color,
    pub red: Color,
    pub background: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            white: Color::Rgb(245, 245, 245),
            gray: Color::Rgb(156, 156, 156),
            dim: Color::Rgb(66, 66, 66),
            surface: Color::Rgb(0, 0, 0),
            surface_raised: Color::Rgb(18, 18, 18),
            accent: Color::Rgb(255, 255, 255),
            green: Color::Rgb(220, 220, 220),
            amber: Color::Rgb(190, 190, 190),
            red: Color::Rgb(255, 255, 255),
            background: Color::Rgb(0, 0, 0),
        }
    }
}

impl Theme {
    pub fn from_config(config: &Option<crate::config::ThemeConfig>) -> Self {
        let mut theme = Theme::default();
        let Some(config) = config else {
            return theme;
        };
        let apply = |target: &mut Color, value: &Option<String>| {
            if let Some(hex) = value.as_deref().and_then(parse_hex) {
                *target = monochrome(hex);
            }
        };
        apply(&mut theme.white, &config.white);
        apply(&mut theme.gray, &config.gray);
        apply(&mut theme.dim, &config.dim);
        apply(&mut theme.surface, &config.surface);
        apply(&mut theme.surface_raised, &config.surface_raised);
        apply(&mut theme.accent, &config.accent);
        apply(&mut theme.green, &config.green);
        apply(&mut theme.amber, &config.amber);
        apply(&mut theme.red, &config.red);
        apply(&mut theme.background, &config.background);
        theme
    }
}

static THEME: OnceLock<Theme> = OnceLock::new();

/// Install the theme from the config file; called once at startup.
pub fn init_theme(theme: Theme) {
    let _ = THEME.set(theme);
}

fn theme() -> &'static Theme {
    THEME.get_or_init(Theme::default)
}

fn parse_hex(input: &str) -> Option<Color> {
    let hex = input.strip_prefix('#').unwrap_or(input);
    if hex.len() != 6 {
        return None;
    }
    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(red, green, blue))
}

fn monochrome(color: Color) -> Color {
    let Color::Rgb(red, green, blue) = color else {
        return color;
    };
    // Integer Rec. 709 luma. Theme overrides remain useful as intensity
    // controls without ever introducing hue into the interface.
    let luma = ((u16::from(red) * 54 + u16::from(green) * 183 + u16::from(blue) * 19) / 256) as u8;
    Color::Rgb(luma, luma, luma)
}

pub async fn run(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    // Arm cleanup immediately. Any later setup failure must still restore the
    // console's cooked input mode before control returns to the shell.
    let restore = TerminalRestore;
    let mut out = stdout();
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        Hide
    )?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    // Paint the initial frame before bootstrap: a browser-extension connect
    // can take several seconds and must never leave a blank alternate screen.
    terminal.draw(|frame| draw(frame, app))?;
    let result = event_loop(&mut terminal, app);
    drop(restore);
    result
}

struct TerminalRestore;
impl Drop for TerminalRestore {
    fn drop(&mut self) {
        // Reverse setup order. Crossterm's Windows mouse capture remembers
        // the console mode that exists when it is enabled (already raw here),
        // so disabling raw mode before mouse capture would restore raw mode a
        // second time and leak escape sequences into the command prompt.
        let _ = execute!(
            stdout(),
            Show,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen,
        );
        let _ = disable_raw_mode();
    }
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    // Bootstrap runs in the background: the spinner animates while the
    // browser companion connects, and the interface stays fully responsive.
    app.request_bootstrap();
    let mut last_idle_redraw = Instant::now();
    let mut last_auto_refresh = Instant::now();
    let mut last_landing_motion_frame = landing::motion_frame();
    while !app.should_quit {
        // While work is in flight the poll window shrinks to a tick so the
        // spinner animates; otherwise XTUI sleeps until a key, mouse, resize
        // or the 30 s relative-time refresh.
        let busy = app.has_pending();
        let landing_motion = matches!(app.screen, Screen::Landing);
        let timeout = if busy || landing_motion || app.browser_mode {
            Duration::from_millis(125)
        } else {
            Duration::from_secs(30)
        };
        let event = event::poll(timeout)?;
        let mut input_changed = false;
        if event {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(app, key);
                    input_changed = true;
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        app.advance(1);
                        input_changed = true;
                    }
                    MouseEventKind::ScrollUp => {
                        app.advance(-1);
                        input_changed = true;
                    }
                    MouseEventKind::Moved => {
                        input_changed = app.hover_at(mouse.column, mouse.row);
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        handle_click(app, mouse.row, mouse.column);
                        input_changed = true;
                    }
                    _ => {}
                },
                Event::Paste(text) if app.mode == InputMode::Search => {
                    app.insert_query_text(&text);
                    input_changed = true;
                }
                Event::Resize(_, _) => input_changed = true,
                _ => {}
            }
        }
        let changed = app.process_messages();
        let prefetched = app.maintain_read_ahead();
        let landing_motion_frame = landing::motion_frame();
        let landing_motion_changed =
            landing_motion && landing_motion_frame != last_landing_motion_frame;
        // Silent background refresh keeps Home live without stealing focus.
        if app.auto_refresh_secs > 0
            && !app.demo
            && !app.browser_mode
            && app.mode == InputMode::Normal
            && !app.nav_focused
            && matches!(app.screen, Screen::Home)
            && !app.has_pending()
            && last_auto_refresh.elapsed() >= Duration::from_secs(app.auto_refresh_secs)
        {
            app.request_screen(Screen::Home, true);
            last_auto_refresh = Instant::now();
        }
        if busy
            || landing_motion_changed
            || changed
            || prefetched
            || input_changed
            || last_idle_redraw.elapsed() >= Duration::from_secs(30)
        {
            if !app.should_quit {
                terminal.draw(|frame| draw(frame, app))?;
            }
            last_landing_motion_frame = landing_motion_frame;
            if !busy {
                last_idle_redraw = Instant::now();
            }
        }
    }
    Ok(())
}

fn handle_click(app: &mut App, row: u16, column: u16) {
    if app.help || app.media_preview.is_some() {
        return;
    }
    match app.hit_at(column, row) {
        Some(HitAction::Landing(index)) => {
            app.landing_selected = index;
            app.activate_landing();
        }
        Some(HitAction::Nav(index)) => {
            app.nav_selected = index;
            app.nav_focused = false;
            app.root(App::nav_screen(index));
        }
        Some(HitAction::Card(index)) => {
            app.selected = index;
            app.activate();
        }
        Some(HitAction::Back) => app.back(),
        Some(HitAction::Search) => app.begin_search(),
        Some(HitAction::ToggleFeed) => app.toggle_feed(),
        Some(HitAction::Help) => app.help = true,
        Some(HitAction::Quit) => app.should_quit = true,
        None => {}
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
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
        // The modal hints promise V/O; honour them instead of swallowing the
        // keys behind the overlay.
        if app.media_preview.is_some() {
            match key.code {
                KeyCode::Char('v') => app.open_external(true),
                KeyCode::Char('o') => app.open_external(false),
                _ => {}
            }
        }
        return;
    }
    if matches!(app.screen, Screen::Landing) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let len = app.landing_items().len();
                app.landing_selected = (app.landing_selected + len - 1) % len;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let len = app.landing_items().len();
                app.landing_selected = (app.landing_selected + 1) % len;
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') => app.activate_landing(),
            KeyCode::Char('r') => app.request_sign_in_check(),
            KeyCode::Esc | KeyCode::Left => {
                app.status = "Use ↑/↓ to choose and Enter to select — Q quits".into();
            }
            KeyCode::Char('q') => app.should_quit = true,
            _ => {}
        }
        return;
    }
    if app.mode == InputMode::Search {
        match key.code {
            KeyCode::Enter => app.submit_search(),
            KeyCode::Esc => {
                app.mode = InputMode::Normal;
                if app.history.is_empty() {
                    app.root(Screen::Home);
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
            KeyCode::Right | KeyCode::Enter => app.nav_activate(),
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
                app.root(Screen::Home)
            }
            KeyCode::Char('2') => {
                app.nav_focused = false;
                app.root(Screen::Explore)
            }
            KeyCode::Char('3') => {
                app.nav_focused = false;
                app.root(Screen::Mentions)
            }
            KeyCode::Char('4') => {
                app.nav_focused = false;
                app.root(Screen::Bookmarks)
            }
            KeyCode::Char('5') => {
                app.nav_focused = false;
                app.root(Screen::Lists)
            }
            _ => {}
        }
        return;
    }
    if let Some(action) = app.keys.action_for(&key) {
        match action {
            Action::Quit => app.should_quit = true,
            Action::MoveUp => app.advance(-1),
            Action::MoveDown => app.advance(1),
            Action::PageUp => app.advance(-5),
            Action::PageDown => app.advance(5),
            Action::Top => app.move_selection(-(app.posts.len() as isize).saturating_mul(2)),
            Action::Bottom => app.move_selection(app.posts.len() as isize * 2),
            Action::Open => app.activate(),
            Action::Back => app.back(),
            Action::Search => app.begin_search(),
            Action::Refresh => app.refresh(),
            Action::Profile => app.open_profile(),
            Action::Likes => app.open_likes(),
            Action::MediaPreview => app.request_media_preview(),
            Action::OpenMedia => app.open_external(true),
            Action::OpenPost => app.open_external(false),
            Action::Help => app.help = true,
            Action::Sidebar => app.toggle_nav_focus(),
            Action::Home => app.root(Screen::Home),
            Action::Explore => app.root(Screen::Explore),
            Action::Mentions => app.root(Screen::Mentions),
            Action::Bookmarks => app.root(Screen::Bookmarks),
            Action::Lists => app.root(Screen::Lists),
            Action::ToggleFeed => app.toggle_feed(),
            Action::ExpandThread => {
                if matches!(app.screen, Screen::Thread(_)) {
                    app.thread_expanded = !app.thread_expanded;
                    app.selected = 0;
                    app.status = if app.thread_expanded {
                        "Replies expanded · Space collapses".into()
                    } else {
                        "Replies collapsed · Space expands".into()
                    };
                }
            }
        }
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.hit_regions.clear();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme().background).fg(theme().white)),
        area,
    );
    if matches!(app.screen, Screen::Landing) {
        landing::render(frame, area, app);
        if app.help {
            render_help(frame, area);
        }
        return;
    }
    if area.width >= 104 {
        let cols = Layout::horizontal([
            Constraint::Length(20),
            Constraint::Length(1),
            Constraint::Min(50),
            Constraint::Length(1),
            Constraint::Length(30),
        ])
        .split(area);
        render_nav(frame, cols[0], app, false);
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

fn render_nav(frame: &mut Frame, area: Rect, app: &mut App, compact: bool) {
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
        let is_hovered = app.hovered == Some(HitAction::Nav(index));
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
            Style::default()
                .fg(theme().accent)
                .add_modifier(Modifier::BOLD)
        } else if is_hovered {
            Style::default()
                .fg(theme().white)
                .bg(theme().surface_raised)
                .add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme().gray)
        };
        lines.push(Line::from(Span::styled(text, style)));
        lines.push(Line::from(""));
        app.register_hit(
            area.x,
            area.y.saturating_add(2 + index as u16 * 2),
            area.width,
            1,
            HitAction::Nav(index),
        );
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme().background)),
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
                    Span::styled("●  ", Style::default().fg(theme().green)),
                    Span::styled(
                        name,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    format!("   @{handle}"),
                    Style::default().fg(theme().gray),
                )),
            ])
            .style(Style::default().bg(theme().background)),
            a,
        );
    }
}

fn render_bottom_nav(frame: &mut Frame, area: Rect, app: &mut App) {
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
                            Style::default()
                                .fg(theme().accent)
                                .add_modifier(Modifier::BOLD)
                        } else if app.hovered == Some(HitAction::Nav(i)) {
                            Style::default()
                                .fg(theme().white)
                                .bg(theme().surface_raised)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme().gray)
                        },
                    ),
                    Span::raw(" "),
                ]
            })
            .collect::<Vec<_>>(),
    );
    let keys = Line::from(vec![
        Span::styled(
            "↑↓/jk",
            Style::default()
                .fg(theme().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" move  ", Style::default().fg(theme().gray)),
        Span::styled(
            "→",
            Style::default()
                .fg(theme().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" open  ", Style::default().fg(theme().gray)),
        Span::styled(
            "←",
            Style::default()
                .fg(theme().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " back  / search  ? keys  Q quit",
            Style::default().fg(theme().gray),
        ),
    ])
    .alignment(Alignment::Right);
    frame.render_widget(
        Paragraph::new(vec![line, keys])
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(theme().dim)),
            ),
        area,
    );
    let segment = area.width / labels.len() as u16;
    for index in 0..labels.len() {
        let x = area.x.saturating_add(segment.saturating_mul(index as u16));
        let width = if index + 1 == labels.len() {
            area.right().saturating_sub(x)
        } else {
            segment
        };
        app.register_hit(x, area.y, width, 1, HitAction::Nav(index));
    }
    let key_row = area.bottom().saturating_sub(1);
    app.register_hit(
        area.right().saturating_sub(7),
        key_row,
        7,
        1,
        HitAction::Quit,
    );
    app.register_hit(
        area.right().saturating_sub(15),
        key_row,
        8,
        1,
        HitAction::Help,
    );
    app.register_hit(
        area.right().saturating_sub(25),
        key_row,
        10,
        1,
        HitAction::Search,
    );
}

fn render_center(frame: &mut Frame, area: Rect, app: &mut App) {
    let panel = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().dim))
        .style(Style::default().bg(theme().surface));
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
        status.push_str("  ·  run `xtui browser` or `xtui login CLIENT_ID` for live X");
    }
    let status_style = if app.error.is_some() {
        Style::default()
            .fg(theme().red)
            .add_modifier(Modifier::BOLD)
    } else if app.demo {
        Style::default().fg(theme().amber)
    } else {
        Style::default().fg(theme().green)
    };
    // Hints are contextual: while typing a search, telling the user about
    // feed navigation would be noise.
    let hints = if app.mode == InputMode::Search {
        "   Enter search · Esc cancel · Ctrl+U clear "
    } else {
        "   ↑↓/jk move  → open  ← back  / search  ? help "
    };
    let hints_width = hints.width();
    let width = chunks[2].width as usize;
    let available = width.saturating_sub(3 + hints_width);
    let mut spans = vec![Span::styled(" ● ", status_style)];
    if available >= 24 {
        spans.push(Span::styled(
            truncate_to_width(&status, available),
            Style::default().fg(theme().gray),
        ));
        spans.push(Span::styled(hints, Style::default().fg(theme().dim)));
    } else {
        spans.push(Span::styled(
            truncate_to_width(&status, width.saturating_sub(3)),
            Style::default().fg(theme().gray),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Left)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(theme().dim)),
            ),
        chunks[2],
    );
    if app.mode == InputMode::Normal && width >= 24 + hints_width {
        let key_row = chunks[2].bottom().saturating_sub(1);
        app.register_hit(
            chunks[2].right().saturating_sub(8),
            key_row,
            8,
            1,
            HitAction::Help,
        );
        app.register_hit(
            chunks[2].right().saturating_sub(18),
            key_row,
            10,
            1,
            HitAction::Search,
        );
    }
}

fn header_height(app: &App) -> u16 {
    match app.screen {
        Screen::Profile(_) => 7,
        Screen::ListFeed(_) => 5,
        _ => 3,
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &mut App) {
    // Title on the left; position and spinner ride the same line on the right
    // so both stay visible in every layout, even without a context rail.
    let mut title = Line::from(vec![
        Span::styled(
            if app.history.is_empty() { "  " } else { "← " },
            if app.hovered == Some(HitAction::Back) {
                Style::default()
                    .fg(theme().white)
                    .bg(theme().surface_raised)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme().accent)
            },
        ),
        Span::styled(
            app.title(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let mut right = String::new();
    if let Some(spinner) = app.spinner_frame() {
        right.push(spinner);
        right.push(' ');
    }
    if !app.posts.is_empty() && !matches!(app.screen, Screen::Lists) {
        right.push_str(&format!("{} / {}", app.selected + 1, app.posts.len()));
    }
    if !right.is_empty() {
        let used = title.width() + right.width() + 1;
        let padding = (area.width as usize).saturating_sub(used);
        title.push_span(Span::raw(" ".repeat(padding)));
        title.push_span(Span::styled(
            right,
            Style::default()
                .fg(if app.has_pending() {
                    theme().accent
                } else {
                    theme().gray
                })
                .add_modifier(Modifier::BOLD),
        ));
    }
    let mut lines = vec![title];
    let mut wrap = false;
    match &app.screen {
        Screen::Home => {
            let label = match app.feed_kind {
                FeedKind::Following => "Following",
                FeedKind::ForYou => "For You",
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  ━━━ {label} ━━━"),
                    if app.hovered == Some(HitAction::ToggleFeed) {
                        Style::default()
                            .fg(theme().background)
                            .bg(theme().white)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(theme().accent)
                            .add_modifier(Modifier::BOLD)
                    },
                ),
                Span::styled(
                    if app.browser_mode { "   f toggles" } else { "" },
                    Style::default().fg(theme().dim),
                ),
            ]));
        }
        Screen::Explore if app.mode == InputMode::Search => lines.push(Line::from(vec![
            Span::styled(
                "  Search X  ",
                if app.hovered == Some(HitAction::Search) {
                    Style::default()
                        .fg(theme().background)
                        .bg(theme().white)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme().accent)
                },
            ),
            Span::styled(
                app.search_display(),
                Style::default().fg(Color::White).bg(theme().surface_raised),
            ),
        ])),
        Screen::Explore => lines.push(Line::from(vec![
            Span::styled("  Search: ", Style::default().fg(theme().accent)),
            Span::styled(app.query.clone(), Style::default().fg(Color::White)),
        ])),
        Screen::Profile(u) => {
            wrap = true;
            lines.push(Line::from(Span::styled(
                format!("  @{}{}", u.username, if u.verified { "  ✓" } else { "" }),
                Style::default().fg(theme().gray),
            )));
            lines.push(Line::from(Span::styled(
                format!("  {}", u.description),
                Style::default().fg(theme().gray),
            )));
            if let Some(m) = &u.public_metrics {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {} ", compact(m.following_count)),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("Following   ", Style::default().fg(theme().gray)),
                    Span::styled(
                        format!("{} ", compact(m.followers_count)),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "Followers   L liked posts",
                        Style::default().fg(theme().gray),
                    ),
                ]));
            }
        }
        Screen::ListFeed(l) => {
            lines.push(Line::from(Span::styled(
                format!("  {}", l.description),
                Style::default().fg(theme().gray),
            )));
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} members  ·  {} followers",
                    l.member_count.unwrap_or(0),
                    l.follower_count.unwrap_or(0)
                ),
                Style::default().fg(theme().gray),
            )));
        }
        Screen::Thread(_) => {
            let key = app.keys.key_label(Action::ExpandThread);
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} posts · {key} {}",
                    app.posts.len(),
                    if app.thread_expanded {
                        "collapses replies"
                    } else {
                        "expands replies"
                    }
                ),
                Style::default().fg(theme().gray),
            )));
        }
        Screen::Likes(user) => {
            lines.push(Line::from(Span::styled(
                format!("  posts liked by @{}", user.username),
                Style::default().fg(theme().gray),
            )));
        }
        _ => {}
    }
    if !app.history.is_empty() {
        app.register_hit(area.x, area.y, 2, 1, HitAction::Back);
    }
    if matches!(app.screen, Screen::Home) && app.browser_mode {
        app.register_hit(
            area.x,
            area.y.saturating_add(1),
            area.width.min(28),
            1,
            HitAction::ToggleFeed,
        );
    }
    if matches!(app.screen, Screen::Explore) {
        app.register_hit(
            area.x,
            area.y.saturating_add(1),
            area.width,
            1,
            HitAction::Search,
        );
    }
    let mut paragraph = Paragraph::new(lines).style(Style::default().fg(theme().white));
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(
        paragraph.block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme().dim)),
        ),
        area,
    );
}

fn render_posts(frame: &mut Frame, area: Rect, app: &mut App) {
    app.card_rows.clear();
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
        } else if matches!(app.screen, Screen::Thread(_)) && app.has_pending() {
            "  Loading this conversation and its repliesâ€¦".into()
        } else if matches!(app.screen, Screen::Thread(_)) {
            "  This conversation has no posts.".into()
        } else {
            "  Nothing here yet.\n  Press r to refresh or / to search.".into()
        };
        frame.render_widget(
            Paragraph::new(message).style(Style::default().fg(theme().gray)),
            area,
        );
        return;
    }
    // Cards are stacked manually. Only the focused card wears a full frame;
    // the rest separate with a thin rule so the feed reads as a single
    // scrolling column instead of a wall of boxes. The feed window begins at
    // the selected post, so reading position stays pinned while rendering
    // stays proportional to the viewport.
    let width = area.width.saturating_sub(4) as usize;
    let thread_view = matches!(app.screen, Screen::Thread(_));
    if thread_view {
        let body_limit = area.height.saturating_sub(8).max(3) as usize;
        let total = app
            .selected_post()
            .map(|post| {
                let target = post.reposted.as_deref().unwrap_or(post);
                textwrap::wrap(&target.text, width.max(10)).len()
            })
            .unwrap_or(0);
        app.body_scroll_max = total.saturating_sub(body_limit);
        app.body_scroll = app.body_scroll.min(app.body_scroll_max);
    } else {
        app.body_scroll = 0;
        app.body_scroll_max = 0;
    }
    let mut y = area.y;
    for (index, post) in app
        .posts
        .iter()
        .enumerate()
        .skip(selected)
        .take(if collapsed { 1 } else { 14 })
    {
        let focused = index == selected;
        let hovered = app.hovered == Some(HitAction::Card(index));
        let reply = matches!(app.screen, Screen::Thread(_)) && index > 0;
        let body_limit = if focused && thread_view {
            area.height.saturating_sub(8).max(3) as usize
        } else if focused {
            9
        } else {
            3
        };
        let body_offset = if focused && thread_view {
            app.body_scroll
        } else {
            0
        };
        let lines = post_lines(post, width, reply, focused, body_limit, body_offset);
        let remaining = area.bottom().saturating_sub(y);
        // Skip cards that cannot fit at least one content row; a sliver looks
        // like a rendering bug.
        if remaining < 2 {
            break;
        }
        let height = (lines.len() as u16 + if focused { 2 } else { 1 }).min(remaining);
        let block = if focused {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme().accent))
                .style(Style::default().bg(theme().surface_raised))
        } else if hovered {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme().gray))
                .style(Style::default().bg(theme().surface_raised))
        } else {
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme().dim))
                .style(Style::default().bg(theme().background))
        };
        frame.render_widget(
            Paragraph::new(lines).block(block),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height,
            },
        );
        app.card_rows.push((y, y + height, index));
        app.hit_regions.push(HitRegion {
            x: area.x,
            y,
            width: area.width,
            height,
            action: HitAction::Card(index),
        });
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
    body_offset: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if post.reposted.is_some() {
        lines.push(Line::from(Span::styled(
            "  ↻  Reposted",
            Style::default().fg(theme().gray),
        )));
    }
    let prefix = if reply { "  └─ " } else { "  " };
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
                Style::default()
                    .fg(theme().accent)
                    .add_modifier(Modifier::BOLD),
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
            Style::default().fg(theme().gray),
        ),
    ]));
    let target = post.reposted.as_deref().unwrap_or(post);
    let wrapped = textwrap::wrap(&target.text, width.max(10));
    let clipped_above = body_offset > 0;
    let clipped_below = wrapped.len() > body_offset.saturating_add(body_limit);
    if clipped_above {
        lines.push(Line::from(Span::styled(
            "  ↑  earlier text",
            Style::default().fg(theme().gray),
        )));
    }
    for line in wrapped.into_iter().skip(body_offset).take(body_limit) {
        lines.push(Line::from(format!("  {line}")));
    }
    if clipped_below {
        lines.push(Line::from(Span::styled(
            if body_offset > 0 {
                "  …  ↓ continue reading"
            } else {
                "  …  → open to read the rest"
            },
            Style::default().fg(theme().gray),
        )));
    }
    if focused && let Some(q) = &post.quoted {
        lines.push(Line::from(vec![
            Span::styled("  ┌ ", Style::default().fg(theme().dim)),
            Span::styled(
                q.author.name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  @{}", q.author.username),
                Style::default().fg(theme().accent),
            ),
        ]));
        for wrapped in textwrap::wrap(&q.text, width.saturating_sub(4).max(10))
            .into_iter()
            .take(2)
        {
            lines.push(Line::from(Span::styled(
                format!("  │ {wrapped}"),
                Style::default().fg(theme().white),
            )));
        }
        lines.push(Line::from(Span::styled(
            "  └",
            Style::default().fg(theme().dim),
        )));
    }
    if let Some(m) = target.media.first() {
        let badge = match m.kind {
            MediaKind::Photo => "▧ PHOTO",
            MediaKind::Video => "▶ VIDEO",
            MediaKind::AnimatedGif => "▶ GIF",
        };
        lines.push(Line::from(vec![
            Span::styled("  ▣  ", Style::default().fg(theme().gray)),
            Span::styled(
                badge,
                Style::default()
                    .fg(theme().accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  {}",
                    m.alt_text.as_deref().unwrap_or("M preview · V open")
                ),
                Style::default().fg(theme().gray),
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
        Style::default().fg(theme().gray),
    )));
    lines
}

fn render_lists(frame: &mut Frame, area: Rect, app: &mut App) {
    app.card_rows.clear();
    if app.lists.is_empty() {
        frame.render_widget(
            Paragraph::new("  No lists yet.\n  Press r to refresh.")
                .style(Style::default().fg(theme().gray)),
            area,
        );
        return;
    }
    let mut y = area.y;
    for (offset, list) in app.lists.iter().skip(app.selected).take(12).enumerate() {
        let index = offset + app.selected;
        let focused = offset == 0;
        let hovered = app.hovered == Some(HitAction::Card(index));
        let lines = vec![
            Line::from(vec![
                Span::styled(
                    "  ",
                    Style::default().fg(if focused {
                        theme().accent
                    } else {
                        theme().gray
                    }),
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
                Style::default().fg(theme().gray),
            )),
            Line::from(Span::styled(
                format!(
                    "  {} members · {} followers",
                    list.member_count.unwrap_or(0),
                    list.follower_count.unwrap_or(0)
                ),
                Style::default().fg(theme().gray),
            )),
        ];
        let remaining = area.bottom().saturating_sub(y);
        if remaining < 2 {
            break;
        }
        let height = (lines.len() as u16 + if focused { 2 } else { 1 }).min(remaining);
        let block = if focused {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme().accent))
                .style(Style::default().bg(theme().surface_raised))
        } else if hovered {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme().gray))
                .style(Style::default().bg(theme().surface_raised))
        } else {
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme().dim))
                .style(Style::default().bg(theme().background))
        };
        frame.render_widget(
            Paragraph::new(lines).block(block),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height,
            },
        );
        app.card_rows.push((y, y + height, index));
        app.hit_regions.push(HitRegion {
            x: area.x,
            y,
            width: area.width,
            height,
            action: HitAction::Card(index),
        });
        y += height;
        if y >= area.bottom() {
            break;
        }
    }
}

fn render_context(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::vertical([Constraint::Min(8), Constraint::Length(1)]).split(area);
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
            Style::default()
                .fg(theme().white)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            selected
                .map(|post| format!("@{}", post.author.username))
                .unwrap_or_default(),
            Style::default().fg(theme().gray),
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
            Span::styled(" replies   ", Style::default().fg(theme().gray)),
            Span::styled(
                selected
                    .map(|p| compact(p.metrics.like_count))
                    .unwrap_or_default(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" likes", Style::default().fg(theme().gray)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "The selected post stays pinned at the top of the feed.",
            Style::default().fg(theme().gray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(summary).wrap(Wrap { trim: false }).block(
            Block::default()
                .title("  ▍ CURRENT  ")
                .title_style(
                    Style::default()
                        .fg(theme().accent)
                        .add_modifier(Modifier::BOLD),
                )
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme().dim))
                .padding(Padding::uniform(1))
                .style(Style::default().bg(theme().surface)),
        ),
        rows[0],
    );
    // A quiet hint that the full help lives behind `?` — no persistent dock.
    let footer = Line::from(vec![
        Span::styled(" ? ", Style::default().fg(theme().accent)),
        Span::styled("full help   ", Style::default().fg(theme().gray)),
        Span::styled("Q ", Style::default().fg(theme().accent)),
        Span::styled("quit", Style::default().fg(theme().gray)),
    ])
    .alignment(Alignment::Right);
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().bg(theme().background)),
        rows[1],
    );
}

fn render_help(frame: &mut Frame, area: Rect) {
    // The popup must be wide enough that the two-column layout never wraps.
    let popup = centered(area, 72, 21);
    frame.render_widget(Clear, popup);
    let text = "j / ↓        Move down                k / ↑     Move up\ng / G        Top / bottom               → / Enter Open post or selection\nPgUp/PgDn    Jump five posts            ← / Esc    Back / leave\nTab          Sidebar focus              /         Search X\n1…5          Switch section             ?         This help\nP / L        Profile / likes            M / V     Media preview\nR            Refresh                    O         Open on x.com\nF            Following / For You        Space     Collapse thread replies\nQ            Quit\n\nMouse wheel and click also move the selection. The active post stays\npinned to the top; previews underneath show what comes next.\n\nQ / Esc / ?   Close this help";
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(theme().white))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title("  KEYBOARD SHORTCUTS  ")
                    .title_style(
                        Style::default()
                            .fg(theme().accent)
                            .add_modifier(Modifier::BOLD),
                    )
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme().dim))
                    .style(Style::default().bg(theme().surface_raised))
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
        Style::default().fg(theme().gray),
    )));
    text.push(Line::from(Span::styled(
        "V open in browser · O open post · Esc close",
        Style::default().fg(theme().dim),
    )));
    frame.render_widget(
        Paragraph::new(text).alignment(Alignment::Center).block(
            Block::default()
                .title("  ▍ MEDIA  · Esc to close  ")
                .title_style(
                    Style::default()
                        .fg(theme().accent)
                        .add_modifier(Modifier::BOLD),
                )
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme().dim))
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

    fn demo_app() -> App {
        App::new(Arc::new(DemoApi::new()), true)
    }

    async fn booted_app() -> App {
        let mut app = demo_app();
        app.bootstrap_for_test().await;
        app
    }

    fn rendered(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    }

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

    #[test]
    fn theme_parses_hex_and_defaults() {
        let t = Theme::from_config(&Some(crate::config::ThemeConfig {
            accent: Some("#112233".into()),
            ..Default::default()
        }));
        assert_eq!(t.accent, Color::Rgb(31, 31, 31));
        assert_eq!(t.gray, Theme::default().gray, "unset colors keep defaults");
        assert_eq!(parse_hex("nope"), None);
        assert_eq!(parse_hex("#FFF"), None);
    }

    #[tokio::test]
    async fn layouts_render_at_compact_medium_and_wide_widths() {
        let mut app = booted_app().await;
        for width in [60, 100, 150] {
            let rows = rendered(&mut app, width, 32);
            let all = rows.join("\n");
            assert!(all.contains("Home"), "missing title at {width} columns");
            assert!(
                all.contains("Following"),
                "missing feed tab at {width} columns"
            );
            assert!(
                all.contains("Ada"),
                "missing post content at {width} columns"
            );
            assert!(all.contains("1 / 6"), "missing position at {width} columns");
            if width >= 104 {
                assert!(all.contains("CURRENT"), "missing context rail at {width}");
            } else {
                assert!(!all.contains("CURRENT"), "rail hidden at {width}");
            }
            assert!(
                !all.contains("KEYBOARD"),
                "keyboard dock must be gone at {width} columns"
            );
            if width < 78 {
                assert!(all.contains("? keys"), "missing compact hints at {width}");
            }
        }
    }

    #[tokio::test]
    async fn selected_post_is_pinned_and_next_post_is_previewed() {
        let mut app = booted_app().await;
        app.selected = 2;
        let rows = rendered(&mut app, 150, 36);
        let focused_row = rows
            .iter()
            .position(|row| row.contains("Orbital"))
            .expect("focused post should be rendered");
        let next_row = rows
            .iter()
            .position(|row| row.contains("Sam Rivera"))
            .expect("next post should be visible as a preview");
        assert!(
            focused_row <= 6,
            "selected card was not pinned: row {focused_row}"
        );
        assert!(next_row > focused_row);
        assert!(
            rows.iter().any(|row| row.contains('━')),
            "focused card keeps its accent frame"
        );
    }

    #[tokio::test]
    async fn arrow_keys_open_and_restore_the_exact_feed_position() {
        let mut app = booted_app().await;
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
        let mut app = booted_app().await;
        app.activate();
        app.drain().await;
        assert!(app.posts.len() > 1);
        app.thread_expanded = false;
        let rows = rendered(&mut app, 150, 36);
        let all = rows.join("\n");
        assert!(all.contains("Ada"));
        assert!(
            !all.contains("Mina") && !all.contains("Drew"),
            "collapsed thread leaked reply authors"
        );
    }

    #[tokio::test]
    async fn long_thread_posts_scroll_all_the_way_to_the_tail() {
        let mut app = booted_app().await;
        let mut root = app.posts[0].clone();
        root.text = format!("{} tail-marker", "long-form body segment ".repeat(180));
        app.screen = Screen::Thread(Box::new(root.clone()));
        app.posts = vec![root];
        let first = rendered(&mut app, 100, 24).join("\n");
        assert!(app.body_scroll_max > 0, "long body exposes a scroll range");
        assert!(
            !first.contains("tail-marker"),
            "tail begins below the viewport"
        );
        app.body_scroll = app.body_scroll_max;
        let tail = rendered(&mut app, 100, 24).join("\n");
        assert!(tail.contains("tail-marker"), "the final text is reachable");
        assert!(
            tail.contains("earlier text"),
            "the UI signals scrolled content above"
        );
    }

    #[tokio::test]
    async fn empty_search_shows_a_no_results_message() {
        let mut app = booted_app().await;
        app.begin_search();
        for character in "zzzz-not-in-demo".chars() {
            app.insert_query_char(character);
        }
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Enter)).await;
        assert_eq!(app.mode, InputMode::Normal);
        let rows = rendered(&mut app, 100, 32);
        assert!(
            rows.join("\n").contains("No results"),
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
    async fn spinner_is_painted_while_work_is_in_flight() {
        let mut app = demo_app();
        app.request_bootstrap();
        assert!(app.has_pending());
        let rows = rendered(&mut app, 100, 32);
        assert!(
            rows.iter()
                .any(|row| { row.chars().any(|c| crate::app::SPINNER_FRAMES.contains(&c)) }),
            "the pending spinner glyph should be visible in the header"
        );
        app.drain().await;
        let rows = rendered(&mut app, 100, 32);
        assert!(
            !rows
                .iter()
                .any(|row| { row.chars().any(|c| crate::app::SPINNER_FRAMES.contains(&c)) }),
            "the spinner disappears once idle"
        );
    }

    #[tokio::test]
    async fn click_opens_the_post_under_the_cursor() {
        let mut app = booted_app().await;
        app.selected = 2;
        rendered(&mut app, 150, 36);
        assert!(app.card_rows.len() >= 3);
        assert_eq!(
            app.card_rows[0].2, 2,
            "the first card is the pinned selected post"
        );
        let (preview_top, preview_bottom, target) = app.card_rows[1];
        assert!(preview_bottom > preview_top);
        let preview = app
            .hit_regions
            .iter()
            .find(|region| region.action == HitAction::Card(target))
            .cloned()
            .expect("preview card has a hit target");
        let target_id = app.posts[target].id.clone();
        handle_click(&mut app, preview_top + 1, preview.x + 1);
        assert!(matches!(app.screen, Screen::Thread(_)));
        assert_eq!(
            app.selected_post().map(|post| post.id.as_str()),
            Some(target_id.as_str()),
            "the cached root is visible while replies load"
        );
        app.drain().await;
        assert_eq!(
            app.posts.first().map(|post| post.id.as_str()),
            Some(target_id.as_str()),
            "click opens the exact previewed post"
        );
    }

    #[test]
    fn landing_options_respond_to_hover_and_click() {
        let mut app = demo_app();
        rendered(&mut app, 110, 44);
        let quit = app
            .hit_regions
            .iter()
            .find(|region| region.action == HitAction::Landing(2))
            .cloned()
            .expect("quit option has a hit target");
        assert!(app.hover_at(quit.x + 1, quit.y));
        assert_eq!(app.hovered, Some(HitAction::Landing(2)));
        let hovered = rendered(&mut app, 110, 44).join("\n");
        assert!(
            hovered.contains("click"),
            "hover exposes a pointer affordance"
        );
        handle_click(&mut app, quit.y, quit.x + 1);
        assert!(app.should_quit, "click activates the hovered option");
    }

    #[tokio::test]
    async fn media_modal_keeps_its_open_shortcuts_alive() {
        let opened = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let recorder = opened.clone();
        let mut app = demo_app().with_external_opener(Arc::new(move |target| {
            recorder.lock().unwrap().push(target.to_owned());
            Ok(())
        }));
        app.bootstrap_for_test().await;
        app.request_media_preview();
        app.drain().await;
        assert!(
            app.media_preview.is_some(),
            "demo media preview should open"
        );
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('v')));
        let post = app.selected_post().expect("demo post selected");
        let media_url = crate::media::best_external_url(&post.media[0])
            .expect("demo photo has an external URL");
        let permalink = post.permalink();
        assert_eq!(opened.lock().unwrap().as_slice(), [media_url.as_str()]);
        assert!(
            app.media_preview.is_some(),
            "V while the modal is open must not dismiss it"
        );
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('o')));
        assert_eq!(*opened.lock().unwrap(), [media_url, permalink]);
        assert!(
            app.media_preview.is_some(),
            "O while the modal is open must not dismiss it"
        );
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(app.media_preview.is_none(), "Esc closes the modal");
    }

    #[tokio::test]
    async fn landing_renders_logo_status_and_menu() {
        let mut app = demo_app();
        let rows = rendered(&mut app, 110, 44);
        let all = rows.join("\n");
        assert!(
            all.contains("████") && all.lines().any(|line| line.matches('█').count() >= 20),
            "the large XTUI wordmark is present"
        );
        assert!(
            all.contains('░') && all.contains('▒'),
            "the wordmark keeps its layered extrusion"
        );
        assert!(all.contains("terminal interface"), "the title is present");
        assert!(
            all.contains("A focused, keyboard-first timeline."),
            "the sentence-case product line is present"
        );
        assert!(
            all.contains("Offline / demo"),
            "the status deck shows the current mode"
        );
        assert!(
            all.contains("Local / no account"),
            "account status is shown"
        );
        assert!(
            all.contains("Start reading") && all.contains("Demo / no account"),
            "the start option is present"
        );
        assert!(
            all.contains("Connect browser extension"),
            "the extension option is present when disconnected"
        );
        assert!(
            all.contains(env!("CARGO_PKG_VERSION")),
            "the version footer is present"
        );
        assert!(all.contains("Entry points"), "the command deck is labeled");
    }

    #[tokio::test]
    async fn landing_degrades_cleanly_on_small_terminals() {
        let mut app = demo_app();
        let compact = rendered(&mut app, 60, 24).join("\n");
        assert!(
            compact.contains("xtui / terminal interface"),
            "compact viewport keeps the identity"
        );
        assert!(
            compact.contains("03  Quit"),
            "compact menu remains complete"
        );

        let tiny = rendered(&mut app, 44, 18).join("\n");
        assert!(!tiny.contains("██╗"), "tiny view hides art before controls");
        assert!(tiny.contains("x t u i"), "tiny view keeps the identity");
        assert!(tiny.contains("03  Quit"), "tiny menu remains complete");
        assert!(
            tiny.contains("enter select"),
            "tiny controls remain discoverable"
        );
    }

    #[tokio::test]
    async fn landing_menu_reflects_login_state() {
        let mut app = demo_app();
        let names = |app: &mut App| {
            app.landing_items()
                .into_iter()
                .map(|(label, _)| label)
                .collect::<Vec<_>>()
        };
        let logged_out = names(&mut app);
        assert!(logged_out.iter().any(|l| l.contains("demo")));
        assert!(
            logged_out
                .iter()
                .any(|l| l.contains("Connect browser extension"))
        );
        app.browser_mode = true;
        app.demo = false;
        app.login_pending = true;
        let logged_in = names(&mut app);
        assert!(
            logged_in
                .iter()
                .any(|l| l.contains("live · browser extension"))
        );
        assert!(
            logged_in.iter().any(|l| l.contains("verify")),
            "a pending sign-in offers verification"
        );
        assert!(
            !logged_in
                .iter()
                .any(|l| l.contains("Connect browser extension")),
            "no sign-in option once in browser mode"
        );
    }

    #[tokio::test]
    async fn landing_keys_move_and_start() {
        let mut app = booted_app().await;
        app.screen = Screen::Landing;
        app.history.clear();
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Char('j'))).await;
        assert_eq!(app.landing_selected, 1, "j moves the menu cursor");
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Char('k'))).await;
        assert_eq!(app.landing_selected, 0, "k moves it back");
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Enter)).await;
        assert!(matches!(app.screen, Screen::Home), "Enter starts reading");
        assert!(!app.posts.is_empty(), "the Home feed loads");
    }

    #[tokio::test]
    async fn q_on_landing_quits() {
        let mut app = demo_app();
        handle_key(&mut app, KeyEvent::from(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    /// Dump the landing page to stdout for visual inspection.
    #[test]
    fn print_landing_preview() {
        let mut app = demo_app();
        let rows = rendered(&mut app, 110, 44);
        println!("\n{}", rows.join("\n"));
    }

    #[tokio::test]
    async fn status_bar_hints_follow_the_input_mode() {
        let mut app = booted_app().await;
        let normal = rendered(&mut app, 150, 32).join("\n");
        assert!(normal.contains("/ search"), "normal mode hints");
        app.begin_search();
        let searching = rendered(&mut app, 150, 32).join("\n");
        assert!(
            searching.contains("Esc cancel"),
            "search mode hints should not advertise feed keys"
        );
    }

    async fn handle_key_with_terminal(app: &mut App, key: KeyEvent) {
        handle_key(app, key);
        app.drain().await;
    }

    #[tokio::test]
    async fn tab_enters_sidebar_and_arrows_drive_it() {
        let mut app = booted_app().await;
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
        let mut app = booted_app().await;
        let rows = rendered(&mut app, 150, 36);
        let all = rows.join("\n");
        assert!(all.contains('━'), "focused card frame should render");
        assert!(!all.contains("NOW READING"), "the banner is gone");
        assert!(all.contains("Home"));
    }

    #[tokio::test]
    async fn jk_and_g_keys_navigate() {
        let mut app = booted_app().await;
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Char('j'))).await;
        assert_eq!(app.selected, 1);
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Char('k'))).await;
        assert_eq!(app.selected, 0);
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Char('G'))).await;
        assert_eq!(app.selected, app.posts.len() - 1);
        handle_key_with_terminal(&mut app, KeyEvent::from(KeyCode::Char('g'))).await;
        assert_eq!(app.selected, 0);
    }

    #[tokio::test]
    async fn feed_header_reflects_the_toggle() {
        let mut app = booted_app().await;
        assert!(rendered(&mut app, 150, 32).join("\n").contains("Following"));
        app.browser_mode = true;
        app.toggle_feed();
        app.drain().await;
        assert_eq!(app.feed_kind, FeedKind::ForYou);
        assert!(rendered(&mut app, 150, 32).join("\n").contains("For You"));
    }

    /// The event loop's Paste path feeds the query box.
    #[tokio::test]
    async fn pasted_search_text_lands_in_the_query() {
        let mut app = booted_app().await;
        app.begin_search();
        app.insert_query_text("from:ada_codes rust");
        assert_eq!(app.query, "from:ada_codes rust");
    }
}
