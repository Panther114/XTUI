use super::*;
use crate::app::LandingAction;

/// Skills-style block typography, re-composed for XTUI. A slow grayscale wave
/// gives it the restrained metallic motion used by the best terminal welcome
/// screens without bringing color into the interface.
const XTUI_WORDMARK: &[&str] = &[
    "██╗  ██╗████████╗██╗   ██╗██╗",
    "╚██╗██╔╝╚══██╔══╝██║   ██║██║",
    " ╚███╔╝    ██║   ██║   ██║██║",
    " ██╔██╗    ██║   ██║   ██║██║",
    "██╔╝ ██╗   ██║   ╚██████╔╝██║",
    "╚═╝  ╚═╝   ╚═╝    ╚═════╝ ╚═╝",
];

fn motion_phase() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

pub(super) fn motion_frame() -> u64 {
    (motion_phase() * 8.0) as u64
}

fn render_wordmark(frame: &mut Frame, area: Rect, scale_x: usize, scale_y: usize) {
    let phase = motion_phase();
    let rows = (XTUI_WORDMARK.len() * scale_y) as f32;
    let columns = (XTUI_WORDMARK
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(1)
        * scale_x) as f32;
    let cycle = (phase % 5.2) / 5.2;
    let beam = -0.28 + (cycle / 0.34).min(1.0) * 1.56;
    let resting = [218u8, 198, 174, 148, 122, 96];

    let art_width = XTUI_WORDMARK
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(1)
        * scale_x;
    let shadow_layers = [
        (6usize, 3usize, '⠈', 62u8),
        (4usize, 2usize, '░', 78u8),
        (2usize, 1usize, '▒', 96u8),
    ];
    let canvas_width = art_width + 6;
    let canvas_height = XTUI_WORDMARK.len() * scale_y + 3;
    let mut canvas = vec![vec![(' ', 0u8, false); canvas_width]; canvas_height];

    // Three offset dot/shade passes create the technical echo seen in the
    // reference: an outline-like extrusion, never a flat drop shadow.
    for (offset_x, offset_y, glyph, luma) in shadow_layers {
        for (row, text) in XTUI_WORDMARK.iter().enumerate() {
            for (column, character) in text.chars().enumerate() {
                if !character.is_whitespace() {
                    for dy in 0..scale_y {
                        for dx in 0..scale_x {
                            canvas[row * scale_y + dy + offset_y]
                                [column * scale_x + dx + offset_x] = (glyph, luma, false);
                        }
                    }
                }
            }
        }
    }

    for (row, text) in XTUI_WORDMARK.iter().enumerate() {
        for (column, character) in text.chars().enumerate() {
            if character.is_whitespace() {
                continue;
            }
            for dy in 0..scale_y {
                for dx in 0..scale_x {
                    let scaled_row = row * scale_y + dy;
                    let scaled_column = column * scale_x + dx;
                    let diagonal =
                        (scaled_column as f32 + (rows - scaled_row as f32)) / (columns + rows);
                    let distance = (diagonal - beam).abs();
                    let shine = if distance < 0.24 {
                        0.5 * (1.0 + (std::f32::consts::PI * distance / 0.24).cos())
                    } else {
                        0.0
                    };
                    let base = resting[row.min(resting.len() - 1)];
                    let luma = (f32::from(base) + f32::from(248 - base) * shine * 0.72) as u8;
                    canvas[scaled_row][scaled_column] = (character, luma, true);
                }
            }
        }
    }

    let lines = canvas
        .into_iter()
        .map(|cells| {
            let mut spans = Vec::new();
            let mut run = String::new();
            let mut run_style = None;
            for (character, luma, bold) in cells {
                if run_style != Some((luma, bold)) {
                    if let Some((previous, previous_bold)) = run_style {
                        let mut style =
                            Style::default().fg(Color::Rgb(previous, previous, previous));
                        if previous_bold {
                            style = style.add_modifier(Modifier::BOLD);
                        }
                        spans.push(Span::styled(std::mem::take(&mut run), style));
                    }
                    run_style = Some((luma, bold));
                }
                run.push(character);
            }
            if let Some((previous, previous_bold)) = run_style {
                let mut style = Style::default().fg(if previous == 0 {
                    theme().background
                } else {
                    Color::Rgb(previous, previous, previous)
                });
                if previous_bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                spans.push(Span::styled(run, style));
            }
            Line::from(spans).alignment(Alignment::Center)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
}

fn state(app: &App) -> (String, String) {
    let state = if app.error.is_some() {
        "Fault / r to retry".to_owned()
    } else if app.has_pending() {
        format!("{}  {}", app.spinner_frame().unwrap_or('·'), app.status)
    } else if app.demo {
        "Offline / demo".to_owned()
    } else if app.browser_mode {
        "Online / extension".to_owned()
    } else {
        "Online / X API".to_owned()
    };
    let identity = app
        .me
        .as_ref()
        .map(|user| format!("{} / @{}", user.name, user.username))
        .unwrap_or_else(|| {
            if app.demo {
                "Local / no account".into()
            } else {
                "Not linked".into()
            }
        });
    (state, identity)
}

fn mode(app: &App) -> &'static str {
    if app.demo {
        "demo"
    } else if app.browser_mode {
        "extension"
    } else {
        "API"
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let left = "xtui / terminal interface";
    let right = format!("v{} / {}", env!("CARGO_PKG_VERSION"), mode(app));
    let left = truncate_to_width(
        left,
        area.width.saturating_sub(right.width() as u16 + 1) as usize,
    );
    let gap = area
        .width
        .saturating_sub((left.width() + right.width()) as u16) as usize;
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(left, Style::default().fg(theme().gray)),
                Span::raw(" ".repeat(gap)),
                Span::styled(right, Style::default().fg(theme().dim)),
            ]),
            Line::from(Span::styled(
                "─".repeat(area.width as usize),
                Style::default().fg(theme().dim),
            )),
        ]),
        area,
    );
}

pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width < 88 || area.height < 30 {
        render_compact(frame, area, app);
        return;
    }

    let items = app.landing_items();
    if app.landing_selected >= items.len() {
        app.landing_selected = items.len().saturating_sub(1);
    }

    let shell = centered(
        area,
        area.width.saturating_sub(6).min(104),
        area.height.saturating_sub(2),
    );
    let large = shell.width >= 96 && shell.height >= 36;
    let scale_y = if large { 2 } else { 1 };
    let art_height = (XTUI_WORDMARK.len() * scale_y + 3) as u16;
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(art_height),
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Length(items.len() as u16),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(shell);

    render_header(frame, rows[0], app);
    render_wordmark(frame, rows[2], 2, scale_y);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Scroll X. Stay in flow.",
                Style::default().fg(theme().white),
            ))
            .alignment(Alignment::Center),
            Line::from(Span::styled(
                "A focused, keyboard-first timeline.",
                Style::default().fg(theme().gray),
            ))
            .alignment(Alignment::Center),
        ]),
        rows[3],
    );

    let (state, identity) = state(app);
    let left = format!("System / {state}");
    let right = truncate_to_width(
        &format!("Identity / {identity}"),
        rows[4].width.saturating_sub(12) as usize,
    );
    let left = truncate_to_width(
        &left,
        rows[4].width.saturating_sub(right.width() as u16 + 1) as usize,
    );
    let gap = rows[4]
        .width
        .saturating_sub((left.width() + right.width()) as u16) as usize;
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "─".repeat(rows[4].width as usize),
                Style::default().fg(theme().dim),
            )),
            Line::from(vec![
                Span::styled(left, Style::default().fg(theme().gray)),
                Span::raw(" ".repeat(gap)),
                Span::styled(right, Style::default().fg(theme().gray)),
            ]),
            Line::from(Span::styled(
                "─".repeat(rows[4].width as usize),
                Style::default().fg(theme().dim),
            )),
        ]),
        rows[4],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Entry points", Style::default().fg(theme().gray)),
            Span::styled(
                format!(" / {:02} available", items.len()),
                Style::default().fg(theme().dim),
            ),
        ])),
        Rect {
            y: rows[5].y + 1,
            height: 1,
            ..rows[5]
        },
    );
    render_menu(frame, rows[6], app, &items, false);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "─".repeat(rows[8].width as usize),
                Style::default().fg(theme().dim),
            )),
            Line::from(vec![
                Span::styled("j / k", Style::default().fg(theme().gray)),
                Span::styled("  navigate     ", Style::default().fg(theme().dim)),
                Span::styled("enter", Style::default().fg(theme().gray)),
                Span::styled("  select     ", Style::default().fg(theme().dim)),
                Span::styled("?", Style::default().fg(theme().gray)),
                Span::styled("  help     ", Style::default().fg(theme().dim)),
                Span::styled("q", Style::default().fg(theme().gray)),
                Span::styled("  quit", Style::default().fg(theme().dim)),
            ]),
        ]),
        rows[8],
    );
    app.register_hit(
        rows[8].right().saturating_sub(20),
        rows[8].y + 1,
        8,
        1,
        HitAction::Help,
    );
    app.register_hit(
        rows[8].right().saturating_sub(9),
        rows[8].y + 1,
        9,
        1,
        HitAction::Quit,
    );
}

fn menu_copy(action: LandingAction, app: &App) -> (&'static str, &'static str) {
    match action {
        LandingAction::Start if app.demo => ("Start reading", "Demo / no account"),
        LandingAction::Start if app.browser_mode => ("Start reading", "Live / extension"),
        LandingAction::Start => ("Start reading", "Live / X API"),
        LandingAction::SignIn => ("Connect browser extension", "No API credits"),
        LandingAction::Verify => ("Verify sign-in", "Extension handshake"),
        LandingAction::Quit => ("Quit", "Return to shell"),
    }
}

fn render_menu(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    items: &[(String, LandingAction)],
    compact: bool,
) {
    let available = area.width as usize;
    let mut lines = Vec::with_capacity(items.len());
    for (index, (_, action)) in items.iter().enumerate() {
        let selected = index == app.landing_selected;
        let hovered = app.hovered == Some(HitAction::Landing(index));
        let prefix = if selected {
            "▌ "
        } else if hovered {
            "› "
        } else {
            "  "
        };
        let number = format!("{:02}  ", index + 1);
        let key = if selected {
            "[ enter ]"
        } else if hovered {
            "[ click ]"
        } else {
            ""
        };
        let (title, detail) = menu_copy(*action, app);
        let fixed = prefix.width() + number.width() + key.width();
        let title = truncate_to_width(title, available.saturating_sub(fixed + 1));
        let detail = if !compact && fixed + title.width() + detail.width() + 5 <= available {
            format!("  /  {detail}")
        } else {
            String::new()
        };
        let gap = available.saturating_sub(
            prefix.width() + number.width() + title.width() + detail.width() + key.width(),
        );
        let title_style = if selected || hovered {
            Style::default().fg(theme().white)
        } else {
            Style::default().fg(theme().gray)
        };
        let key_style = if selected {
            Style::default().fg(theme().background).bg(theme().white)
        } else {
            Style::default()
                .fg(theme().white)
                .bg(theme().surface_raised)
        };
        lines.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(theme().white)),
            Span::styled(number, Style::default().fg(theme().dim)),
            Span::styled(title, title_style),
            Span::styled(detail, Style::default().fg(theme().dim)),
            Span::raw(" ".repeat(gap)),
            Span::styled(key, key_style),
        ]));
        app.register_hit(
            area.x,
            area.y + index as u16,
            area.width,
            1,
            HitAction::Landing(index),
        );
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_compact(frame: &mut Frame, area: Rect, app: &mut App) {
    let items = app.landing_items();
    if app.landing_selected >= items.len() {
        app.landing_selected = items.len().saturating_sub(1);
    }
    let shell = area.inner(ratatui::layout::Margin {
        horizontal: 1.min(area.width / 2),
        vertical: 1.min(area.height / 2),
    });
    let tiny = shell.width < 50 || shell.height < 19;
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(if tiny { 2 } else { 10 }),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(items.len() as u16),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(shell);
    render_header(frame, rows[0], app);

    if tiny {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("x t u i", Style::default().fg(theme().white)))
                    .alignment(Alignment::Center),
                Line::from(Span::styled(
                    "Scroll X / stay in flow",
                    Style::default().fg(theme().dim),
                ))
                .alignment(Alignment::Center),
            ]),
            rows[1],
        );
    } else {
        render_wordmark(
            frame,
            Rect {
                height: (XTUI_WORDMARK.len() + 3) as u16,
                ..rows[1]
            },
            1,
            1,
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Scroll X / stay in flow / zero chrome",
                Style::default().fg(theme().dim),
            )))
            .alignment(Alignment::Center),
            Rect {
                y: rows[1].y + 9,
                height: 1,
                ..rows[1]
            },
        );
    }

    let (state, identity) = state(app);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                truncate_to_width(&format!("System / {state}"), rows[2].width as usize),
                Style::default().fg(theme().gray),
            )),
            Line::from(Span::styled(
                truncate_to_width(&format!("Identity / {identity}"), rows[2].width as usize),
                Style::default().fg(theme().dim),
            )),
        ]),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Entry points",
            Style::default().fg(theme().dim),
        ))),
        rows[3],
    );
    render_menu(frame, rows[4], app, &items, true);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "j/k move  ·  enter select  ·  q quit",
            Style::default().fg(theme().gray),
        )))
        .alignment(Alignment::Center),
        rows[6],
    );
    app.register_hit(
        rows[6].right().saturating_sub(8),
        rows[6].y,
        8,
        1,
        HitAction::Quit,
    );
}
