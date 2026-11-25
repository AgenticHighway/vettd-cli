//! Home, the one dashboard: a daily chart, where tokens went, and the few
//! findings worth opening. Only the findings take the selection.

use super::*;
use ratatui::widgets::{Bar, BarChart, BarGroup, Cell, Padding, Row as TRow, Table};

/// One day on the chart. `value` is None when there was no input at all;
/// `partial` means some input is incomplete or, for spend, unpriced, so the
/// value is only a floor.
pub struct Day {
    pub day: u32,
    pub value: Option<u32>,
    pub partial: bool,
}

/// Each day's total in the current unit, and the median complete day.
pub fn daily(all: &Data, count: bool) -> (Vec<Day>, u32) {
    let days: Vec<Day> = all
        .days
        .iter()
        .map(|&day| {
            let sessions: Vec<_> = all.iter().filter(|s| s.day == day).collect();
            Day {
                day,
                value: (!sessions.is_empty())
                    .then(|| sessions.iter().map(|s| measure(s, count)).sum()),
                partial: sessions.iter().any(|s| !comparable(s, count)),
            }
        })
        .collect();
    // A partial day's total is a floor, so it never sets what "normal" means.
    let mut values: Vec<u32> = days
        .iter()
        .filter(|d| !d.partial)
        .filter_map(|d| d.value)
        .collect();
    values.sort_unstable();
    let normal = values.get(values.len() / 2).copied().unwrap_or(0);
    (days, normal)
}

pub fn total_tokens(all: &[Session]) -> u64 {
    all.iter()
        .flat_map(|s| &s.requests)
        .flat_map(|r| r.tokens)
        .map(u64::from)
        .sum()
}

pub fn compact(tokens: u64) -> String {
    if tokens >= 1_000_000_000 {
        format!("{:.1}B", tokens as f64 / 1e9)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1e6)
    } else {
        format!("{}k", tokens.div_ceil(1000))
    }
}

pub(super) fn draw_home(f: &mut Frame, body: Rect, cx: &Ctx) {
    let wide = body.width >= 76;
    let (chart_h, sources_h) = home_heights(body.height, cx.rows.len() as u16, wide);
    let (chart, sources, findings) = if wide {
        let [chart, rest] =
            Layout::vertical([Constraint::Length(chart_h), Constraint::Fill(1)]).areas(body);
        let [a, b] = Layout::horizontal([Constraint::Length(42), Constraint::Fill(1)])
            .spacing(1)
            .areas(rest);
        (chart, a, b)
    } else {
        let [chart, a, b] = Layout::vertical([
            Constraint::Length(chart_h),
            Constraint::Length(sources_h),
            Constraint::Fill(1),
        ])
        .areas(body);
        (chart, a, b)
    };
    draw_chart(f, chart, wide, cx);
    draw_sources(f, sources, cx);
    draw_findings(f, findings, cx);
}

fn draw_chart(f: &mut Frame, chart: Rect, wide: bool, cx: &Ctx) {
    let (days, normal) = daily(cx.all, cx.count);
    let peak = days.iter().filter_map(|d| d.value).max().unwrap_or(0);
    let label = |v: u32| if cx.count { v.to_string() } else { usd(v) };
    let bars: Vec<Bar> = days
        .iter()
        .map(|d| {
            let outlier = d.value.is_some_and(|v| v == peak && v > normal * 3);
            // No-data days show "·", never a zero-height bar that reads as $0.
            // Partial days show "≥": the total is a floor, not the day's cost.
            let floor = if d.partial { "≥" } else { "" };
            // The day of the month: the chart's seven bars are consecutive.
            let label_day = day_label(d.day);
            let dom = label_day.rsplit(' ').next().unwrap_or_default();
            let caption = match d.value {
                None => format!("{dom} ·"),
                Some(v) if wide => format!("{dom} {floor}{}", label(v)),
                Some(_) => format!("{dom}{floor}"),
            };
            Bar::default()
                .value(u64::from(d.value.unwrap_or(0)))
                .text_value(String::new())
                .label(Line::from(caption))
                .style(cx.theme.fg(if outlier { AMBER } else { CYAN }))
        })
        .collect();
    let gap = 1u16;
    let bar_width = (chart.width.saturating_sub(2) / days.len().max(1) as u16)
        .saturating_sub(gap)
        .clamp(3, 9);
    let title = format!(
        "DAILY {}  ·  normal ≈ {}{}",
        if cx.count { "REQUESTS" } else { "SPEND" },
        label(normal),
        if days.iter().any(|d| d.partial) {
            "  ·  ≥ partial"
        } else {
            ""
        }
    );
    f.render_widget(
        BarChart::default()
            .block(panel_block(&title, cx.theme))
            .data(BarGroup::default().bars(&bars))
            .bar_width(bar_width)
            .bar_gap(gap)
            .label_style(cx.theme.fg(MUTED)),
        chart,
    );
}

fn draw_sources(f: &mut Frame, sources: Rect, cx: &Ctx) {
    let theme = cx.theme;
    let total = total_tokens(cx.all);
    // Local source names are longer ("Instructions and files").
    let name_width: u16 = if cx.all.local() { 22 } else { 19 };
    let bar_cells = usize::from(sources.width.saturating_sub(17 + name_width)).clamp(4, 14);
    // Local data has more sources than the table has rows: the largest
    // six are shown, and Token detail and the export hold the rest.
    let mut shares: Vec<_> = cx.all.token_sources.iter().collect();
    if cx.all.local() {
        shares.sort_by_key(|s| std::cmp::Reverse(s.basis_points));
        shares.truncate(6);
    }
    let table_rows: Vec<TRow> = shares
        .into_iter()
        .map(|share| {
            let (name, bp) = (share.name, share.basis_points);
            let filled = (bp as usize * bar_cells).div_ceil(10_000);
            let (amount, share) = if bp == 0 {
                ("0".to_string(), "—".to_string())
            } else {
                let tokens = share.tokens.unwrap_or(total / 10_000 * u64::from(bp));
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
    let widths = [
        Constraint::Length(name_width),
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Fill(1),
    ];
    f.render_widget(
        Table::new(table_rows, widths)
            .header(TRow::new(["SOURCE", "TOKENS", "SHARE", ""]).style(theme.fg(MUTED)))
            .column_spacing(1)
            .block(panel_block(
                if cx.all.local() {
                    "WHERE TOKENS WENT (ESTIMATED)"
                } else {
                    "WHERE TOKENS WENT"
                },
                theme,
            )),
        sources,
    );
}

/// The findings, scrolled so the selected one is always drawn.
fn draw_findings(f: &mut Frame, findings: Rect, cx: &Ctx) {
    let room = usize::from(findings.height.saturating_sub(1)).max(1);
    let selected = cx.active.map_or(0, |(i, _)| i);
    let top = (selected + 1).saturating_sub(room);
    let lines: Vec<Line> = cx
        .rows
        .iter()
        .enumerate()
        .skip(top)
        .take(room)
        .map(|(i, row)| {
            let selected = cx.active.is_some_and(|(selection, _)| i == selection);
            Line::styled(
                format!(
                    "{}{:<width$}",
                    if selected { "▸ " } else { "  " },
                    row.text,
                    width = usize::from(findings.width.saturating_sub(4))
                ),
                if selected {
                    cx.theme.selected()
                } else {
                    cx.theme.fg(INK)
                },
            )
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(panel_block("WORTH A LOOK", cx.theme)),
        findings,
    );
}

/// Splits Home's body so the findings, which take the selection, always get
/// their rows first. The chart and token table share what is left: a small
/// chart first, then the full table, then a taller chart. Returns the chart
/// height and, when stacked, the token table height.
pub fn home_heights(body: u16, findings: u16, wide: bool) -> (u16, u16) {
    const CHART_MIN: u16 = 4;
    const CHART_MAX: u16 = 8;
    const TABLE: u16 = 8; // title, header and six sources
    let findings = (findings + 1).min(body);
    let left = body - findings;
    if wide {
        // Findings sit beside the table, so the table can use their rows.
        let rest = findings.max(TABLE.min(left.saturating_sub(CHART_MIN)));
        let chart = body.saturating_sub(rest).min(CHART_MAX);
        let chart = if chart < CHART_MIN { 0 } else { chart };
        return (chart, 0);
    }
    let chart_min = if left >= CHART_MIN { CHART_MIN } else { 0 };
    let table = TABLE.min(left - chart_min);
    let chart = (left - table).min(CHART_MAX - 2);
    (if chart < CHART_MIN { 0 } else { chart }, table)
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
