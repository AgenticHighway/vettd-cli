//! Scope, views, pages and the row helpers every view builds from.

use crate::fixture::{Session, PERIOD};
use std::collections::BTreeSet;
use std::ops::Range;

/// The question a result answers: which workspace, agent and period.
/// It travels with the view so Back restores the earlier scope exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Scope {
    pub(crate) ws: Option<&'static str>,
    pub(crate) agent: Option<&'static str>,
    pub(crate) from: u32,
    pub(crate) to: u32,
}

impl Scope {
    pub(crate) const WEEK: Scope = Scope {
        ws: None,
        agent: None,
        from: 12,
        to: 18,
    };

    pub(crate) fn of(s: &Session) -> Self {
        Self {
            ws: Some(s.workspace),
            agent: Some(s.agent),
            from: s.day,
            to: s.day,
        }
    }

    pub(crate) fn has(&self, s: &Session) -> bool {
        (self.from..=self.to).contains(&s.day)
            && self.ws.is_none_or(|w| w == s.workspace)
            && self.agent.is_none_or(|a| a == s.agent)
    }

    pub(crate) fn period(&self) -> String {
        if self.from == self.to {
            format!("Nov {} UTC", self.from)
        } else {
            PERIOD.into()
        }
    }

    pub(crate) fn label(&self) -> String {
        format!(
            "{} | {} | {}",
            self.ws
                .map_or("All workspaces".into(), |w| format!("Workspace {w}")),
            self.agent.unwrap_or("All agents"),
            self.period()
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum View {
    Home,
    Headroom,
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
    /// Views whose numbers follow the unit toggle. Only these offer `c`.
    pub(crate) fn measured(self) -> bool {
        matches!(
            self,
            View::Home | View::Week | View::Day(_) | View::Session(_) | View::Tokens(_)
        )
    }

    /// The explicit, reversible widening from one day to the whole period.
    pub(crate) fn widened(self) -> Option<View> {
        match self {
            View::Component(c, s) if s.from == s.to => Some(View::Component(c, s.wide())),
            View::Signal(c, n, s) if s.from == s.to => Some(View::Signal(c, n, s.wide())),
            _ => None,
        }
    }
}

impl Scope {
    pub(crate) fn wide(self) -> Self {
        Self {
            from: Scope::WEEK.from,
            to: Scope::WEEK.to,
            ..self
        }
    }
}

pub(crate) struct Page {
    pub(crate) view: View,
    pub(crate) selected: usize,
    pub(crate) top: usize,
    pub(crate) filter: String,
    /// Inventory filters: type, workspace, agent, evidence (0 = all).
    pub(crate) filters: [usize; 4],
    /// Whether the viewport follows the selection. Scrolling past the first
    /// or last selectable row, or on a view with none, turns it off so rows
    /// without actions stay reachable.
    pub(crate) follow: bool,
    /// Home only: the daily bar chosen with ←/→. While set, Enter opens that
    /// day instead of the selected finding.
    pub(crate) day: Option<u32>,
}

impl Page {
    pub(crate) fn new(view: View) -> Self {
        Self {
            view,
            selected: 0,
            top: 0,
            filter: String::new(),
            filters: [0; 4],
            follow: true,
            day: None,
        }
    }
}

/// Process-local simulation state, independent of the fixture and history.
#[derive(Default)]
pub(crate) struct Runtime {
    pub(crate) disabled: BTreeSet<usize>,
    pub(crate) notice: Option<(usize, String)>,
}

pub(crate) struct Row {
    pub(crate) text: String,
    pub(crate) open: Option<(View, &'static str)>,
    /// Char ranges of `text` drawn with extra weight or less. Empty for
    /// most rows, which draw in one style.
    pub(crate) spans: Vec<(Range<usize>, Emphasis)>,
}

/// How a span of a row is drawn: figures bold, secondary columns muted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Emphasis {
    Figure,
    Secondary,
}

pub(crate) fn line(text: impl Into<String>) -> Row {
    Row {
        text: text.into(),
        open: None,
        spans: Vec::new(),
    }
}

pub(crate) fn link(text: impl Into<String>, view: View, action: &'static str) -> Row {
    Row {
        text: text.into(),
        open: Some((view, action)),
        spans: Vec::new(),
    }
}

/// Builds a row's text from its cells, recording which cells are figures
/// or secondary. The builders that know the columns mark them, so nothing
/// has to guess which digits are amounts.
#[derive(Default)]
pub(crate) struct Cells {
    text: String,
    spans: Vec<(Range<usize>, Emphasis)>,
}

impl Cells {
    pub(crate) fn add(mut self, cell: impl AsRef<str>, emphasis: Option<Emphasis>) -> Self {
        let cell = cell.as_ref();
        let start = self.text.chars().count();
        self.text.push_str(cell);
        if let Some(emphasis) = emphasis {
            self.spans
                .push((start..start + cell.chars().count(), emphasis));
        }
        self
    }

    pub(crate) fn line(self) -> Row {
        Row {
            text: self.text,
            open: None,
            spans: self.spans,
        }
    }

    pub(crate) fn link(self, view: View, action: &'static str) -> Row {
        Row {
            open: Some((view, action)),
            ..self.line()
        }
    }
}

/// Cuts text to `max` characters, keeping its start and its end, because
/// names often differ only at the ends. Counts chars so text is never split
/// inside a character.
pub(crate) fn shorten(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    match max {
        0 => String::new(),
        1 => "…".into(),
        _ => {
            let head = max / 2;
            let tail = (max - 1) / 2;
            let start: String = chars[..head].iter().collect();
            let end: String = chars[chars.len() - tail..].iter().collect();
            format!("{start}…{end}")
        }
    }
}

pub(crate) fn usd(cents: u32) -> String {
    format!("${}.{:02}", cents / 100, cents % 100)
}

pub(crate) fn bar(amount: u32, max: u32, width: usize) -> String {
    let boxes = (amount as usize * width).div_ceil(max.max(1) as usize);
    format!("{:width$}", "━".repeat(boxes))
}

pub(crate) fn priced(s: &Session) -> usize {
    s.requests
        .iter()
        .filter(|r| r.milli_usd_per_ktok.is_some())
        .count()
}

pub(crate) fn measure(s: &Session, count: bool) -> u32 {
    if count {
        s.requests.len() as u32
    } else {
        s.cents()
    }
}

pub(crate) fn units(value: u32, count: bool) -> String {
    if count {
        format!("{value} req")
    } else {
        usd(value)
    }
}

pub(crate) fn cost_label(s: &Session) -> String {
    let n = priced(s);
    if n == 0 {
        "No priced requests".into()
    } else {
        format!("{} USD ({n}/{} priced)", usd(s.cents()), s.requests.len())
    }
}

pub(crate) fn comparable(s: &Session, count: bool) -> bool {
    !s.incomplete && (count || priced(s) == s.requests.len())
}

/// `ws` is the workspace column width; long names are shortened to fit it.
pub(crate) fn session_row(id: usize, s: &Session, count: bool, ws: usize) -> Row {
    let measure = if count {
        format!("{} req", s.requests.len())
    } else if s.requests.iter().all(|r| r.milli_usd_per_ktok.is_none()) {
        "unpriced".into()
    } else {
        usd(s.cents())
    };
    // "unpriced" is a flag, not an amount, so it isn't bold.
    let figure = (measure != "unpriced").then_some(Emphasis::Figure);
    Cells::default()
        .add(format!("{:<16} ", s.name), None)
        .add(
            format!("{:<ws$}", shorten(s.workspace, ws)),
            Some(Emphasis::Secondary),
        )
        .add(" ", None)
        .add(format!("{:<7}", s.agent), Some(Emphasis::Secondary))
        .add(" ", None)
        .add(format!("{measure:>8}"), figure)
        .add(if s.incomplete { "  partial" } else { "" }, None)
        .link(View::Session(id), "Open session cost and components")
}
