//! Scope, views, pages and the row helpers every view builds from.

use crate::data::{day_label, period_label};
use crate::fixture::Session;
use std::collections::BTreeSet;

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
            format!("{} UTC", day_label(self.from))
        } else {
            period_label(self.from, self.to)
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
}

pub(crate) fn line(text: impl Into<String>) -> Row {
    Row {
        text: text.into(),
        open: None,
    }
}

pub(crate) fn link(text: impl Into<String>, view: View, action: &'static str) -> Row {
    Row {
        text: text.into(),
        open: Some((view, action)),
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
    s.requests.iter().filter(|r| r.priced()).count()
}

pub(crate) fn measure(s: &Session, count: bool) -> u32 {
    if count {
        s.requests.len() as u32
    } else {
        s.cents()
    }
}

/// Several sessions' requests or cents. Cents are summed unrounded and
/// rounded once, so many sub-cent sessions don't add up to $0.00.
pub(crate) fn total<'a>(sessions: impl IntoIterator<Item = &'a Session>, count: bool) -> u32 {
    let sessions: Vec<&Session> = sessions.into_iter().collect();
    if count {
        sessions.iter().map(|s| s.requests.len() as u32).sum()
    } else {
        sessions
            .iter()
            .map(|s| s.cents_exact())
            .sum::<f64>()
            .round() as u32
    }
}

/// Whether any of these sessions has a request with a rate. A dollar total
/// with none is unknown, not $0.00.
pub(crate) fn any_priced<'a>(sessions: impl IntoIterator<Item = &'a Session>) -> bool {
    sessions.into_iter().any(|s| priced(s) > 0)
}

pub(crate) fn units(value: u32, count: bool) -> String {
    if count {
        format!("{value} req")
    } else {
        usd(value)
    }
}

/// A session's dollar figure. On local data it is what the usage would
/// cost on the API, which the person may not pay (a plan may cover it).
pub(crate) fn cost_label(s: &Session, local: bool) -> String {
    let n = priced(s);
    match (n, local) {
        (0, false) => "No priced requests".into(),
        (0, true) => "No request has a rate".into(),
        (_, false) => format!("{} USD ({n}/{} priced)", usd(s.cents()), s.requests.len()),
        (_, true) => format!(
            "{} USD API-equivalent ({n}/{} with a rate)",
            usd(s.cents()),
            s.requests.len()
        ),
    }
}

pub(crate) fn comparable(s: &Session, count: bool) -> bool {
    !s.incomplete && (count || priced(s) == s.requests.len())
}

pub(crate) fn session_row(id: usize, s: &Session, count: bool) -> Row {
    let measure = if count {
        format!("{} req", s.requests.len())
    } else if s.requests.iter().all(|r| !r.priced()) {
        "unpriced".into()
    } else {
        usd(s.cents())
    };
    link(
        format!(
            "{:<16} {:<4} {:<7} {:>8}{}",
            s.name.chars().take(16).collect::<String>(),
            s.workspace,
            s.agent,
            measure,
            if s.incomplete { "  partial" } else { "" }
        ),
        View::Session(id),
        "Open session cost and components",
    )
}
