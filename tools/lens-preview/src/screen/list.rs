//! Every screen except Home: a column header, the rows, and a preview pane
//! beside them when there is room.

use super::preview::preview;
use super::*;

fn header(view: View, count: bool) -> &'static str {
    match view {
        View::Home => "",
        View::Headroom => "WINDOW         USED  AT RESET  RESETS       SOURCE",
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

pub(super) fn draw_list(f: &mut Frame, body: Rect, page: &Page, cx: &Ctx) {
    // Record views carry exact locators and results, so they keep full width.
    let pane = cx.active.is_some()
        && body.width >= 76
        && body.height >= 9
        && !matches!(
            page.view,
            View::Change(..) | View::Placement(_) | View::Observation(..) | View::Headroom
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
        Paragraph::new(format!("   {}", header(page.view, cx.count)))
            .style(cx.theme.panel().add_modifier(Modifier::BOLD)),
        head,
    );
    if cx.rows.is_empty() {
        let message = match page.view {
            View::Find => "No matches. Edit search.",
            View::Inventory => "No components match these filters. x clears.",
            _ => "No session input.",
        };
        f.render_widget(
            Paragraph::new(format!("  {message}")).style(cx.theme.fg(AMBER)),
            items,
        );
    }
    f.render_widget(Paragraph::new(lines(page, items, cx)), items);
    if let (true, Some((_, (next, _)))) = (pane, cx.active) {
        draw_pane(f, side, next, cx);
    }
}

/// The rows that fit from the scroll position, the selected one marked.
fn lines(page: &Page, items: Rect, cx: &Ctx) -> Vec<Line<'static>> {
    cx.rows
        .iter()
        .enumerate()
        .skip(page.top)
        .take(usize::from(items.height))
        .map(|(i, row)| {
            let selected = cx.active.is_some_and(|(selection, _)| i == selection);
            let marker = if selected { " ▸ " } else { "   " };
            let style = if selected {
                cx.theme.selected()
            } else {
                row_style(&row.text, cx.theme)
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
        .collect()
}

fn draw_pane(f: &mut Frame, side: Rect, next: View, cx: &Ctx) {
    let (heading, detail) = preview(next, cx.all, cx.count, side.width, cx.rt);
    let theme = cx.theme;
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
