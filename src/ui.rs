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
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap},
};
use std::{
    io::{self, stdout},
    sync::OnceLock,
    time::{Duration, Instant},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

mod landing;
mod motion;
pub mod screenshot;

/// XTUI's visual language is a true-black canvas, white type, and a small
/// grayscale ramp for hierarchy. Author names are the one reserved hue: a
/// cool steel that sits next to the gray ramp instead of a stock sky blue.
#[derive(Clone, Debug)]
pub struct Theme {
    pub white: Color,
    pub gray: Color,
    pub dim: Color,
    pub surface: Color,
    pub surface_raised: Color,
    pub accent: Color,
    pub author: Color,
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
            author: Color::Rgb(168, 196, 214),
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

fn gray(luma: u8) -> Color {
    Color::Rgb(luma, luma, luma)
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
    // Query graphics capabilities after the alternate screen is up so Sixel /
    // Kitty / iTerm2 can paint real pixels instead of unicode blocks.
    app.attach_image_engine();
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
    let mut last_landing_motion_frame = landing_motion_frame(app);
    while !app.should_quit {
        // While work is in flight the poll window shrinks to a tick so the
        // spinner animates; otherwise XTUI sleeps until a key, mouse, resize
        // or the 30 s relative-time refresh.
        let busy = app.has_pending() || app.has_media_inflight();
        let landing_motion = matches!(app.screen, Screen::Landing);
        let timeout = if busy || app.browser_mode {
            Duration::from_millis(125)
        } else if motion::enabled() && landing_motion {
            Duration::from_millis(80)
        } else {
            Duration::from_secs(30)
        };
        let event = event::poll(timeout)?;
        let mut input_changed = false;
        if event {
            match event::read()? {
                Event::Key(key)
                    if key.kind == KeyEventKind::Press
                        || (key.kind == KeyEventKind::Repeat && key_repeats(key.code)) =>
                {
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
        let landing_motion_frame = landing_motion_frame(app);
        let landing_motion_changed = motion::enabled()
            && matches!(app.screen, Screen::Landing)
            && landing_motion_frame != last_landing_motion_frame;
        // Silent background refresh keeps Home live without stealing focus.
        let refresh_interval = if app.browser_mode {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(app.auto_refresh_secs)
        };
        if (app.browser_mode || app.auto_refresh_secs > 0)
            && !app.demo
            && app.mode == InputMode::Normal
            && !app.nav_focused
            && matches!(app.screen, Screen::Home)
            && !app.has_pending()
            && last_auto_refresh.elapsed() >= refresh_interval
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

fn key_repeats(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Backspace
            | KeyCode::Char('j')
            | KeyCode::Char('k')
    )
}

fn landing_motion_frame(app: &App) -> u64 {
    if matches!(app.screen, Screen::Landing) {
        landing::motion_frame()
    } else {
        0
    }
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
            render_help(frame, area, app);
        }
        return;
    }
    if area.width >= 78 {
        let cols = Layout::horizontal([Constraint::Length(18), Constraint::Min(48)]).split(area);
        render_nav(frame, cols[0], app, false);
        render_center(frame, cols[1], app);
    } else {
        let rows = Layout::vertical([Constraint::Min(10), Constraint::Length(2)]).split(area);
        render_center(frame, rows[0], app);
        render_bottom_nav(frame, rows[1], app);
    }
    if app.help {
        render_help(frame, area, app);
    }
    if app.media_preview.is_some() {
        render_media(frame, area, app);
    }
}

/// Landing-page X, then plain "tui". The six-row letter is 8 columns; the
/// lockup stays inside the 17-column rail.
const NAV_X: &[&str] = &[
    "██╗  ██╗",
    "╚██╗██╔╝",
    " ╚███╔╝ ",
    " ██╔██╗ ",
    "██╔╝ ██╗",
    "╚═╝  ╚═╝",
];
const NAV_X_LUMA: [u8; 6] = [226, 205, 184, 164, 142, 122];

fn render_nav_wordmark_text(frame: &mut Frame, area: Rect) -> u16 {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "xtui",
            Style::default()
                .fg(theme().white)
                .add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center),
        Rect {
            height: area.height.min(1),
            ..area
        },
    );
    1
}

fn render_nav_wordmark(frame: &mut Frame, area: Rect) -> u16 {
    if area.height < 6 || area.width < 12 {
        return render_nav_wordmark_text(frame, area);
    }
    let lines: Vec<Line> = NAV_X
        .iter()
        .enumerate()
        .map(|(index, letter)| {
            let mut spans = vec![Span::styled(
                *letter,
                Style::default()
                    .fg(gray(NAV_X_LUMA[index]))
                    .add_modifier(Modifier::BOLD),
            )];
            // Pad every row to the same width so centering does not jog the X.
            spans.push(Span::styled(
                if index == 5 { " tui" } else { "    " },
                Style::default()
                    .fg(theme().white)
                    .add_modifier(Modifier::BOLD),
            ));
            Line::from(spans).alignment(Alignment::Center)
        })
        .collect();
    let mark_height = lines.len() as u16;
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            height: area.height.min(mark_height),
            ..area
        },
    );
    mark_height
}

fn render_nav(frame: &mut Frame, area: Rect, app: &mut App, compact: bool) {
    let active = app.nav_index();
    let items = ["Home", "Explore", "Mentions", "Bookmarks", "Lists"];
    frame.render_widget(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(theme().dim))
            .style(Style::default().bg(theme().background)),
        area,
    );
    let inner = Rect {
        width: area.width.saturating_sub(1),
        ..area
    };
    let mark_height = if compact {
        render_nav_wordmark_text(frame, inner)
    } else {
        render_nav_wordmark(frame, inner)
    };

    let stride = 2u16;
    let nav_top = area.y.saturating_add(mark_height.saturating_add(2));
    for (index, label) in items.iter().enumerate() {
        let y = nav_top.saturating_add(index as u16 * stride);
        if y >= area.bottom() {
            break;
        }
        let is_active = index == active;
        let is_focused = app.nav_focused && index == app.nav_selected;
        let is_hovered = app.hovered == Some(HitAction::Nav(index));
        let marker = if is_focused || (is_active && !app.nav_focused) || is_hovered {
            ">"
        } else {
            " "
        };
        let style = if is_focused {
            Style::default()
                .fg(theme().background)
                .bg(theme().white)
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
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                format!("{marker} {label}"),
                style,
            )]))
            .alignment(Alignment::Center)
            .style(style),
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
        );
        app.register_hit(
            inner.x,
            y,
            inner.width,
            stride.saturating_sub(1).max(1),
            HitAction::Nav(index),
        );
    }

    if !compact {
        let width = inner.width as usize;
        let handle = app
            .me
            .as_ref()
            .map(|user| truncate_to_width(&user.username, width.saturating_sub(2)))
            .unwrap_or_else(|| "demo".into());
        let account = Rect {
            x: inner.x,
            y: inner.bottom().saturating_sub(2),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("@{handle}"),
                Style::default().fg(theme().gray),
            )))
            .alignment(Alignment::Center)
            .style(Style::default().bg(theme().background)),
            account,
        );
    }
}

fn render_card_rail(frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let lines = vec![
        Line::from(Span::styled(
            "┃",
            Style::default()
                .fg(theme().white)
                .bg(theme().surface_raised)
                .add_modifier(Modifier::BOLD),
        ));
        area.height as usize
    ];
    frame.render_widget(Paragraph::new(lines), area);
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
                                .fg(theme().background)
                                .bg(theme().white)
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
    frame.render_widget(
        Paragraph::new(line).alignment(Alignment::Center).block(
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
        app.register_hit(x, area.y.saturating_add(1), width, 1, HitAction::Nav(index));
    }
}

fn render_center(frame: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(header_height(app)),
        Constraint::Min(4),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(area);
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
    let width = chunks[2].width as usize;
    let indicator = if app.error.is_some() { '!' } else { '·' };
    let status_line = Line::from(vec![
        Span::styled(format!(" {indicator} "), status_style),
        Span::styled(
            truncate_to_width(&status, width.saturating_sub(4)),
            Style::default().fg(theme().gray),
        ),
    ]);
    // Status and hints are separate rows so a long error cannot push the
    // arrow keycaps off the bottom of the screen.
    frame.render_widget(
        Paragraph::new(status_line).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme().dim)),
        ),
        chunks[2],
    );
    let hints = if app.mode == InputMode::Search {
        Line::from(vec![
            keycap("Enter", true),
            Span::styled("search   ", Style::default().fg(theme().dim)),
            keycap("Esc", false),
            Span::styled("cancel   ", Style::default().fg(theme().dim)),
            keycap("Ctrl+U", false),
            Span::styled("clear", Style::default().fg(theme().dim)),
        ])
    } else {
        Line::from(vec![
            keycap("↑↓", false),
            Span::styled("move   ", Style::default().fg(theme().dim)),
            keycap("→", true),
            Span::styled("open   ", Style::default().fg(theme().dim)),
            keycap("←", false),
            Span::styled("back   ", Style::default().fg(theme().dim)),
            keycap("/", false),
            Span::styled("search   ", Style::default().fg(theme().dim)),
            keycap("?", false),
            Span::styled("help", Style::default().fg(theme().dim)),
        ])
    };
    frame.render_widget(Paragraph::new(hints), chunks[3]);
    if app.mode == InputMode::Normal && width >= 42 {
        let key_row = chunks[3].y;
        app.register_hit(
            chunks[3].right().saturating_sub(8),
            key_row,
            8,
            1,
            HitAction::Help,
        );
        app.register_hit(
            chunks[3].right().saturating_sub(18),
            key_row,
            10,
            1,
            HitAction::Search,
        );
    }
}

fn keycap(label: impl Into<String>, active: bool) -> Span<'static> {
    let style = if active {
        Style::default()
            .fg(theme().background)
            .bg(theme().white)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme().white)
            .bg(theme().surface_raised)
    };
    Span::styled(format!(" {} ", label.into()), style)
}

fn header_height(app: &App) -> u16 {
    match app.screen {
        Screen::Profile(_) => 7,
        Screen::ListFeed(_) => 5,
        _ => 3,
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &mut App) {
    // Route identity, title, position and activity share one stable line. The
    // count is treated as a compact instrument readout instead of free text.
    let mut title = Line::from(vec![
        Span::styled(
            if app.history.is_empty() {
                "   "
            } else {
                " ← "
            },
            if app.hovered == Some(HitAction::Back) {
                Style::default()
                    .fg(theme().background)
                    .bg(theme().white)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme().gray).bg(theme().surface_raised)
            },
        ),
        Span::raw(" "),
        Span::styled(
            app.title(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let mut right = String::new();
    if !app.posts.is_empty() && !matches!(app.screen, Screen::Lists) {
        right.push_str(&format!("{} / {}", app.selected + 1, app.posts.len()));
    }
    if !right.is_empty() {
        let right = format!(" {right} ");
        let used = title.width() + right.width() + 1;
        let padding = (area.width as usize).saturating_sub(used);
        title.push_span(Span::raw(" ".repeat(padding)));
        title.push_span(Span::styled(
            right,
            Style::default()
                .fg(if app.has_pending() {
                    theme().accent
                } else {
                    theme().white
                })
                .bg(theme().surface_raised)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let mut lines = vec![title];
    let mut wrap = false;
    match &app.screen {
        Screen::Home => {
            let hovered = app.hovered == Some(HitAction::ToggleFeed);
            let following_active = app.feed_kind == FeedKind::Following;
            let for_you_active = app.feed_kind == FeedKind::ForYou;
            let segment = |label: &'static str, active: bool| {
                Span::styled(
                    format!(" {label} "),
                    if active || hovered {
                        Style::default()
                            .fg(theme().background)
                            .bg(theme().white)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(if app.browser_mode {
                                theme().gray
                            } else {
                                theme().dim
                            })
                            .bg(theme().surface_raised)
                    },
                )
            };
            lines.push(Line::from(vec![
                Span::styled("   ", Style::default()),
                segment("Following", following_active),
                Span::raw("  "),
                segment("For You", for_you_active),
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
        if app.has_pending() {
            render_loading(frame, area, app);
            return;
        }
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
            "  Loading this conversation and its replies…".into()
        } else if matches!(app.screen, Screen::Thread(_)) {
            "  This conversation has no posts.".into()
        } else {
            "  Nothing here yet.\n  Press r to refresh or / to search.".into()
        };
        let panel = centered(
            area,
            area.width.saturating_sub(4).min(64),
            area.height.saturating_sub(2).min(9),
        );
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().fg(theme().gray))
                .alignment(Alignment::Center),
            panel,
        );
        return;
    }
    // Cards are stacked manually. Only the focused card wears a full frame;
    // the rest separate with a thin rule so the feed reads as a single
    // scrolling column instead of a wall of boxes. The feed window begins at
    // the selected post, so reading position stays pinned while rendering
    // stays proportional to the viewport.
    let width = area.width.saturating_sub(5) as usize;
    let thread_view = matches!(app.screen, Screen::Thread(_));
    let visible: Vec<usize> = app
        .posts
        .iter()
        .enumerate()
        .skip(selected)
        .take(if collapsed { 1 } else { 14 })
        .map(|(index, _)| index)
        .collect();
    let pending_urls: Vec<String> = visible
        .iter()
        .filter_map(|&index| {
            app.posts.get(index).and_then(|post| {
                post.reposted
                    .as_deref()
                    .unwrap_or(post)
                    .media
                    .first()
                    .and_then(crate::media::best_preview_url)
            })
        })
        .collect();
    app.ensure_visible_media(pending_urls);
    let font = app
        .image_engine
        .as_ref()
        .map(|engine| engine.font_size())
        .unwrap_or_else(crate::media::default_font);
    if thread_view {
        let image_rows = app
            .selected_post()
            .and_then(|post| {
                let url = crate::media::best_preview_url(
                    post.reposted.as_deref().unwrap_or(post).media.first()?,
                )?;
                let image = app.cached_media(&url)?;
                Some(
                    crate::media::fit_cells(
                        image,
                        font,
                        crate::media::INLINE_MAX_COLS,
                        crate::media::INLINE_MAX_ROWS,
                    )
                    .1,
                )
            })
            .unwrap_or(0);
        let body_limit = (area.height as usize)
            .saturating_sub(6 + image_rows as usize)
            .max(3);
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
    for index in visible {
        let Some(post) = app.posts.get(index) else {
            break;
        };
        let focused = index == selected;
        let hovered = app.hovered == Some(HitAction::Card(index));
        let reply = matches!(app.screen, Screen::Thread(_)) && index > 0;
        let media_url = post
            .reposted
            .as_deref()
            .unwrap_or(post)
            .media
            .first()
            .and_then(crate::media::best_preview_url);
        let cached = media_url
            .as_ref()
            .and_then(|url| app.cached_media(url).cloned());
        let max_image_rows = if focused {
            crate::media::INLINE_MAX_ROWS
        } else {
            crate::media::PREVIEW_MAX_ROWS
        };
        let (image_cols, image_rows) = cached
            .as_ref()
            .map(|image| {
                crate::media::fit_cells(
                    image,
                    font,
                    (width as u16).min(crate::media::INLINE_MAX_COLS),
                    max_image_rows,
                )
            })
            .unwrap_or((0, 0));
        let body_limit = if focused && thread_view {
            (area.height as usize)
                .saturating_sub(6 + image_rows as usize)
                .max(3)
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
        let (header, footer) = post_lines(post, width, reply, focused, body_limit, body_offset);
        let remaining = area.bottom().saturating_sub(y);
        if remaining < 2 {
            break;
        }
        let text_rows = (header.len() + footer.len()) as u16;
        let content_height =
            (text_rows.saturating_add(image_rows)).min(remaining.saturating_sub(1).max(1));
        let height = (content_height + 1).min(remaining);
        let content = Rect {
            x: area.x,
            y,
            width: area.width,
            height: content_height,
        };
        if focused || hovered {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme().surface_raised)),
                content,
            );
        }
        let text_x = if focused {
            render_card_rail(
                frame,
                Rect {
                    x: content.x,
                    y: content.y,
                    width: 1,
                    height: content.height,
                },
            );
            content.x.saturating_add(1)
        } else {
            content.x
        };
        let text_width = content.width.saturating_sub(if focused { 1 } else { 0 });
        let header_h = (header.len() as u16).min(content.height);
        frame.render_widget(
            Paragraph::new(header),
            Rect {
                x: text_x,
                y: content.y,
                width: text_width,
                height: header_h,
            },
        );
        let mut cursor = content.y.saturating_add(header_h);
        if let (Some(url), Some(image)) = (media_url.as_ref(), cached.as_ref()) {
            let slot_h = image_rows.min(content.bottom().saturating_sub(cursor));
            if slot_h > 0 {
                let slot = Rect {
                    x: text_x.saturating_add(2),
                    y: cursor,
                    width: image_cols.min(text_width.saturating_sub(2)).max(1),
                    height: slot_h,
                };
                if let Some(engine) = app.image_engine.as_mut() {
                    engine.render(frame, slot, url, image);
                }
                cursor = cursor.saturating_add(slot_h);
            }
        }
        let footer_h = (footer.len() as u16).min(content.bottom().saturating_sub(cursor));
        if footer_h > 0 {
            frame.render_widget(
                Paragraph::new(footer),
                Rect {
                    x: text_x,
                    y: cursor,
                    width: text_width,
                    height: footer_h,
                },
            );
        }
        if height > content_height {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "─".repeat(area.width as usize),
                    Style::default().fg(theme().dim),
                ))),
                Rect {
                    x: area.x,
                    y: y.saturating_add(content_height),
                    width: area.width,
                    height: 1,
                },
            );
        }
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

fn loading_subject(app: &App) -> &'static str {
    match app.screen {
        Screen::Home => "home",
        Screen::Explore => "search",
        Screen::Mentions => "mentions",
        Screen::Bookmarks => "bookmarks",
        Screen::Lists => "lists",
        Screen::ListFeed(_) => "list",
        Screen::Profile(_) => "profile",
        Screen::Likes(_) => "likes",
        Screen::Thread(_) => "conversation",
        Screen::Landing => "session",
    }
}

fn render_loading(frame: &mut Frame, area: Rect, app: &App) {
    let spinner = app.spinner_frame().unwrap_or('·');
    let bar_width = area.width.saturating_sub(2).max(8) as usize;
    let block = (bar_width / 4).max(8).min(bar_width);
    let travel = bar_width.saturating_sub(block).max(1);
    let cycle_ms = 1400u128;
    let phase = (app.activity_ms() % cycle_ms) as f32 / cycle_ms as f32;
    let ping = if phase < 0.5 {
        phase * 2.0
    } else {
        2.0 - phase * 2.0
    };
    let start = (ping * travel as f32).round() as usize;
    let bar: String = (0..bar_width)
        .map(|index| {
            if index >= start && index < start + block {
                '█'
            } else {
                '─'
            }
        })
        .collect();
    let pulse = gray(motion::luma(150, 245, motion::pulse(1.8, 0.0)));
    let mid = area.y.saturating_add(area.height / 2);
    let title_y = mid.saturating_sub(2).max(area.y);
    let bar_y = mid.min(area.bottom().saturating_sub(2));
    let caption_y = bar_y.saturating_add(2).min(area.bottom().saturating_sub(1));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                "{spinner}  loading {subject}",
                subject = loading_subject(app)
            ),
            Style::default()
                .fg(theme().white)
                .add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center),
        Rect {
            x: area.x,
            y: title_y,
            width: area.width,
            height: 1,
        },
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(bar, Style::default().fg(pulse)))),
        Rect {
            x: area.x.saturating_add(1),
            y: bar_y,
            width: area.width.saturating_sub(2).max(1),
            height: 1,
        },
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Loading posts…",
                Style::default().fg(theme().gray),
            )),
            Line::from(Span::styled(
                app.status.clone(),
                Style::default().fg(theme().dim),
            )),
        ])
        .alignment(Alignment::Center),
        Rect {
            x: area.x,
            y: caption_y,
            width: area.width,
            height: area.bottom().saturating_sub(caption_y).min(2),
        },
    );
}

fn post_lines(
    post: &Post,
    width: usize,
    reply: bool,
    focused: bool,
    body_limit: usize,
    body_offset: usize,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let mut lines = Vec::new();
    if post.reposted.is_some() {
        lines.push(Line::from(Span::styled(
            "  ↻  Reposted",
            Style::default().fg(theme().gray),
        )));
    }
    let prefix = if reply { "  └─ " } else { "  " };
    lines.push(Line::from(vec![
        Span::styled(prefix, Style::default().fg(theme().dim)),
        Span::styled(
            post.author.name.clone(),
            Style::default()
                .fg(theme().author)
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
                    .fg(theme().author)
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
                    m.alt_text.as_deref().unwrap_or("M enlarge · V open")
                ),
                Style::default().fg(theme().gray),
            ),
        ]));
    }
    let header = lines;
    let mut footer = Vec::new();
    let metrics = [
        ("◯", compact(target.metrics.reply_count)),
        ("↻", compact(target.metrics.retweet_count)),
        ("♡", compact(target.metrics.like_count)),
        ("◉", compact(target.metrics.impression_count.unwrap_or(0))),
    ];
    let mut metric_line = vec![Span::raw("  ")];
    for (icon, number) in metrics {
        metric_line.push(Span::styled(icon, Style::default().fg(theme().dim)));
        metric_line.push(Span::styled(
            format!(" {number} "),
            Style::default()
                .fg(if focused { theme().white } else { theme().gray })
                .bg(if focused {
                    theme().surface
                } else {
                    theme().background
                }),
        ));
        metric_line.push(Span::raw("  "));
    }
    footer.push(Line::from(metric_line));
    if focused {
        footer.push(Line::from(vec![
            Span::styled("  ENTER", Style::default().fg(theme().white)),
            Span::styled(" OPEN   ", Style::default().fg(theme().dim)),
            Span::styled("P", Style::default().fg(theme().white)),
            Span::styled(" AUTHOR   ", Style::default().fg(theme().dim)),
            Span::styled("M", Style::default().fg(theme().white)),
            Span::styled(" MEDIA   ", Style::default().fg(theme().dim)),
            Span::styled("O", Style::default().fg(theme().white)),
            Span::styled(" X.COM", Style::default().fg(theme().dim)),
        ]));
    }
    (header, footer)
}

fn render_lists(frame: &mut Frame, area: Rect, app: &mut App) {
    app.card_rows.clear();
    if app.lists.is_empty() {
        if app.has_pending() {
            render_loading(frame, area, app);
            return;
        }
        let panel = centered(
            area,
            area.width.saturating_sub(4).min(60),
            area.height.saturating_sub(2).min(8),
        );
        frame.render_widget(
            Paragraph::new("No lists yet. Press r to refresh.")
                .style(Style::default().fg(theme().gray))
                .alignment(Alignment::Center),
            panel,
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
        let height = (lines.len() as u16 + 1).min(remaining);
        let block = Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(theme().dim))
            .style(Style::default().bg(if focused || hovered {
                theme().surface_raised
            } else {
                theme().background
            }));
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

#[allow(dead_code)]
fn render_context(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::vertical([
        Constraint::Length(14),
        Constraint::Length(9),
        Constraint::Min(6),
        Constraint::Length(1),
    ])
    .split(area);
    let selected = app.selected_post();
    let position = if app.posts.is_empty() {
        "NO SIGNAL SELECTED".into()
    } else {
        format!("POST {:02} / {:02}", app.selected + 1, app.posts.len())
    };
    let meter_width = area.width.saturating_sub(8) as usize;
    let meter_position = if app.posts.is_empty() {
        0
    } else {
        (app.selected * meter_width) / app.posts.len().max(1)
    };
    let meter = (0..meter_width)
        .map(|index| {
            let luma = if index == meter_position {
                motion::luma(178, 255, motion::pulse(1.6, 0.0))
            } else if index < meter_position {
                112
            } else {
                48
            };
            Span::styled(
                if index == meter_position {
                    "◆"
                } else {
                    "━"
                },
                Style::default().fg(gray(luma)),
            )
        })
        .collect::<Vec<_>>();
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
        Line::from(Span::styled(
            selected
                .map(|post| format!("RECEIVED / {}", relative_time(post.created_at)))
                .unwrap_or_else(|| "AWAITING SIGNAL".into()),
            Style::default().fg(theme().dim),
        )),
        Line::from(Span::styled(
            "────────────────────────────",
            Style::default().fg(theme().dim),
        )),
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
        Line::from(meter),
        Line::from(Span::styled(
            "READ POSITION / PINNED",
            Style::default().fg(theme().dim),
        )),
    ];
    frame.render_widget(
        Paragraph::new(summary)
            .wrap(Wrap { trim: false })
            .block(instrument_block("  ▍ CURRENT SIGNAL  ", 0.0)),
        rows[0],
    );
    let commands = vec![
        context_command(&app.keys.key_label(Action::Open), "OPEN SIGNAL", true),
        context_command(
            &app.keys.key_label(Action::Profile),
            "AUTHOR PROFILE",
            false,
        ),
        context_command(
            &app.keys.key_label(Action::MediaPreview),
            "MEDIA PREVIEW",
            false,
        ),
        context_command(
            &app.keys.key_label(Action::OpenPost),
            "OPEN ON X.COM",
            false,
        ),
        context_command(&app.keys.key_label(Action::Back), "BACK", false),
    ];
    frame.render_widget(
        Paragraph::new(commands).block(instrument_block("  COMMANDS  ", 0.7)),
        rows[1],
    );

    let source = if app.demo {
        "LOCAL / DEMO"
    } else if app.browser_mode {
        "LIVE / EXTENSION"
    } else {
        "LIVE / X API"
    };
    let buffer = format!("{:03} SIGNALS", app.posts.len());
    let session = vec![
        context_value("SOURCE", source),
        context_value(
            "FEED",
            match app.feed_kind {
                FeedKind::Following => "FOLLOWING",
                FeedKind::ForYou => "FOR YOU",
            },
        ),
        context_value("BUFFER", &buffer),
        context_value(
            "MOTION",
            if motion::enabled() {
                "ACTIVE / 6 FPS"
            } else {
                "REDUCED"
            },
        ),
    ];
    frame.render_widget(
        Paragraph::new(session).block(instrument_block("  SESSION  ", 1.4)),
        rows[2],
    );

    let footer = Line::from(vec![
        keycap("?", false),
        Span::styled(" COMMAND INDEX   ", Style::default().fg(theme().gray)),
        keycap("Q", false),
        Span::styled(" QUIT", Style::default().fg(theme().gray)),
    ])
    .alignment(Alignment::Right);
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().bg(theme().background)),
        rows[3],
    );
}

fn instrument_block(title: &'static str, offset: f32) -> Block<'static> {
    Block::default()
        .title(title)
        .title_style(
            Style::default()
                .fg(gray(motion::luma(156, 245, motion::pulse(2.4, offset))))
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(gray(motion::luma(48, 72, motion::pulse(4.0, offset)))))
        .padding(Padding::horizontal(1))
        .style(Style::default().bg(theme().surface))
}

fn context_command(key: &str, label: &'static str, primary: bool) -> Line<'static> {
    Line::from(vec![
        keycap(truncate_to_width(key, 8), primary),
        Span::styled(format!("  {label}"), Style::default().fg(theme().gray)),
    ])
}

fn context_value(label: &'static str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<9}"), Style::default().fg(theme().dim)),
        Span::styled(value.to_owned(), Style::default().fg(theme().gray)),
    ])
}

fn render_help(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered(area, 84, 25);
    let shadow = Rect {
        x: popup.x.saturating_add(1).min(area.right()),
        y: popup.y.saturating_add(1).min(area.bottom()),
        width: popup
            .width
            .min(area.right().saturating_sub(popup.x.saturating_add(1))),
        height: popup
            .height
            .min(area.bottom().saturating_sub(popup.y.saturating_add(1))),
    };
    frame.render_widget(Clear, shadow);
    frame.render_widget(
        Block::default().style(Style::default().bg(gray(20))),
        shadow,
    );
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title("  Keyboard shortcuts  ")
        .title_style(
            Style::default()
                .fg(theme().white)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().gray))
        .style(Style::default().bg(theme().surface_raised))
        .padding(Padding::uniform(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let binding = |action| app.keys.bound_keys(action).join(" / ");
    let lines = vec![
        Line::from(vec![
            Span::styled("READ / NAVIGATE", Style::default().fg(theme().white)),
            Span::styled(
                "  ───────────────────────────────────────────────────────────",
                Style::default().fg(theme().dim),
            ),
        ]),
        help_pair(
            &binding(Action::MoveDown),
            "Next signal",
            &binding(Action::MoveUp),
            "Previous signal",
        ),
        help_pair(
            &format!(
                "{} / {}",
                binding(Action::PageDown),
                binding(Action::PageUp)
            ),
            "Jump five",
            &format!("{} / {}", binding(Action::Top), binding(Action::Bottom)),
            "Top / bottom",
        ),
        help_pair(
            &binding(Action::Open),
            "Open selection",
            &binding(Action::Back),
            "Back / leave",
        ),
        Line::from(""),
        Line::from(vec![
            Span::styled("ROUTES / DISCOVERY", Style::default().fg(theme().white)),
            Span::styled(
                "  ────────────────────────────────────────────────────────",
                Style::default().fg(theme().dim),
            ),
        ]),
        help_pair(
            &binding(Action::Sidebar),
            "Focus route rail",
            &binding(Action::Search),
            "Search X",
        ),
        help_pair(
            "1 — 5",
            "Direct route",
            &binding(Action::Refresh),
            "Refresh",
        ),
        help_pair(
            &binding(Action::ToggleFeed),
            "Following / For You",
            &binding(Action::ExpandThread),
            "Expand replies",
        ),
        Line::from(""),
        Line::from(vec![
            Span::styled("CONTEXT / HANDOFF", Style::default().fg(theme().white)),
            Span::styled(
                "  ─────────────────────────────────────────────────────────",
                Style::default().fg(theme().dim),
            ),
        ]),
        help_pair(
            &binding(Action::Profile),
            "Author profile",
            &binding(Action::Likes),
            "Liked posts",
        ),
        help_pair(
            &binding(Action::MediaPreview),
            "Media preview",
            &binding(Action::OpenMedia),
            "Open media",
        ),
        help_pair(
            &binding(Action::OpenPost),
            "Open on x.com",
            &binding(Action::Quit),
            "Quit XTUI",
        ),
        Line::from(""),
        Line::from(vec![
            Span::styled("MOUSE", Style::default().fg(theme().dim)),
            Span::styled(
                "  Wheel moves · hover reveals targets · click opens",
                Style::default().fg(theme().gray),
            ),
        ]),
        Line::from(vec![
            Span::styled("FOCUS MODEL", Style::default().fg(theme().dim)),
            Span::styled(
                "  The selected post stays pinned; the next posts preview below.",
                Style::default().fg(theme().gray),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            keycap("Q / Esc / ?", true),
            Span::styled("  CLOSE COMMAND INDEX", Style::default().fg(theme().gray)),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn help_pair(
    left_key: &str,
    left_label: &'static str,
    right_key: &str,
    right_label: &'static str,
) -> Line<'static> {
    let left_key = truncate_to_width(left_key, 10);
    let right_key = truncate_to_width(right_key, 10);
    Line::from(vec![
        Span::styled(
            format!(" {left_key:<10} "),
            Style::default().fg(theme().white).bg(theme().background),
        ),
        Span::styled(
            format!(" {left_label:<22}"),
            Style::default().fg(theme().gray),
        ),
        Span::styled(
            format!(" {right_key:<10} "),
            Style::default().fg(theme().white).bg(theme().background),
        ),
        Span::styled(format!(" {right_label}"), Style::default().fg(theme().gray)),
    ])
}

fn render_media(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some((alt, image)) = app.media_preview.clone() else {
        return;
    };
    let url = app.selected_preview_url().unwrap_or_else(|| alt.clone());
    let font = app
        .image_engine
        .as_ref()
        .map(|engine| engine.font_size())
        .unwrap_or_else(crate::media::default_font);
    let max_cols = area
        .width
        .saturating_sub(8)
        .min(crate::media::MODAL_MAX_COLS);
    let max_rows = area
        .height
        .saturating_sub(10)
        .min(crate::media::MODAL_MAX_ROWS);
    let (cols, rows) = crate::media::fit_cells(&image, font, max_cols, max_rows);
    let popup = centered(
        area,
        (cols.saturating_add(6))
            .min(area.width.saturating_sub(2))
            .max(28),
        (rows.saturating_add(7))
            .min(area.height.saturating_sub(2))
            .max(10),
    );
    let shadow = Rect {
        x: popup.x.saturating_add(1).min(area.right()),
        y: popup.y.saturating_add(1).min(area.bottom()),
        width: popup
            .width
            .min(area.right().saturating_sub(popup.x.saturating_add(1))),
        height: popup
            .height
            .min(area.bottom().saturating_sub(popup.y.saturating_add(1))),
    };
    frame.render_widget(Clear, shadow);
    frame.render_widget(
        Block::default().style(Style::default().bg(gray(20))),
        shadow,
    );
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title("  Media preview  ")
        .title_style(
            Style::default()
                .fg(theme().white)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().gray))
        .padding(Padding::uniform(1))
        .style(Style::default().bg(theme().surface_raised));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let image_area = Rect {
        x: inner.x,
        y: inner.y,
        width: cols.min(inner.width).max(1),
        height: rows.min(inner.height.saturating_sub(3)).max(1),
    };
    if let Some(engine) = app.image_engine.as_mut() {
        engine.render(frame, image_area, &url, &image);
    }
    let hints = inner.y.saturating_add(image_area.height.saturating_add(1));
    if hints < inner.bottom() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    truncate_to_width(&alt, inner.width as usize),
                    Style::default().fg(theme().gray),
                )),
                Line::from(vec![
                    keycap("V", true),
                    Span::styled(" OPEN MEDIA   ", Style::default().fg(theme().dim)),
                    keycap("O", false),
                    Span::styled(" X.COM   ", Style::default().fg(theme().dim)),
                    keycap("Esc", false),
                    Span::styled(" CLOSE", Style::default().fg(theme().dim)),
                ]),
            ]),
            Rect {
                x: inner.x,
                y: hints,
                width: inner.width,
                height: inner.bottom().saturating_sub(hints).min(2),
            },
        );
    }
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
            assert!(
                !all.contains("CURRENT SIGNAL"),
                "context rail must stay hidden"
            );
            assert!(
                !all.contains("KEYBOARD"),
                "keyboard dock must be gone at {width} columns"
            );
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
        assert!(!rows.join("\n").contains("FOCUSED"));
    }

    #[tokio::test]
    async fn held_arrow_keys_keep_moving() {
        let mut app = booted_app().await;
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.selected, 1);
        let mut repeat = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        repeat.kind = KeyEventKind::Repeat;
        handle_key(&mut app, repeat);
        assert_eq!(app.selected, 2, "key repeat must keep scrolling the feed");
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
        rendered(&mut app, 110, 44);
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
            all.contains("████") && all.lines().any(|line| line.matches('█').count() >= 14),
            "the XTUI wordmark is present"
        );
        assert!(
            all.contains('░') && all.contains('▒'),
            "the wordmark keeps its layered extrusion"
        );
        assert!(
            all.contains("Browse X in your Terminal."),
            "the product line is present"
        );
        assert!(
            all.contains(&format!("xtui version {}", env!("CARGO_PKG_VERSION"))),
            "the version line is present"
        );
        assert!(all.contains("Start"), "the start option is present");
        assert!(
            !all.contains("Start reading"),
            "the start label is the short form"
        );
        assert!(
            all.contains("Connect browser extension"),
            "the extension option is present when disconnected"
        );
        assert!(!all.contains("System /"), "system chrome is gone");
        assert!(!all.contains("Identity /"), "identity chrome is gone");
        assert!(!all.contains("Entry points"), "the command deck is gone");
        assert!(all.contains("↑↓"), "navigation hints use arrows");
        assert!(all.contains('→'), "right is listed as its own action");
        assert!(all.contains('←'), "left is listed as its own action");
        assert!(!all.contains("j/k"), "j/k is no longer advertised");
        assert!(!all.contains("ENTER"), "selected rows no longer show Enter");
        assert!(
            all.contains('╭') && all.contains('╰'),
            "the landing page is cropped by a rounded frame"
        );
    }

    #[tokio::test]
    async fn landing_degrades_cleanly_on_small_terminals() {
        let mut app = demo_app();
        let compact = rendered(&mut app, 60, 24).join("\n");
        assert!(
            compact.contains("Browse X in your Terminal."),
            "compact viewport keeps the identity"
        );
        assert!(compact.contains("Quit"), "compact menu remains complete");

        let tiny = rendered(&mut app, 44, 18).join("\n");
        assert!(!tiny.contains("██╗"), "tiny view hides art before controls");
        assert!(tiny.contains("xtui"), "tiny view keeps the identity");
        assert!(tiny.contains("Quit"), "tiny menu remains complete");
        assert!(tiny.contains('→'), "tiny controls remain discoverable");
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
    async fn reader_omits_decorative_chrome_and_keeps_post_content() {
        let mut app = booted_app().await;
        let all = rendered(&mut app, 150, 42).join("\n");
        assert!(all.contains("Home"));
        assert!(all.contains("Ada"));
        assert!(
            all.contains("██╗  ██╗") && all.contains("tui"),
            "the sidebar uses the landing X plus plain tui"
        );
        assert!(all.contains('┃'), "the selected post has a left rail");
        assert!(!all.contains("SIGNAL"));
        assert!(!all.contains("FOCUSED"));
        assert!(!all.contains("SESSION"));
    }

    #[tokio::test]
    async fn pending_reader_uses_an_animated_loading_screen() {
        let mut app = demo_app();
        app.screen = Screen::Home;
        app.request_bootstrap();
        let all = rendered(&mut app, 100, 32).join("\n");
        assert!(all.contains("Loading posts"));
        assert!(all.contains("loading home"));
        assert!(
            all.contains('█') || all.contains('▓') || all.contains('▒'),
            "the loader paints a moving bar"
        );
        assert!(!all.contains("BUFFERING"));
        app.drain().await;
    }

    #[tokio::test]
    async fn command_index_reflects_the_runtime_keymap() {
        let mut app = booted_app().await;
        app.help = true;
        let all = rendered(&mut app, 150, 42).join("\n");
        assert!(all.contains("Keyboard shortcuts"));
        assert!(all.contains("READ / NAVIGATE"));
        assert!(all.contains("CONTEXT / HANDOFF"));
        assert!(all.contains("j / ↓"), "configured bindings are rendered");
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
    async fn cards_use_selection_background_without_focus_chrome() {
        let mut app = booted_app().await;
        let rows = rendered(&mut app, 150, 36);
        let all = rows.join("\n");
        assert!(!all.contains("FOCUSED"));
        assert!(!all.contains("SIGNAL"));
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
