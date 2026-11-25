use super::{
    comparable, content, cost_label, measure, priced, units, usd, Page, Row, Runtime, Session,
    View, COMPONENTS, PARTS,
};
use crate::data::{day_label, Data, Source};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

mod home;
mod list;
mod preview;

pub use home::{compact, daily, total_tokens};

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

/// Rows the answer line needs: it wraps onto a second line rather than cut
/// off its numbers, which usually come last.
pub fn answer_rows(w: u16, title: &str) -> u16 {
    if title.chars().count() + 3 > usize::from(w) {
        2
    } else {
        1
    }
}

pub fn visible(h: u16, answer: u16) -> usize {
    h.saturating_sub(HEAD + FOOT + answer - 1).max(1).into()
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

fn crumb(view: View, all: &Data) -> String {
    match view {
        View::Home => "Home".into(),
        View::Headroom => "Plan headroom".into(),
        View::Week => "Daily activity".into(),
        View::Find => "Lookup".into(),
        View::Day(day) => day_label(day),
        View::Session(id) => all[id].name.into(),
        View::Tokens(_) => "Tokens".into(),
        View::Help => "Keys".into(),
        View::Inventory => "Components".into(),
        View::Component(c, s) => format!(
            "{} ({})",
            COMPONENTS[c].name,
            if s.from == s.to {
                day_label(s.from)
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

/// Keeps Home and the last levels that fit, so the current place is never cut.
fn breadcrumb(stack: &[Page], all: &Data, max: usize) -> String {
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

/// What every part of a frame reads.
struct Ctx<'a> {
    all: &'a Data,
    count: bool,
    rows: &'a [Row],
    active: Option<(usize, (View, &'static str))>,
    rt: &'a Runtime,
    theme: Theme,
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    f: &mut Frame,
    stack: &[Page],
    all: &Data,
    count: bool,
    (title, scope): (&str, &str),
    rows: &[Row],
    active: Option<(usize, (View, &'static str))>,
    rt: &Runtime,
    mono: bool,
) {
    let cx = Ctx {
        all,
        count,
        rows,
        active,
        rt,
        theme: Theme { mono },
    };
    let area = f.area();
    f.render_widget(Block::default().style(cx.theme.base()), area);
    if too_small(area.width, area.height) {
        draw_too_small(f, area, cx.theme);
        return;
    }
    let page = stack.last().unwrap();
    let [bar, path, answer, chip, body, hint, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(answer_rows(area.width, title)),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, [bar, path, answer, chip], stack, (title, scope), &cx);
    if matches!(page.view, View::Home) {
        home::draw_home(f, body, &cx);
    } else {
        list::draw_list(f, body, page, &cx);
    }
    draw_hint(f, hint, &cx);
    f.render_widget(
        Paragraph::new(keys(&footer_keys(page.view, area.width), cx.theme))
            .style(cx.theme.on(INK, PANEL)),
        footer,
    );
}

fn draw_too_small(f: &mut Frame, area: Rect, theme: Theme) {
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
}

/// The bar, breadcrumb, answer line and scope chips.
fn draw_header(
    f: &mut Frame,
    [bar, path, answer, chip]: [Rect; 4],
    stack: &[Page],
    (title, scope): (&str, &str),
    cx: &Ctx,
) {
    let theme = cx.theme;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" LENS ", theme.panel().add_modifier(Modifier::BOLD)),
            Span::styled(
                match cx.all.source {
                    Source::Fixture => "  synthetic data · nothing leaves this machine",
                    Source::Local => "  local logs · read-only · nothing leaves this machine",
                },
                theme.fg(MUTED),
            ),
        ]))
        .style(theme.on(INK, PANEL)),
        bar,
    );
    let max = usize::from(f.area().width.saturating_sub(2));
    f.render_widget(
        Paragraph::new(format!(" {}", breadcrumb(stack, cx.all, max))).style(theme.fg(MUTED)),
        path,
    );
    // The marker gets its own column so a wrapped answer keeps its indent.
    let [marker, text] =
        Layout::horizontal([Constraint::Length(3), Constraint::Fill(1)]).areas(answer);
    f.render_widget(Paragraph::new(" ◆ ").style(theme.fg(CYAN)), marker);
    f.render_widget(
        Paragraph::new(title.to_string())
            .style(theme.fg(INK).add_modifier(Modifier::BOLD))
            .wrap(Wrap { trim: true }),
        text,
    );
    f.render_widget(
        Paragraph::new(chips(scope, usize::from(f.area().width), theme)),
        chip,
    );
}

/// What Enter opens, and which of the selectable rows is selected.
fn draw_hint(f: &mut Frame, hint: Rect, cx: &Ctx) {
    let Some((index, (_, action))) = cx.active else {
        return;
    };
    let n = cx
        .rows
        .iter()
        .take(index + 1)
        .filter(|row| row.open.is_some())
        .count();
    let total = cx.rows.iter().filter(|row| row.open.is_some()).count();
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(8)]).areas(hint);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter ", cx.theme.key()),
            Span::styled(action.to_string(), cx.theme.fg(CYAN)),
        ])),
        left,
    );
    f.render_widget(
        Paragraph::new(format!("{n}/{total} "))
            .style(cx.theme.fg(MUTED))
            .right_aligned(),
        right,
    );
}

fn footer_keys(view: View, w: u16) -> Vec<(&'static str, &'static str)> {
    let widen = view.widened().is_some();
    // Only views that follow the selected measure advertise its toggle.
    let measured = view.measured();
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
