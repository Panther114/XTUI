use super::*;
use crate::app::LandingAction;

/// XTUI's original block wordmark, retained as the center of the landing
/// experience. Motion is layered around the mark instead of replacing it, so
/// the identity remains recognizable in every frame and with motion disabled.
const XTUI_WORDMARK: &[&str] = &[
    "██╗  ██╗████████╗██╗   ██╗██╗",
    "╚██╗██╔╝╚══██╔══╝██║   ██║██║",
    " ╚███╔╝    ██║   ██║   ██║██║",
    " ██╔██╗    ██║   ██║   ██║██║",
    "██╔╝ ██╗   ██║   ╚██████╔╝██║",
    "╚═╝  ╚═╝   ╚═╝    ╚═════╝ ╚═╝",
];

pub(super) fn motion_frame() -> u64 {
    motion::frame(12.0)
}

fn gray(luma: u8) -> Color {
    Color::Rgb(luma, luma, luma)
}

fn render_wordmark(frame: &mut Frame, area: Rect, scale_x: usize, scale_y: usize) {
    let phase = motion::phase();
    let art_rows = XTUI_WORDMARK.len() * scale_y;
    let art_width = XTUI_WORDMARK
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(1)
        * scale_x;
    let canvas_width = art_width + 11;
    let canvas_height = art_rows + 3;
    let mut canvas = vec![vec![(' ', 0u8, false); canvas_width]; canvas_height];

    // Technical echo layers. Their glyph changes slowly, creating apparent
    // depth even in terminals that cannot display intermediate color values.
    let echo_glyphs = [('⠈', 42u8), ('░', 60u8), ('▒', 78u8)];
    let echo_offsets = [(9usize, 3usize), (6usize, 2usize), (3usize, 1usize)];
    let echo_shift = (motion::frame(2.0) % 2) as usize;
    for ((glyph, luma), (offset_x, offset_y)) in echo_glyphs.into_iter().zip(echo_offsets) {
        for (row, text) in XTUI_WORDMARK.iter().enumerate() {
            for (column, character) in text.chars().enumerate() {
                if character.is_whitespace() {
                    continue;
                }
                for dy in 0..scale_y {
                    for dx in 0..scale_x {
                        let x = column * scale_x + dx + offset_x + echo_shift;
                        let y = row * scale_y + dy + offset_y;
                        if y < canvas_height && x < canvas_width {
                            canvas[y][x] = (glyph, luma, false);
                        }
                    }
                }
            }
        }
    }

    let cycle = (phase % 6.2) / 6.2;
    // The sweep travels for most of the cycle, then leaves a short quiet beat.
    let beam = if cycle < 0.78 {
        -0.2 + cycle / 0.78 * 1.4
    } else {
        -0.2
    };
    let scan_row = motion::sweep(art_rows.max(1), 3.4, 0.65);
    let breathing = motion::pulse(4.8, 0.0);
    let glitch_active = motion::enabled() && phase.rem_euclid(7.4) > 7.08;
    let base_luma = [226u8, 205, 184, 164, 142, 122];
    let rows = art_rows as f32;
    let columns = art_width as f32;

    for (row, text) in XTUI_WORDMARK.iter().enumerate() {
        let row_shift: i32 = if glitch_active {
            match row % 4 {
                0 => 2,
                2 => -1,
                _ => 0,
            }
        } else {
            0
        };
        for (column, mut character) in text.chars().enumerate() {
            if character.is_whitespace() {
                continue;
            }
            for dy in 0..scale_y {
                for dx in 0..scale_x {
                    let scaled_row = row * scale_y + dy;
                    let unshifted_column = column * scale_x + dx;
                    let diagonal = (unshifted_column as f32 + (rows - scaled_row as f32))
                        / (columns + rows).max(1.0);
                    let distance = (diagonal - beam).abs();
                    let shine = if distance < 0.2 {
                        0.5 * (1.0 + (std::f32::consts::PI * distance / 0.2).cos())
                    } else {
                        0.0
                    };
                    let scan_distance = scaled_row.abs_diff(scan_row);
                    let scan = match scan_distance {
                        0 => 0.42,
                        1 => 0.16,
                        _ => 0.0,
                    };
                    let base = base_luma[row.min(base_luma.len() - 1)];
                    let mut amount = (shine * 0.76 + scan + breathing * 0.05).min(1.0);
                    if glitch_active && (scaled_row * 17 + unshifted_column * 7).is_multiple_of(19)
                    {
                        character = if character == '█' { '▓' } else { '█' };
                        amount = 1.0;
                    }
                    let luma = motion::luma(base, 255, amount);
                    let x = (unshifted_column as i32 + 1 + row_shift).max(0) as usize;
                    if scaled_row < canvas_height && x < canvas_width {
                        canvas[scaled_row][x] = (character, luma, true);
                    }
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
                        let mut style = Style::default().fg(if previous == 0 {
                            theme().background
                        } else {
                            gray(previous)
                        });
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
                    gray(previous)
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

fn user_id(app: &App) -> String {
    app.me
        .as_ref()
        .map(|user| format!("@{}", user.username))
        .unwrap_or_default()
}

fn version_line() -> String {
    format!("xtui version {}", env!("CARGO_PKG_VERSION"))
}

fn menu_title(action: LandingAction) -> &'static str {
    match action {
        LandingAction::Start => "Start",
        LandingAction::SignIn => "Connect browser extension",
        LandingAction::Verify => "Verify sign-in",
        LandingAction::Quit => "Quit",
    }
}

pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let items = app.landing_items();
    if app.landing_selected >= items.len() {
        app.landing_selected = items.len().saturating_sub(1);
    }

    let content = paint_frame(frame, area).unwrap_or(area);
    let tiny = content.width < 50 || content.height < 19;
    let logo_height = if tiny { 1 } else { 9 };
    let tagline_height = if tiny { 1 } else { 2 };
    let menu_gap = if tiny { 1 } else { 2 };
    let rows = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(logo_height),
        Constraint::Length(1),
        Constraint::Length(tagline_height),
        Constraint::Length(1),
        Constraint::Length(menu_gap),
        Constraint::Length(items.len().max(1) as u16),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(content);

    if tiny {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "xtui",
                Style::default()
                    .fg(theme().white)
                    .add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Center),
            rows[1],
        );
    } else {
        render_wordmark(frame, rows[1], 1, 1);
    }
    render_taglines(frame, rows[3]);
    render_user_id(frame, rows[4], app);
    render_menu(frame, rows[6], app, &items);
    render_footer(frame, rows[8], app);
}

/// Inset rounded crop around the landing page only. Outer margin keeps the
/// rule off the terminal edge; inner padding keeps copy off the rule.
fn paint_frame(frame: &mut Frame, area: Rect) -> Option<Rect> {
    if area.width < 24 || area.height < 14 {
        return None;
    }
    let framed = area.inner(ratatui::layout::Margin {
        horizontal: 3,
        vertical: 2,
    });
    if framed.width < 18 || framed.height < 10 {
        return None;
    }
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme().dim)),
        framed,
    );
    Some(framed.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    }))
}

fn render_taglines(frame: &mut Frame, area: Rect) {
    if area.height == 0 {
        return;
    }
    let mut lines = vec![Line::from(Span::styled(
        "Browse X in your Terminal.",
        Style::default()
            .fg(theme().white)
            .add_modifier(Modifier::BOLD),
    ))
    .alignment(Alignment::Center)];
    if area.height > 1 {
        lines.push(
            Line::from(Span::styled(
                version_line(),
                Style::default().fg(theme().gray),
            ))
            .alignment(Alignment::Center),
        );
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_user_id(frame: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let handle = user_id(app);
    let mut spans = Vec::new();
    if let Some(spinner) = app.spinner_frame() {
        spans.push(Span::styled(
            spinner.to_string(),
            Style::default().fg(theme().white),
        ));
        if !handle.is_empty() {
            spans.push(Span::raw("  "));
        }
    }
    if !handle.is_empty() {
        spans.push(Span::styled(handle, Style::default().fg(theme().gray)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
}

fn render_menu(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    items: &[(String, LandingAction)],
) {
    if items.is_empty() || area.height == 0 {
        return;
    }

    for (index, (_, action)) in items.iter().enumerate() {
        let y = area.y.saturating_add(index as u16);
        if y >= area.bottom() {
            break;
        }
        let selected = index == app.landing_selected;
        let hovered = app.hovered == Some(HitAction::Landing(index));
        let item_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        let title = menu_title(*action);
        let title_style = if selected {
            Style::default()
                .fg(theme().background)
                .bg(theme().white)
                .add_modifier(Modifier::BOLD)
        } else if hovered {
            Style::default()
                .fg(theme().white)
                .bg(theme().surface_raised)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme().gray)
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {title} "), title_style)))
                .alignment(Alignment::Center),
            item_area,
        );
        app.register_hit(
            item_area.x,
            item_area.y,
            item_area.width,
            item_area.height,
            HitAction::Landing(index),
        );
    }
}

fn render_footer(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height == 0 {
        return;
    }
    let mut spans = vec![
        Span::styled(
            " ↑↓ ",
            Style::default()
                .fg(theme().white)
                .bg(theme().surface_raised),
        ),
        Span::styled(" move   ", Style::default().fg(theme().dim)),
        Span::styled(
            " → ",
            Style::default().fg(theme().background).bg(theme().white),
        ),
        Span::styled(" start   ", Style::default().fg(theme().dim)),
        Span::styled(
            " ← ",
            Style::default()
                .fg(theme().white)
                .bg(theme().surface_raised),
        ),
        Span::styled(" back", Style::default().fg(theme().dim)),
    ];
    if area.width >= 52 {
        spans.extend([
            Span::styled("   ", Style::default()),
            Span::styled(
                " enter ",
                Style::default()
                    .fg(theme().white)
                    .bg(theme().surface_raised),
            ),
            Span::styled(" select", Style::default().fg(theme().dim)),
        ]);
    }
    if area.width >= 68 {
        spans.extend([
            Span::styled("   ", Style::default()),
            Span::styled(
                " ? ",
                Style::default()
                    .fg(theme().white)
                    .bg(theme().surface_raised),
            ),
            Span::styled(" help   ", Style::default().fg(theme().dim)),
            Span::styled(
                " q ",
                Style::default()
                    .fg(theme().white)
                    .bg(theme().surface_raised),
            ),
            Span::styled(" quit", Style::default().fg(theme().dim)),
        ]);
    }
    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
    let hint_width = 48u16.min(area.width);
    let hint_x = area.x + (area.width.saturating_sub(hint_width)) / 2;
    app.register_hit(
        hint_x.saturating_add(hint_width.saturating_sub(18)),
        area.y,
        8,
        1,
        HitAction::Help,
    );
    app.register_hit(
        hint_x.saturating_add(hint_width.saturating_sub(9)),
        area.y,
        9,
        1,
        HitAction::Quit,
    );
}
