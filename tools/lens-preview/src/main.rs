mod components;
mod fixture;
mod screen;

use fixture::{Session, AGENTS, COMPONENTS, KINDS, PARTS, PERIOD, WORKSPACES};
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
        execute,
        style::ResetColor,
        terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
    },
};
use std::collections::BTreeSet;
use std::io::{self, BufWriter};

/// The question a result answers: which workspace, agent and period.
/// It travels with the view so Back restores the earlier scope exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Scope {
    ws: Option<&'static str>,
    agent: Option<&'static str>,
    from: u32,
    to: u32,
}

impl Scope {
    const WEEK: Scope = Scope {
        ws: None,
        agent: None,
        from: 12,
        to: 18,
    };

    fn of(s: &Session) -> Self {
        Self {
            ws: Some(s.workspace),
            agent: Some(s.agent),
            from: s.day,
            to: s.day,
        }
    }

    fn has(&self, s: &Session) -> bool {
        (self.from..=self.to).contains(&s.day)
            && self.ws.is_none_or(|w| w == s.workspace)
            && self.agent.is_none_or(|a| a == s.agent)
    }

    fn period(&self) -> String {
        if self.from == self.to {
            format!("Nov {} UTC", self.from)
        } else {
            PERIOD.into()
        }
    }

    fn label(&self) -> String {
        format!(
            "{} | {} | {}",
            self.ws
                .map_or("All workspaces".into(), |w| format!("Workspace {w}")),
            self.agent.unwrap_or("All agents"),
            self.period()
        )
    }
}

#[derive(Clone, Copy)]
enum View {
    Home,
    Week,
    Find,
    Day(u32),
    Session(usize),
    Tokens(usize),
    Help,
    Inventory,
    Component(usize, Scope),
    Placement(usize),
    Observation(usize, usize),
    Signal(usize, &'static str, Scope),
    Change(usize, bool),
    Apply(usize, bool),
}

impl View {
    /// The explicit, reversible widening from one day to the whole period.
    fn widened(self) -> Option<View> {
        match self {
            View::Component(c, s) if s.from == s.to => Some(View::Component(c, s.wide())),
            View::Signal(c, n, s) if s.from == s.to => Some(View::Signal(c, n, s.wide())),
            _ => None,
        }
    }
}

impl Scope {
    fn wide(self) -> Self {
        Self {
            from: Scope::WEEK.from,
            to: Scope::WEEK.to,
            ..self
        }
    }
}

struct Page {
    view: View,
    selected: usize,
    top: usize,
    filter: String,
    /// Inventory filters: type, workspace, agent, evidence (0 = all).
    filters: [usize; 4],
}

impl Page {
    fn new(view: View) -> Self {
        Self {
            view,
            selected: 0,
            top: 0,
            filter: String::new(),
            filters: [0; 4],
        }
    }
}

/// Process-local simulation state, independent of the fixture and history.
#[derive(Default)]
struct Runtime {
    disabled: BTreeSet<usize>,
    notice: Option<(usize, String)>,
}

struct Row {
    text: String,
    open: Option<(View, &'static str)>,
}

fn line(text: impl Into<String>) -> Row {
    Row {
        text: text.into(),
        open: None,
    }
}

fn link(text: impl Into<String>, view: View, action: &'static str) -> Row {
    Row {
        text: text.into(),
        open: Some((view, action)),
    }
}

fn usd(cents: u32) -> String {
    format!("${}.{:02}", cents / 100, cents % 100)
}

fn bar(amount: u32, max: u32, width: usize) -> String {
    let boxes = (amount as usize * width).div_ceil(max.max(1) as usize);
    format!("{:width$}", "━".repeat(boxes))
}

fn priced(s: &Session) -> usize {
    s.requests
        .iter()
        .filter(|r| r.milli_usd_per_ktok.is_some())
        .count()
}

fn measure(s: &Session, count: bool) -> u32 {
    if count {
        s.requests.len() as u32
    } else {
        s.cents()
    }
}

fn units(value: u32, count: bool) -> String {
    if count {
        format!("{value} req")
    } else {
        usd(value)
    }
}

fn cost_label(s: &Session) -> String {
    let n = priced(s);
    if n == 0 {
        "No priced requests".into()
    } else {
        format!("{} USD ({n}/{} priced)", usd(s.cents()), s.requests.len())
    }
}

fn comparable(s: &Session, count: bool) -> bool {
    !s.incomplete && (count || priced(s) == s.requests.len())
}

fn session_row(id: usize, s: &Session, count: bool) -> Row {
    let measure = if count {
        format!("{} req", s.requests.len())
    } else if s.requests.iter().all(|r| r.milli_usd_per_ktok.is_none()) {
        "unpriced".into()
    } else {
        usd(s.cents())
    };
    link(
        format!(
            "{:<16} {:<4} {:<7} {:>8}{}",
            s.name,
            s.workspace,
            s.agent,
            measure,
            if s.incomplete { "  partial" } else { "" }
        ),
        View::Session(id),
        "Open session cost and components",
    )
}

/// The dashboard: one sentence on what was used, then findings to open.
/// It describes the week; judgments live on the screens the findings open.
fn home(all: &[Session], count: bool) -> (String, String, Vec<Row>) {
    let requests: usize = all.iter().map(|s| s.requests.len()).sum();
    let cents: u32 = all.iter().map(Session::cents).sum();
    let (days, normal) = screen::daily(all, count);
    let mut rows = Vec::new();
    if let Some((day, value)) = days
        .iter()
        .filter_map(|&(d, v)| v.map(|v| (d, v)))
        .max_by_key(|&(_, v)| v)
        .filter(|&(_, v)| normal > 0 && v > normal * 3)
    {
        rows.push(link(
            format!("Nov {day}: {}× a normal day", value / normal),
            View::Day(day),
            "See the sessions behind that day",
        ));
    }
    rows.push(link(
        "review-kit: unknown load in Codex",
        View::Component(0, Scope::WEEK),
        "Inspect copies and use this week",
    ));
    if let Some(id) = all.iter().position(|s| s.incomplete) {
        rows.push(link(
            format!("Nov {}: partial input, unpriced", all[id].day),
            View::Session(id),
            "Open the session",
        ));
    }
    rows.push(link(
        "Every day and its sessions",
        View::Week,
        "Open daily activity",
    ));
    (
        format!(
            "{} sessions · {requests} requests · {} tokens · {} priced",
            all.len(),
            screen::compact(screen::total_tokens(all)),
            usd(cents)
        ),
        format!(
            "All workspaces | All agents | {PERIOD} | {}",
            if count { "Requests" } else { "Priced USD" }
        ),
        rows,
    )
}

fn content(
    page: &Page,
    all: &[Session],
    count: bool,
    width: u16,
    rt: &Runtime,
) -> (String, String, Vec<Row>) {
    let bar_width = usize::from(width.saturating_sub(30).min(34));
    match page.view {
        View::Inventory
        | View::Component(..)
        | View::Placement(_)
        | View::Observation(..)
        | View::Signal(..)
        | View::Change(..)
        | View::Apply(..) => components::rows(page, all, rt),
        View::Home => home(all, count),
        View::Help => (
            "Keyboard shortcuts".into(),
            "Esc returns to your previous selection and scope".into(),
            [
                "↑ / ↓   Move selection; preview updates",
                "PgUp/Dn Scroll by a screen",
                "Enter   Inspect the selected object",
                "Esc     Back to previous scope",
                "p       Widen a one-day scope to the week",
                "i       Component inventory",
                "Tab     Inventory: cycle type",
                "w a e   Inventory: workspace, agent, evidence",
                "x       Inventory: clear filters",
                "/       Find a component",
                "c       Toggle priced USD and requests",
                "q       Quit (Ctrl+C also works in search)",
            ]
            .into_iter()
            .map(line)
            .collect(),
        ),

        View::Week => {
            let days: Vec<_> = (12..=18)
                .map(|day| {
                    let sessions: Vec<_> = all.iter().filter(|s| s.day == day).collect();
                    let value = sessions.iter().map(|s| measure(s, count)).sum();
                    (
                        day,
                        value,
                        sessions.len(),
                        sessions.iter().any(|s| s.incomplete),
                    )
                })
                .collect();
            let peak = days.iter().map(|day| day.1).max().unwrap_or(1);
            let rows = days
                .into_iter()
                .map(|(day, value, n, partial)| {
                    let label = if n == 0 {
                        "No input".into()
                    } else if count {
                        value.to_string()
                    } else {
                        usd(value)
                    };
                    link(
                        format!(
                            "Nov {day}  {label:>8}  {}{}",
                            bar(value, peak, bar_width),
                            if partial { "  partial" } else { "" }
                        ),
                        View::Day(day),
                        "See contributing sessions",
                    )
                })
                .collect();
            (
                "Activity over time".into(),
                format!(
                    "{PERIOD} | All workspaces | {} per day",
                    if count { "Requests" } else { "Priced USD" }
                ),
                rows,
            )
        }
        View::Find => {
            let rows = COMPONENTS
                .iter()
                .enumerate()
                .map(|(i, c)| (i, format!("{} {}", c.kind, c.name)))
                .filter(|(_, name)| {
                    name.to_ascii_lowercase()
                        .contains(&page.filter.to_ascii_lowercase())
                })
                .map(|(i, name)| {
                    link(
                        name,
                        View::Component(i, Scope::WEEK),
                        "Inspect copies and use this week",
                    )
                })
                .collect();
            (
                "Find a component".into(),
                format!("{PERIOD} | Search: {}_", page.filter),
                rows,
            )
        }
        View::Day(day) => {
            let mut sessions: Vec<_> = all
                .iter()
                .enumerate()
                .filter(|(_, s)| s.day == day)
                .collect();
            sessions.sort_by_key(|(_, s)| std::cmp::Reverse(measure(s, count)));
            let total: u32 = sessions.iter().map(|(_, s)| measure(s, count)).sum();
            let prior: Vec<_> = all.iter().filter(|s| s.day + 1 == day).collect();
            let comparison = if sessions.is_empty()
                || prior.is_empty()
                || prior.iter().any(|s| !comparable(s, count))
                || sessions.iter().any(|(_, s)| !comparable(s, count))
            {
                "No comparable previous day".into()
            } else {
                let previous = prior.iter().map(|s| measure(s, count)).sum::<u32>();
                format!(
                    "Prior day {} | {}{} change",
                    units(previous, count),
                    if total >= previous { "+" } else { "-" },
                    units(total.abs_diff(previous), count)
                )
            };
            (
                format!("Nov {day} sessions"),
                format!(
                    "Nov {day} UTC | All workspaces | {} | {comparison}",
                    units(total, count)
                ),
                sessions
                    .into_iter()
                    .map(|(id, s)| session_row(id, s, count))
                    .collect(),
            )
        }
        View::Session(id) | View::Tokens(id) => {
            let s = &all[id];
            let scope = format!(
                "Nov {} UTC | Workspace {} | {} | {}",
                s.day,
                s.workspace,
                s.agent,
                cost_label(s)
            );
            let mut rows = Vec::new();
            if s.incomplete {
                rows.push(line("Input incomplete"));
            }
            if matches!(page.view, View::Tokens(_)) {
                for r in &s.requests {
                    rows.push(line(format!("{} / {}", r.worker, r.model)));
                    rows.extend(
                        PARTS
                            .iter()
                            .zip(r.tokens)
                            .map(|(part, tokens)| line(format!("{part:<14} {tokens:>8} tokens"))),
                    );
                }
                (format!("{} / token and cache detail", s.name), scope, rows)
            } else {
                let parts: [u32; 4] =
                    std::array::from_fn(|i| s.requests.iter().map(|r| r.cents()[i]).sum());
                if priced(s) > 0 {
                    rows.extend(PARTS.iter().zip(parts).map(|(part, cost)| {
                        line(format!(
                            "{part:<13} {:>7}  {}",
                            usd(cost),
                            bar(cost, s.cents(), bar_width)
                        ))
                    }));
                }
                rows.extend(s.requests.iter().map(|r| {
                    line(format!(
                        "{} / {}   {}",
                        r.worker,
                        r.model,
                        if r.milli_usd_per_ktok.is_some() {
                            usd(r.cents().iter().sum())
                        } else {
                            "unpriced".into()
                        }
                    ))
                }));
                rows.push(link(
                    "Token and cache detail",
                    View::Tokens(id),
                    "See token counts by model and worker",
                ));
                rows.push(line("── Components observed ──"));
                rows.extend(s.observed.iter().map(|u| {
                    let c = &COMPONENTS[u.component];
                    link(
                        format!(
                            "{:<5} {:<13} {:<4} x{}",
                            c.kind,
                            c.name,
                            u.version.unwrap_or("?"),
                            u.count
                        ),
                        View::Component(u.component, Scope::of(s)),
                        "Inspect component in this session's scope",
                    )
                }));
                (format!("{} / cost components", s.name), scope, rows)
            }
        }
    }
}

struct Terminal;

impl Terminal {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(io::stderr(), EnterAlternateScreen, cursor::Hide)?;
        Ok(guard)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), ResetColor, cursor::Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn run(all: &[Session]) -> io::Result<()> {
    let _guard = Terminal::enter()?;
    // The TUI draws on stderr so stdout stays free for machine output.
    let mut term = ratatui::Terminal::new(CrosstermBackend::new(BufWriter::new(io::stderr())))?;
    let mut stack = vec![Page::new(View::Home)];
    let mut count = false;
    let mut rt = Runtime::default();
    let mono = screen::no_color();
    loop {
        let size = term.size()?;
        let (w, h) = (size.width, size.height);
        let can_go_back = stack.len() > 1;
        let page = stack.last_mut().unwrap();
        let (title, scope, rows) = content(page, all, count, screen::list_width(w), &rt);
        let actions: Vec<_> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.open.map(|open| (i, open)))
            .collect();
        let finding = matches!(page.view, View::Find);
        let inventory = matches!(page.view, View::Inventory);
        page.selected = page.selected.min(actions.len().saturating_sub(1));
        let active = actions.get(page.selected);
        let visible = screen::visible(h);
        let focus = active.map_or(page.selected, |(i, _)| *i);
        page.top = page.top.min(focus).max((focus + 1).saturating_sub(visible));
        let active = active.copied();
        term.draw(|f| {
            screen::draw(
                f,
                &stack,
                all,
                count,
                (&title, &scope),
                &rows,
                active,
                &rt,
                mono,
            )
        })?;
        let page = stack.last_mut().unwrap();
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if screen::too_small(w, h) {
            if key.code == KeyCode::Char('q')
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                break;
            }
            continue;
        }
        let mut cycle = |slot: usize, len: usize| {
            page.filters[slot] = (page.filters[slot] + 1) % (len + 1);
            page.selected = 0;
            page.top = 0;
        };
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
            KeyCode::Esc if can_go_back => {
                stack.pop();
            }
            KeyCode::Enter => {
                if let Some((_, (next, _))) = active {
                    if let View::Apply(pid, disable) = next {
                        components::apply(pid, disable, &mut rt);
                        stack.pop();
                    } else {
                        stack.push(Page::new(next));
                    }
                }
            }
            KeyCode::Down | KeyCode::PageDown => {
                let step = if key.code == KeyCode::PageDown {
                    visible
                } else {
                    1
                };
                page.selected = (page.selected + step).min(actions.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::PageUp => {
                page.selected = page
                    .selected
                    .saturating_sub(if key.code == KeyCode::PageUp {
                        visible
                    } else {
                        1
                    })
            }
            KeyCode::Backspace if finding => {
                page.filter.pop();
                page.selected = 0;
                page.top = 0;
            }
            KeyCode::Char(ch) if finding && (ch.is_ascii_graphic() || ch == ' ') => {
                page.filter.push(ch);
                page.selected = 0;
                page.top = 0;
            }
            KeyCode::Tab if inventory => cycle(0, KINDS.len()),
            KeyCode::Char('w') if inventory => cycle(1, WORKSPACES.len()),
            KeyCode::Char('a') if inventory => cycle(2, AGENTS.len()),
            KeyCode::Char('e') if inventory => cycle(3, components::EVIDENCE.len()),
            KeyCode::Char('x') if inventory => {
                page.filters = [0; 4];
                page.selected = 0;
                page.top = 0;
            }
            KeyCode::Char('p') => {
                if let Some(next) = page.view.widened() {
                    stack.push(Page::new(next));
                }
            }
            KeyCode::Char('i') if !inventory => stack.push(Page::new(View::Inventory)),
            KeyCode::Char('?') => stack.push(Page::new(View::Help)),
            KeyCode::Char('q') => break,
            KeyCode::Char('/') => stack.push(Page::new(View::Find)),
            KeyCode::Char('c') => count = !count,
            _ => {}
        }
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let sessions = fixture::sessions();
    if std::env::args().nth(1).as_deref() == Some("--json") {
        println!(
            "{}",
            serde_json::json!({
                "synthetic": true,
                "period": PERIOD,
                "snapshot": fixture::SNAPSHOT,
                "workspaces": WORKSPACES,
                "agents": AGENTS,
                "components": COMPONENTS,
                "placements": fixture::PLACEMENTS,
                "scans": fixture::SCANS,
                "sessions": sessions,
            })
        );
        return Ok(());
    }
    run(&sessions)
}

#[cfg(test)]
mod tests {
    use super::components::{apply, resolve, Resolution};
    use super::*;

    fn rows(view: View, rt: &Runtime) -> Vec<Row> {
        content(&Page::new(view), &fixture::sessions(), false, 80, rt).2
    }

    fn texts(view: View, rt: &Runtime) -> Vec<String> {
        rows(view, rt).into_iter().map(|r| r.text).collect()
    }

    fn opens_placement(rows: &[Row], pid: usize) -> bool {
        rows.iter()
            .any(|r| matches!(r.open, Some((View::Placement(q), _)) if q == pid))
    }

    fn find_use(day: u32, ws: &str, component: usize) -> (usize, usize) {
        let all = fixture::sessions();
        let sid = all
            .iter()
            .position(|s| s.day == day && s.workspace == ws)
            .unwrap();
        let u = all[sid]
            .observed
            .iter()
            .position(|u| u.component == component)
            .unwrap();
        (sid, u)
    }

    #[test]
    fn spike_reconciles_and_subagent_is_counted_once() {
        let sessions = fixture::sessions();
        let day: Vec<_> = sessions.iter().filter(|s| s.day == 16).collect();
        assert_eq!(day.len(), 2);
        assert_eq!(day.iter().map(|s| s.cents()).sum::<u32>(), 852);
        assert_eq!(day[0].cents(), 820);
    }

    #[test]
    fn unpriced_session_keeps_tokens_without_zero_cost_bars() {
        let all = fixture::sessions();
        let i = all.iter().position(|s| s.incomplete).unwrap();
        let rt = Runtime::default();
        let (_, scope, rows) = content(&Page::new(View::Session(i)), &all, false, 80, &rt);
        assert!(scope.contains("No priced requests"));
        assert!(rows.iter().all(|r| !r.text.contains("$0.00")));
        assert!(texts(View::Tokens(i), &rt)
            .iter()
            .any(|t| t.contains("30000 tokens")));
    }

    // Two workspaces both install review-kit 1.5. Only the api artifact was
    // scanned, so the web copy must not borrow its grade by version label.
    #[test]
    fn same_version_label_never_confers_scan_or_current_copy() {
        let rt = Runtime::default();
        assert!(texts(View::Placement(0), &rt)
            .iter()
            .any(|t| t.contains("RK-15")));
        assert!(texts(View::Placement(2), &rt)
            .iter()
            .any(|t| t == "Scan      None for this artifact"));
        let (sid, u) = find_use(16, "web", 0);
        let web = rows(View::Observation(sid, u), &rt);
        assert!(opens_placement(&web, 2));
        assert!(!opens_placement(&web, 0));
    }

    // A historical 1.4 observation leads to the user copy with that artifact,
    // even though a different copy is effective in the workspace today.
    #[test]
    fn observation_links_to_current_copy_only_by_artifact() {
        let rt = Runtime::default();
        let (sid, u) = find_use(16, "api", 0);
        let observed = rows(View::Observation(sid, u), &rt);
        assert!(opens_placement(&observed, 1));
        assert!(!opens_placement(&observed, 0));
        let (sid, u) = find_use(12, "api", 0);
        let unknown = texts(View::Observation(sid, u), &rt);
        assert!(unknown.contains(&"Installed unknown: artifact not recorded".to_string()));
        assert!(unknown.contains(&"Scan      Unavailable: no artifact".to_string()));
    }

    #[test]
    fn disabling_api_copy_falls_back_without_touching_web_or_history() {
        let mut rt = Runtime::default();
        let history = |rt: &Runtime| -> Vec<String> {
            texts(View::Component(0, Scope::WEEK), rt)
                .into_iter()
                .filter(|t| t.starts_with("Nov"))
                .collect()
        };
        let before = history(&rt);
        assert_eq!(
            resolve(0, "api", "Claude", &rt.disabled),
            Resolution::Copy(0)
        );
        apply(0, true, &mut rt);
        assert_eq!(
            resolve(0, "api", "Claude", &rt.disabled),
            Resolution::Copy(1)
        );
        assert_eq!(
            resolve(0, "web", "Claude", &rt.disabled),
            Resolution::Copy(1)
        );
        assert!(matches!(
            resolve(0, "web", "Codex", &rt.disabled),
            Resolution::Unknown(_)
        ));
        assert_eq!(before, history(&rt));
        apply(0, false, &mut rt);
        assert!(rt.disabled.is_empty());
        assert_eq!(
            resolve(0, "api", "Claude", &rt.disabled),
            Resolution::Copy(0)
        );
    }

    #[test]
    fn change_preview_predicts_fallback_or_states_unknown() {
        let rt = Runtime::default();
        assert!(texts(View::Change(0, true), &rt)
            .contains(&"Predicted user 1.4 (sha:1c4e)".to_string()));
        assert!(texts(View::Change(2, true), &rt)
            .iter()
            .any(|t| t.starts_with("Predicted Unknown")));
    }

    #[test]
    fn session_scope_stays_on_its_day_and_workspace_until_widened() {
        let all = fixture::sessions();
        let rt = Runtime::default();
        let (sid, _) = find_use(16, "api", 0);
        let day = View::Component(0, Scope::of(&all[sid]));
        let observed = |view: View| -> Vec<String> {
            texts(view, &rt)
                .into_iter()
                .filter(|t| t.starts_with("Nov"))
                .collect()
        };
        assert_eq!(observed(day), ["Nov 16 api Index rebuild  1.4   2 calls"]);
        let week = observed(day.widened().unwrap());
        assert_eq!(week.len(), 6);
        assert!(week.iter().all(|t| t.contains(" api ")));
    }

    // Beside the preview pane at 80 columns a row has 45 cells. A maintainer
    // triaging the list must see every row's state, unknown resolution first.
    #[test]
    fn inventory_state_fits_row_and_surfaces_unknown() {
        let mut rt = Runtime::default();
        apply(0, true, &mut rt);
        let rows = texts(View::Inventory, &rt);
        assert!(rows.iter().all(|t| t.chars().count() <= 45), "{rows:?}");
        let review = rows.iter().find(|t| t.contains("review-kit")).unwrap();
        assert!(review.ends_with(" Unknown +2 more"), "{review}");
    }

    #[test]
    fn empty_inventory_filter_reports_zero_results() {
        let mut page = Page::new(View::Inventory);
        page.filters = [4, 0, 2, 0];
        let (_, scope, rows) = content(&page, &fixture::sessions(), false, 80, &Runtime::default());
        assert!(rows.is_empty());
        assert!(scope.starts_with("0 of 10 |"));
    }

    use ratatui::{backend::TestBackend, buffer::Buffer, style::Modifier};

    fn render(stack: &[Page], (w, h): (u16, u16), mono: bool) -> Buffer {
        let all = fixture::sessions();
        let rt = Runtime::default();
        let page = stack.last().unwrap();
        let (title, scope, rows) = content(page, &all, false, screen::list_width(w), &rt);
        let active = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.open.map(|open| (i, open)))
            .nth(page.selected);
        let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            screen::draw(
                f,
                stack,
                &all,
                false,
                (&title, &scope),
                &rows,
                active,
                &rt,
                mono,
            )
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    fn screen_text(buffer: &Buffer) -> String {
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // Home is the first thing a developer sees. At 80x24 it must show what
    // was used and where it went before any finding, with the findings still
    // reachable on the same screen.
    #[test]
    fn home_dashboard_shows_usage_before_findings_at_80x24() {
        let text = screen_text(&render(&[Page::new(View::Home)], (80, 24), false));
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle} missing:\n{text}"))
        };
        assert!(at("12 sessions") < at("WHERE TOKENS WENT"));
        assert!(text.contains("$12.36 priced"), "{text}");
        assert!(at("DAILY SPEND") < at("WORTH A LOOK"));
        for day in 12..=18 {
            assert!(text.contains(&day.to_string()), "day {day} label missing");
        }
        // Hooks are listed so people learn they cost nothing, not omitted.
        assert!(at("Hooks") > at("File reads"));
        assert!(text.contains("▸ Nov 16:"), "{text}");
        assert!(text.contains("review-kit: unknown load in Codex"), "{text}");
    }

    // At 60 columns the dashboard stacks instead of truncating its findings.
    #[test]
    fn home_dashboard_stacks_at_60x24_and_keeps_selection() {
        let text = screen_text(&render(&[Page::new(View::Home)], (60, 24), false));
        assert!(
            text.contains("$12.36 priced"),
            "answer cut:
{text}"
        );
        assert!(text.contains("WHERE TOKENS WENT"), "{text}");
        assert!(text.contains("WORTH A LOOK"), "{text}");
        assert!(text.contains("▸ Nov 16:"), "{text}");
    }

    // Shares on Home must add to 100% or the dashboard misstates the week.
    #[test]
    fn token_source_shares_sum_to_whole() {
        let total: u32 = fixture::SOURCE_SHARES.iter().map(|s| s.1).sum();
        assert_eq!(total, 10_000);
    }

    // Under NO_COLOR, focus must still be visible without colour.
    #[test]
    fn no_color_marks_selection_with_reverse_video_only() {
        let buffer = render(&[Page::new(View::Home)], (80, 24), true);
        let mut reversed = false;
        for y in 0..24 {
            for x in 0..80 {
                let cell = &buffer[(x, y)];
                assert_eq!(cell.fg, ratatui::style::Color::Reset, "colour at {x},{y}");
                reversed |= cell.modifier.contains(Modifier::REVERSED);
            }
        }
        assert!(reversed);
    }

    // Esc must always be able to find Home, so a deep breadcrumb keeps its
    // root and current level even when it no longer fits.
    #[test]
    fn deep_breadcrumb_keeps_home_and_current_level() {
        let stack = [
            Page::new(View::Home),
            Page::new(View::Day(16)),
            Page::new(View::Session(6)),
            Page::new(View::Component(0, Scope::WEEK)),
            Page::new(View::Placement(1)),
        ];
        let text = screen_text(&render(&stack, (40, 24), false));
        let crumb = text.lines().nth(1).unwrap();
        assert!(crumb.trim_start().starts_with("Home ›"), "{crumb}");
        assert!(crumb.trim_end().ends_with("Copy"), "{crumb}");
    }

    #[test]
    fn too_small_terminal_says_what_it_needs() {
        let text = screen_text(&render(&[Page::new(View::Home)], (30, 8), false));
        assert!(text.contains("38×10"), "{text}");
    }
}
