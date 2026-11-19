use super::{
    comparable, content, cost_label, measure, units, usd, Page, Row, Runtime, Session, View,
    COMPONENTS, PARTS,
};
use crate::fixture::SOURCE_SHARES;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Bar, BarChart, BarGroup, Block, Borders, Cell, Padding, Paragraph, Row as TRow, Table, Wrap,
    },
    Frame,
};

const INK: Color = Color::Rgb(231, 239, 243);
const MUTED: Color = Color::Rgb(158, 179, 195);
const CYAN: Color = Color::Rgb(104, 228, 212);
const AMBER: Color = Color::Rgb(248, 194, 128);
const BASE: Color = Color::Rgb(13, 22, 32);
const PANEL: Color = Color::Rgb(22, 37, 49);
const ACTIVE: Color = Color::Rgb(35, 77, 83);

/// Header (bar, breadcrumb, answer, scope chips) plus the column header row.
const HEAD: u16 = 5;
/// Enter hint and key bar.
const FOOT: u16 = 2;

pub fn list_width(w: u16) -> u16 {
    if w >= 76 {
        w * 3 / 5
    } else {
        w.saturating_sub(1)
    }
}

pub fn too_small(w: u16, h: u16) -> bool {
    w < 38 || h < 10
}

/// NO_COLOR set to any non-empty value turns colour off.
pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}

pub fn visible(h: u16) -> usize {
    h.saturating_sub(HEAD + FOOT).max(1).into()
}

/// Colours, or none at all under NO_COLOR, where selection falls back to
/// reverse video so focus is still visible.
#[derive(Clone, Copy)]
struct Theme {
    mono: bool,
}

impl Theme {
    fn fg(self, color: Color) -> Style {
        if self.mono {
            Style::default()
        } else {
            Style::default().fg(color)
        }
    }

    fn on(self, fg: Color, bg: Color) -> Style {
        if self.mono {
            Style::default()
        } else {
            Style::default().fg(fg).bg(bg)
        }
    }

    fn base(self) -> Style {
        self.on(INK, BASE)
    }

    fn panel(self) -> Style {
        self.on(CYAN, PANEL)
    }

    fn selected(self) -> Style {
        if self.mono {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(Color::White).bg(ACTIVE)
        }
    }

    fn key(self) -> Style {
        self.fg(CYAN).add_modifier(Modifier::BOLD)
    }
}

fn crumb(view: View, all: &[Session]) -> String {
    match view {
        View::Home => "Home".into(),
        View::Week => "Daily activity".into(),
        View::Find => "Lookup".into(),
        View::Day(day) => format!("Nov {day}"),
        View::Session(id) => all[id].name.into(),
        View::Tokens(_) => "Tokens".into(),
        View::Help => "Keys".into(),
        View::Inventory => "Components".into(),
        View::Component(c, s) => format!(
            "{} ({})",
            COMPONENTS[c].name,
            if s.from == s.to {
                format!("Nov {}", s.from)
            } else {
                "week".into()
            }
        ),
        View::Placement(_) => "Copy".into(),
        View::Observation(_, _) => "Observation".into(),
        View::Signal(_, name, _) => name.into(),
        View::Change(_, _) | View::Apply(_, _) => "Change preview".into(),
    }
}

fn preview(
    view: View,
    all: &[Session],
    count: bool,
    width: u16,
    rt: &Runtime,
) -> (String, Vec<String>) {
    let (title, scope, rows) = content(&Page::new(view), all, count, width, rt);
    match view {
        View::Day(day) => {
            let current: Vec<_> = all.iter().filter(|s| s.day == day).collect();
            let prior: Vec<_> = all.iter().filter(|s| s.day + 1 == day).collect();
            let total: u32 = current.iter().map(|s| measure(s, count)).sum();
            let mut preview = vec![
                if count {
                    format!("{total} requests")
                } else if current.iter().all(|s| super::priced(s) == 0) {
                    "No priced requests".into()
                } else {
                    format!("{} priced USD", usd(total))
                },
                format!("{} sessions", current.len()),
            ];
            if current.is_empty() {
                preview[0] = "No session input".into();
            } else if prior.is_empty()
                || current.iter().chain(&prior).any(|s| !comparable(s, count))
            {
                preview.push("No comparable prior day".into());
                if current.iter().any(|s| s.incomplete) {
                    preview.push("Partial / unpriced input".into());
                }
            } else if count {
                let previous: u32 = prior.iter().map(|s| measure(s, true)).sum();
                preview.push(format!(
                    "{:+} req vs Nov {}",
                    i64::from(total) - i64::from(previous),
                    day - 1
                ));
            } else {
                let sum = |sessions: &[&Session], i: usize| -> u32 {
                    sessions
                        .iter()
                        .flat_map(|s| &s.requests)
                        .map(|r| r.cents()[i])
                        .sum()
                };
                let previous: u32 = prior.iter().map(|s| s.cents()).sum();
                let delta = |a: u32, b: u32| {
                    format!("{}{}", if a >= b { "+" } else { "−" }, usd(a.abs_diff(b)))
                };
                preview.extend([
                    String::new(),
                    format!("{} vs Nov {}", delta(total, previous), day - 1),
                ]);
                preview.extend(PARTS.iter().enumerate().map(|(i, part)| {
                    format!("{part:<13} {:>8}", delta(sum(&current, i), sum(&prior, i)))
                }));
            }
            if let Some(lead) = current
                .iter()
                .filter(|s| count || super::priced(s) != 0)
                .max_by_key(|s| measure(s, count))
            {
                preview.extend([
                    String::new(),
                    if count {
                        "Most requests"
                    } else {
                        "Largest priced session"
                    }
                    .into(),
                    format!("{}  {}", lead.name, units(measure(lead, count), count)),
                ]);
            }
            (format!("Nov {day} | UTC"), preview)
        }
        View::Session(id) => {
            let session = &all[id];
            let mut detail = vec![
                format!(
                    "{} | {} | Nov {}",
                    session.agent, session.workspace, session.day
                ),
                cost_label(session),
                String::new(),
            ];
            detail.extend(
                rows.into_iter()
                    .filter(|row| row.open.is_none() && !row.text.starts_with("──"))
                    .map(|row| row.text),
            );
            (all[id].name.into(), detail)
        }
        View::Tokens(_) => (
            crumb(view, all),
            rows.into_iter()
                .map(|row| row.text.replace(" tokens", " tok"))
                .collect(),
        ),
        View::Component(..) | View::Placement(_) | View::Change(..) => {
            let mut detail = vec![scope];
            detail.extend(rows.into_iter().map(|row| row.text));
            (title, detail)
        }
        _ => (
            crumb(view, all),
            rows.into_iter().map(|row| row.text).collect(),
        ),
    }
}

/// Keeps Home and the last levels that fit, so the current place is never cut.
fn breadcrumb(stack: &[Page], all: &[Session], max: usize) -> String {
    let parts: Vec<_> = stack.iter().map(|p| crumb(p.view, all)).collect();
    let full = parts.join(" › ");
    if full.chars().count() <= max {
        return full;
    }
    let mut tail = parts.len() - 1;
    while tail > 1 && parts[tail - 1..].join(" › ").chars().count() + parts[0].len() + 5 <= max {
        tail -= 1;
    }
    format!("{} › … › {}", parts[0], parts[tail..].join(" › "))
}

/// Scope parts become chips, so workspace, agent and period read as values.
/// Chips that don't fit are replaced by "…" instead of being cut mid-word.
fn chips(scope: &str, width: usize, theme: Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    let parts: Vec<_> = scope.split(" | ").filter(|p| !p.is_empty()).collect();
    for (i, part) in parts.iter().enumerate() {
        let chip = if theme.mono {
            format!("[{part}]")
        } else {
            format!(" {part} ")
        };
        let more = if i + 1 < parts.len() { 2 } else { 0 };
        if used + chip.chars().count() + more > width {
            spans.push(Span::styled("…", theme.fg(MUTED)));
            break;
        }
        used += chip.chars().count() + 1;
        spans.push(Span::styled(chip, theme.on(INK, PANEL)));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

fn keys(pairs: &[(&str, &str)], theme: Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in pairs {
        spans.push(Span::styled(key.to_string(), theme.key()));
        spans.push(Span::styled(format!(" {label}   "), theme.fg(INK)));
    }
    Line::from(spans)
}

fn header(view: View, count: bool) -> &'static str {
    match view {
        View::Home => "",
        View::Week if count => "DAY       REQUESTS  DAILY VOLUME",
        View::Week => "DAY      PRICED USD DAILY COST",
        View::Day(_) => "SESSION          WS   AGENT      TOTAL",
        View::Find => "MATCHES",
        View::Session(_) => "COST, WORKERS AND OBSERVED COMPONENTS",
        View::Tokens(_) => "TOKENS BY WORKER",
        View::Help => "KEY     ACTION",
        View::Inventory => "TYPE    COMPONENT       #  STATE",
        View::Component(..) => "USE, COPIES, OBSERVATIONS, SIGNALS",
        View::Placement(_) => "EXACT COPY AND RELATIONSHIPS",
        View::Observation(..) => "RECORDED OBSERVATION",
        View::Signal(..) => "MATCHING OBSERVATIONS",
        View::Change(..) | View::Apply(..) => "PROPOSED CHANGE",
    }
}

fn row_style(text: &str, theme: Theme) -> Style {
    if text.starts_with("Applied") {
        theme.fg(CYAN)
    } else if text.starts_with("──") {
        theme.fg(MUTED)
    } else if ["partial", "unpriced", "Unknown", "unknown", "None for"]
        .iter()
        .any(|flag| text.contains(flag))
    {
        theme.fg(AMBER)
    } else {
        theme.fg(INK)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    f: &mut Frame,
    stack: &[Page],
    all: &[Session],
    count: bool,
    (title, scope): (&str, &str),
    rows: &[Row],
    active: Option<(usize, (View, &'static str))>,
    rt: &Runtime,
    mono: bool,
) {
    let theme = Theme { mono };
    let area = f.area();
    f.render_widget(Block::default().style(theme.base()), area);
    if too_small(area.width, area.height) {
        let text = vec![
            Line::styled(
                format!(
                    "Lens needs at least 38×10. This is {}×{}.",
                    area.width, area.height
                ),
                theme.fg(AMBER),
            ),
            Line::raw("Your place is kept. Enlarge the window to continue."),
            keys(&[("q", "Quit")], theme),
        ];
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
        return;
    }
    let page = stack.last().unwrap();
    let [bar, path, answer, chip, body, hint, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" LENS ", theme.panel().add_modifier(Modifier::BOLD)),
            Span::styled(
                "  synthetic data · nothing leaves this machine",
                theme.fg(MUTED),
            ),
        ]))
        .style(theme.on(INK, PANEL)),
        bar,
    );
    let max = usize::from(area.width.saturating_sub(2));
    f.render_widget(
        Paragraph::new(format!(" {}", breadcrumb(stack, all, max))).style(theme.fg(MUTED)),
        path,
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ◆ ", theme.fg(CYAN)),
            Span::styled(
                title.to_string(),
                theme.fg(INK).add_modifier(Modifier::BOLD),
            ),
        ])),
        answer,
    );
    f.render_widget(
        Paragraph::new(chips(scope, usize::from(area.width), theme)),
        chip,
    );

    if matches!(page.view, View::Home) {
        draw_home(f, body, all, count, rows, active, theme);
    } else {
        draw_list(f, body, page, all, count, rows, active, rt, theme);
    }

    if let Some((index, (_, action))) = active {
        let n = rows
            .iter()
            .take(index + 1)
            .filter(|row| row.open.is_some())
            .count();
        let total = rows.iter().filter(|row| row.open.is_some()).count();
        let [left, right] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(8)]).areas(hint);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Enter ", theme.key()),
                Span::styled(action.to_string(), theme.fg(CYAN)),
            ])),
            left,
        );
        f.render_widget(
            Paragraph::new(format!("{n}/{total} "))
                .style(theme.fg(MUTED))
                .right_aligned(),
            right,
        );
    }
    f.render_widget(
        Paragraph::new(keys(&footer_keys(page.view, area.width), theme))
            .style(theme.on(INK, PANEL)),
        footer,
    );
}

fn footer_keys(view: View, w: u16) -> Vec<(&'static str, &'static str)> {
    let widen = view.widened().is_some();
    // Only views that follow the selected measure advertise its toggle.
    let measured = matches!(
        view,
        View::Home | View::Week | View::Day(_) | View::Session(_) | View::Tokens(_)
    );
    match view {
        View::Change(..) => vec![("↑↓", "Move"), ("Enter", "Apply"), ("Esc", "Cancel")],
        View::Inventory if w >= 76 => vec![
            ("Tab", "Type"),
            ("w", "Workspace"),
            ("a", "Agent"),
            ("e", "Evidence"),
            ("x", "Clear"),
            ("Esc", "Back"),
        ],
        View::Inventory => vec![
            ("Tab", "Type"),
            ("w", "Wksp"),
            ("a", "Agent"),
            ("x", "Clear"),
        ],
        View::Find => vec![("Type", "to filter"), ("Esc", "Back"), ("Ctrl+C", "Quit")],
        _ if widen => vec![("p", "Widen to week"), ("Esc", "Back"), ("?", "Keys")],
        View::Home if w < 65 => vec![("c", "USD/req"), ("i", "Inventory"), ("?", "Keys")],
        View::Home => vec![
            ("c", "USD/requests"),
            ("i", "Inventory"),
            ("/", "Find"),
            ("?", "Keys"),
            ("q", "Quit"),
        ],
        _ if w < 65 => vec![("i", "Inventory"), ("/", "Find"), ("Esc", "Back")],
        _ if measured => vec![
            ("c", "USD/requests"),
            ("/", "Find"),
            ("Esc", "Back"),
            ("?", "Keys"),
        ],
        _ => vec![("/", "Find"), ("Esc", "Back"), ("?", "Keys"), ("q", "Quit")],
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_list(
    f: &mut Frame,
    body: Rect,
    page: &Page,
    all: &[Session],
    count: bool,
    rows: &[Row],
    active: Option<(usize, (View, &'static str))>,
    rt: &Runtime,
    theme: Theme,
) {
    // Record views carry exact locators and results, so they keep full width.
    let pane = active.is_some()
        && body.width >= 76
        && body.height >= 9
        && !matches!(
            page.view,
            View::Change(..) | View::Placement(_) | View::Observation(..)
        );
    let [list, side] = if pane {
        Layout::horizontal([
            Constraint::Length(list_width(body.width)),
            Constraint::Fill(1),
        ])
        .areas(body)
    } else {
        [body, Rect::default()]
    };
    let [head, items] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(list);
    f.render_widget(
        Paragraph::new(format!("   {}", header(page.view, count)))
            .style(theme.panel().add_modifier(Modifier::BOLD)),
        head,
    );
    if rows.is_empty() {
        let message = match page.view {
            View::Find => "No matches. Edit search.",
            View::Inventory => "No components match these filters. x clears.",
            _ => "No session input.",
        };
        f.render_widget(
            Paragraph::new(format!("  {message}")).style(theme.fg(AMBER)),
            items,
        );
    }
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(page.top)
        .take(usize::from(items.height))
        .map(|(i, row)| {
            let selected = active.is_some_and(|(selection, _)| i == selection);
            let marker = if selected { " ▸ " } else { "   " };
            let style = if selected {
                theme.selected()
            } else {
                row_style(&row.text, theme)
            };
            Line::styled(
                format!(
                    "{marker}{:<width$}",
                    row.text,
                    width = usize::from(items.width)
                ),
                style,
            )
        })
        .collect();
    f.render_widget(Paragraph::new(lines), items);

    if let (true, Some((_, (next, _)))) = (pane, active) {
        let (heading, detail) = preview(next, all, count, side.width, rt);
        let mut text = vec![Line::styled(
            heading,
            theme.fg(INK).add_modifier(Modifier::BOLD),
        )];
        text.extend(detail.into_iter().map(|t| Line::styled(t, theme.fg(MUTED))));
        f.render_widget(
            Paragraph::new(text).wrap(Wrap { trim: false }).block(
                Block::default()
                    .borders(Borders::LEFT)
                    .border_style(theme.fg(ACTIVE))
                    .title(Span::styled(" PREVIEW ", theme.panel()))
                    .style(theme.on(MUTED, PANEL)),
            ),
            side,
        );
    }
}

/// Days with input, their totals in the current unit, and the median day.
pub fn daily(all: &[Session], count: bool) -> (Vec<(u32, Option<u32>)>, u32) {
    let days: Vec<_> = (12..=18)
        .map(|day| {
            let sessions: Vec<_> = all.iter().filter(|s| s.day == day).collect();
            let value =
                (!sessions.is_empty()).then(|| sessions.iter().map(|s| measure(s, count)).sum());
            (day, value)
        })
        .collect();
    let mut values: Vec<u32> = days.iter().filter_map(|d| d.1).collect();
    values.sort_unstable();
    let normal = values.get(values.len() / 2).copied().unwrap_or(0);
    (days, normal)
}

pub fn total_tokens(all: &[Session]) -> u32 {
    all.iter()
        .flat_map(|s| &s.requests)
        .map(|r| r.tokens.iter().sum::<u32>())
        .sum()
}

pub fn compact(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", f64::from(tokens) / 1e6)
    } else {
        format!("{}k", tokens.div_ceil(1000))
    }
}

/// Home is the one dashboard: a daily chart, where tokens went, and the few
/// findings worth opening. Only the findings take the selection.
fn draw_home(
    f: &mut Frame,
    body: Rect,
    all: &[Session],
    count: bool,
    rows: &[Row],
    active: Option<(usize, (View, &'static str))>,
    theme: Theme,
) {
    let wide = body.width >= 76;
    let chart_h = match (wide, body.height >= 16) {
        (true, true) => 8,
        (false, true) => 6,
        _ => 5,
    };
    let [chart, rest] =
        Layout::vertical([Constraint::Length(chart_h), Constraint::Fill(1)]).areas(body);
    let (sources, findings) = if wide {
        let [a, b] = Layout::horizontal([Constraint::Length(42), Constraint::Fill(1)])
            .spacing(1)
            .areas(rest);
        (a, b)
    } else {
        let [a, b] = Layout::vertical([Constraint::Length(8), Constraint::Fill(1)]).areas(rest);
        (a, b)
    };

    let (days, normal) = daily(all, count);
    let peak = days.iter().filter_map(|d| d.1).max().unwrap_or(0);
    let label = |v: u32| if count { v.to_string() } else { usd(v) };
    let bars: Vec<Bar> = days
        .iter()
        .map(|&(day, value)| {
            let outlier = value.is_some_and(|v| v == peak && v > normal * 3);
            // No-data days show "·", never a zero-height bar that reads as $0.
            let caption = match value {
                None => format!("{day} ·"),
                Some(v) if wide => format!("{day} {}", label(v)),
                Some(_) => day.to_string(),
            };
            Bar::default()
                .value(u64::from(value.unwrap_or(0)))
                .text_value(String::new())
                .label(Line::from(caption))
                .style(theme.fg(if outlier { AMBER } else { CYAN }))
        })
        .collect();
    let gap = 1u16;
    let bar_width = (chart.width.saturating_sub(2) / 7)
        .saturating_sub(gap)
        .clamp(3, 9);
    f.render_widget(
        BarChart::default()
            .block(panel_block(
                &format!(
                    "DAILY {}  ·  normal ≈ {}",
                    if count { "REQUESTS" } else { "SPEND" },
                    label(normal)
                ),
                theme,
            ))
            .data(BarGroup::default().bars(&bars))
            .bar_width(bar_width)
            .bar_gap(gap)
            .label_style(theme.fg(MUTED)),
        chart,
    );

    let total = total_tokens(all);
    let bar_cells = usize::from(sources.width.saturating_sub(36)).clamp(4, 14);
    let table_rows: Vec<TRow> = SOURCE_SHARES
        .iter()
        .map(|&(name, bp)| {
            let tokens = total / 10_000 * bp;
            let filled = (bp as usize * bar_cells).div_ceil(10_000);
            let (amount, share) = if bp == 0 {
                ("0".to_string(), "—".to_string())
            } else {
                (compact(tokens), format!("{}%", bp / 100))
            };
            TRow::new(vec![
                Cell::from(name),
                Cell::from(Line::from(amount).right_aligned()),
                Cell::from(Line::from(share).right_aligned()),
                Cell::from(Span::styled("█".repeat(filled), theme.fg(CYAN))),
            ])
            .style(theme.fg(if bp == 0 { MUTED } else { INK }))
        })
        .collect();
    f.render_widget(
        Table::new(
            table_rows,
            [
                Constraint::Length(19),
                Constraint::Length(6),
                Constraint::Length(5),
                Constraint::Fill(1),
            ],
        )
        .header(TRow::new(["SOURCE", "TOKENS", "SHARE", ""]).style(theme.fg(MUTED)))
        .column_spacing(1)
        .block(panel_block("WHERE TOKENS WENT", theme)),
        sources,
    );

    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let selected = active.is_some_and(|(selection, _)| i == selection);
            Line::styled(
                format!(
                    "{}{:<width$}",
                    if selected { "▸ " } else { "  " },
                    row.text,
                    width = usize::from(findings.width.saturating_sub(4))
                ),
                if selected {
                    theme.selected()
                } else {
                    theme.fg(INK)
                },
            )
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel_block("WORTH A LOOK", theme)),
        findings,
    );
}

fn panel_block(title: &str, theme: Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::TOP)
        .padding(Padding::horizontal(1))
        .border_style(theme.fg(ACTIVE))
        .title(Span::styled(
            format!(" {title} "),
            theme.fg(CYAN).add_modifier(Modifier::BOLD),
        ))
}
