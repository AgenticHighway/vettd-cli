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
/// `None` means no day is complete, so there is no normal to compare with.
pub fn daily(all: &Data, count: bool) -> (Vec<Day>, Option<u32>) {
    let days: Vec<Day> = all
        .days
        .iter()
        .map(|&day| {
            let sessions: Vec<_> = all.iter().filter(|s| s.day == day).collect();
            Day {
                day,
                value: (!sessions.is_empty()).then(|| total(sessions.iter().copied(), count)),
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
    let normal = values.get(values.len() / 2).copied();
    (days, normal)
}

/// The chart's floor: bars are drawn above this, so ordinary days still
/// differ. It is the lowest day less the gap between it and the peak, rounded
/// down to whole dollars (or whole requests). Zero means no floor is drawn:
/// there is no data, all days match, or the peak is under twice the lowest.
pub fn baseline(days: &[Day], count: bool) -> u32 {
    let values = days.iter().filter_map(|d| d.value);
    let (Some(min), Some(peak)) = (values.clone().min(), values.max()) else {
        return 0;
    };
    if min == peak || min.saturating_mul(2) < peak {
        return 0;
    }
    let unit = if count { 1 } else { 100 };
    let floor = min.saturating_sub(peak - min);
    floor / unit * unit
}

/// u64: a few thousand sessions already pass 3B tokens, near u32's limit.
pub fn total_tokens(all: &[Session]) -> u64 {
    all.iter()
        .flat_map(|s| &s.requests)
        .flat_map(|r| r.tokens)
        .map(u64::from)
        .sum()
}

/// At most six characters below 1,000B, so it fits Home's token column.
pub fn compact(tokens: u64) -> String {
    // 999.95M would round up to a seven-character "1000.0M".
    if tokens >= 999_950_000 {
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
    let with_data = || days.iter().filter(|d| d.value.is_some());
    let all_partial = with_data().all(|d| d.partial) && with_data().next().is_some();
    // Bars start above the floor, so small differences between days show.
    let base = baseline(&days, cx.count);
    // No complete day means no normal: say so rather than show a zero.
    let unit = if cx.count {
        ("DAILY REQUESTS", "REQUESTS")
    } else {
        ("DAILY SPEND", "SPEND")
    };
    let normal_part: Part = normal.map_or(
        ("normal: no complete day".into(), "no normal".into()),
        |n| (format!("normal ≈ {}", label(n)), format!("≈ {}", label(n))),
    );
    let partial_part: Option<Part> = if all_partial {
        Some(("every day partial (≥)".into(), "all partial ≥".into()))
    } else if days.iter().any(|d| d.partial) {
        Some(("≥ partial".into(), "≥ partial".into()))
    } else {
        None
    };
    let axis_part: Option<Part> = (base > 0).then(|| {
        let amount = if cx.count {
            base.to_string()
        } else {
            format!("${}", base / 100)
        };
        (format!("axis from {amount}"), format!("from {amount}"))
    });
    let unit_part: Part = (unit.0.into(), unit.1.into());
    let title = chart_title(
        &unit_part,
        &normal_part,
        partial_part.as_ref(),
        axis_part.as_ref(),
        usize::from(chart.width),
    );
    // The title doesn't wrap. When every day is partial, the bars drop their
    // own ≥ only while the title carries the full note; with the short note
    // or none, every bar keeps its ≥, so no floor reads as an exact total.
    let noted = all_partial && title.contains("every day partial");
    let gap = 1u16;
    let bar_width = (chart.width.saturating_sub(2) / days.len().max(1) as u16)
        .saturating_sub(gap)
        .clamp(3, 9);
    // A caption wider than its bar is cut, dropping digits. Fall back to
    // whole dollars (rounded down, so a floor stays a floor), then to the
    // day alone.
    let fits = |text: &String| text.chars().count() <= usize::from(bar_width);
    let bars: Vec<Bar> = days
        .iter()
        .map(|d| {
            let outlier = d
                .value
                .is_some_and(|v| v == peak && normal.is_some_and(|n| v > n * 3));
            // No-data days show "·", never a zero-height bar that reads as $0.
            // Partial days show "≥": the total is a floor, not the day's cost.
            // When every day is partial and the title fits, it says so once.
            let floor = if d.partial && !noted { "≥" } else { "" };
            // The day of the month: the chart's seven bars are consecutive.
            let label_day = day_label(d.day);
            let dom = label_day.rsplit(' ').next().unwrap_or_default();
            let caption = match d.value {
                None => format!("{dom} ·"),
                Some(v) if wide => {
                    let full = format!("{dom} {floor}{}", label(v));
                    let whole = format!("{dom} {floor}${}", v / 100);
                    if fits(&full) {
                        full
                    } else if !cx.count && fits(&whole) {
                        whole
                    } else {
                        format!("{dom}{floor}")
                    }
                }
                Some(_) => format!("{dom}{floor}"),
            };
            Bar::default()
                .value(u64::from(d.value.map_or(0, |v| v.saturating_sub(base))))
                .text_value(String::new())
                .label(Line::from(caption))
                .style(cx.theme.fg(if outlier { AMBER } else { CYAN }))
        })
        .collect();
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

/// One title part: its full wording and its short wording.
type Part = (String, String);

/// Builds the chart title from parts, each in full or short wording. Full
/// wording is used while it fits the block. Then the unit and the axis
/// shorten first, then the normal and partial notes, so the figures keep
/// their full wording as long as the title allows. If every part is short
/// and it still does not fit, the partial note and then the normal are
/// dropped. The unit and the axis are never dropped: a raised axis must
/// always be named. No word is ever cut. A block title is drawn as
/// " {title} ", so it needs two cells more than its text.
fn chart_title(
    unit: &Part,
    normal: &Part,
    partial: Option<&Part>,
    axis: Option<&Part>,
    width: usize,
) -> String {
    let fits = |text: &str| cells(text) + 2 <= width;
    // Shown in this order; `true` marks a part that may be dropped.
    let parts: Vec<(&Part, bool)> = [Some((unit, false)), Some((normal, true))]
        .into_iter()
        .chain([partial.map(|p| (p, true)), axis.map(|a| (a, false))])
        .flatten()
        .collect();
    let render = |short: &[bool], kept: &[bool]| -> String {
        let texts: Vec<&str> = parts
            .iter()
            .zip(short)
            .zip(kept)
            .filter(|(_, &kept)| kept)
            .map(|(((part, _), &is_short), _)| {
                if is_short {
                    part.1.as_str()
                } else {
                    part.0.as_str()
                }
            })
            .collect();
        let all_short = short.iter().all(|&is_short| is_short);
        texts.join(if all_short { " · " } else { "  ·  " })
    };
    let last = parts.len() - 1;
    let mut order = vec![0];
    if axis.is_some() {
        order.push(last);
    }
    order.extend(1..parts.len() - usize::from(axis.is_some()));
    let mut short = vec![false; parts.len()];
    let mut kept = vec![true; parts.len()];
    let full = render(&short, &kept);
    if fits(&full) {
        return full;
    }
    for &i in &order {
        short[i] = true;
        let text = render(&short, &kept);
        if fits(&text) {
            return text;
        }
    }
    // Drop optional parts from the end: the partial note, then the normal.
    for i in (0..parts.len()).rev().filter(|&i| parts[i].1) {
        kept[i] = false;
        let text = render(&short, &kept);
        if fits(&text) {
            return text;
        }
    }
    render(&short, &kept)
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
