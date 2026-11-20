//! Frame computation and key handling, apart from the terminal.

use crate::fixture::{workspaces, Session, AGENTS, KINDS};
use crate::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// What one frame shows: computed from the app state at a terminal size.
pub(crate) struct Shown {
    pub(crate) title: String,
    pub(crate) scope: String,
    pub(crate) rows: Vec<Row>,
    pub(crate) active: Option<(usize, (View, &'static str))>,
    pub(crate) visible: usize,
}

/// What arrow and page keys work against: the selectable rows, the
/// selected row's position, and how many rows fit.
pub(crate) struct Viewport {
    pub(crate) actions: usize,
    pub(crate) focus: Option<usize>,
    pub(crate) rows: usize,
    pub(crate) visible: usize,
}

impl Viewport {
    /// Moves the selection while there is a selectable row to move to, and
    /// scrolls otherwise: on views without actions, above the first action
    /// and below the last, so leading and trailing rows stay reachable.
    pub(crate) fn navigate(&self, page: &mut Page, down: bool, step: usize) {
        let max_top = self.rows.saturating_sub(self.visible);
        let shown = |f: usize| (page.top..page.top + self.visible).contains(&f);
        if !page.follow && self.focus.is_some_and(shown) {
            page.follow = true;
        }
        let can_move = if down {
            page.selected + 1 < self.actions
        } else {
            page.selected > 0
        };
        if page.follow && can_move {
            page.selected = if down {
                (page.selected + step).min(self.actions - 1)
            } else {
                page.selected.saturating_sub(step)
            };
            return;
        }
        page.follow = false;
        page.top = if down {
            (page.top + step).min(max_top)
        } else {
            page.top.saturating_sub(step)
        };
    }
}

/// Everything the loop carries between frames. Keeping it apart from the
/// terminal lets tests drive real key presses through the same code.
pub(crate) struct App {
    pub(crate) stack: Vec<Page>,
    pub(crate) count: bool,
    pub(crate) rt: Runtime,
}

impl App {
    pub(crate) fn new() -> Self {
        Self {
            stack: vec![Page::new(View::Home)],
            count: false,
            rt: Runtime::default(),
        }
    }

    /// Computes the current frame and clamps selection and scroll to it.
    pub(crate) fn frame(&mut self, all: &[Session], (w, h): (u16, u16)) -> Shown {
        let page = self.stack.last_mut().unwrap();
        let (title, scope, rows) = content(page, all, self.count, screen::list_width(w), &self.rt);
        let actions: Vec<_> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.open.map(|open| (i, open)))
            .collect();
        page.selected = page.selected.min(actions.len().saturating_sub(1));
        let active = actions.get(page.selected).copied();
        let visible = screen::visible(h, screen::answer_rows(w, &title));
        let focus = active.map_or(page.selected, |(i, _)| i);
        if page.follow {
            page.top = page.top.min(focus).max((focus + 1).saturating_sub(visible));
        } else {
            page.top = page.top.min(rows.len().saturating_sub(visible));
        }
        Shown {
            title,
            scope,
            rows,
            active,
            visible,
        }
    }

    pub(crate) fn draw(&self, f: &mut ratatui::Frame, all: &[Session], shown: &Shown, mono: bool) {
        screen::draw(
            f,
            &self.stack,
            all,
            self.count,
            (&shown.title, &shown.scope),
            &shown.rows,
            shown.active,
            &self.rt,
            mono,
        );
    }

    /// Applies one key press. Returns false when the key quits.
    pub(crate) fn press(&mut self, all: &[Session], (w, h): (u16, u16), key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return true;
        }
        let ctrl_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        if screen::too_small(w, h) {
            return !(ctrl_c || key.code == KeyCode::Char('q'));
        }
        if ctrl_c {
            return false;
        }
        let shown = self.frame(all, (w, h));
        match key.code {
            KeyCode::Esc => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                }
            }
            KeyCode::Enter => self.open(shown.active),
            code if self.edit_page(code, all, &shown) => {}
            code => return self.global(code, all, (w, h), shown.active),
        }
        true
    }

    /// Enter: open the selected row, or apply a staged change and go back.
    fn open(&mut self, active: Option<(usize, (View, &'static str))>) {
        let Some((_, (next, _))) = active else {
            return;
        };
        if let View::Apply(pid, disable) = next {
            components::apply(pid, disable, &mut self.rt);
            self.stack.pop();
        } else {
            self.stack.push(Page::new(next));
        }
    }

    /// Keys that act on the current page: moving and scrolling, typing in
    /// Find, and inventory filters. Returns false when the key isn't one.
    /// Find takes letters first, so typing never triggers a command.
    fn edit_page(&mut self, code: KeyCode, all: &[Session], shown: &Shown) -> bool {
        let page = self.stack.last_mut().unwrap();
        let finding = matches!(page.view, View::Find);
        let inventory = matches!(page.view, View::Inventory);
        let reset = |page: &mut Page| {
            page.selected = 0;
            page.top = 0;
        };
        let cycle = |page: &mut Page, slot: usize, len: usize| {
            page.filters[slot] = (page.filters[slot] + 1) % (len + 1);
            reset(page);
        };
        match code {
            KeyCode::Down | KeyCode::Up | KeyCode::PageDown | KeyCode::PageUp => {
                let paged = matches!(code, KeyCode::PageDown | KeyCode::PageUp);
                let down = matches!(code, KeyCode::Down | KeyCode::PageDown);
                let view = Viewport {
                    actions: shown.rows.iter().filter(|r| r.open.is_some()).count(),
                    focus: shown.active.map(|(i, _)| i),
                    rows: shown.rows.len(),
                    visible: shown.visible,
                };
                view.navigate(page, down, if paged { shown.visible } else { 1 });
            }
            KeyCode::Backspace if finding => {
                page.filter.pop();
                reset(page);
            }
            KeyCode::Char(ch) if finding && (ch.is_ascii_graphic() || ch == ' ') => {
                page.filter.push(ch);
                reset(page);
            }
            KeyCode::Tab if inventory => cycle(page, 0, KINDS.len()),
            KeyCode::Char('w') if inventory => cycle(page, 1, workspaces(all).len()),
            KeyCode::Char('a') if inventory => cycle(page, 2, AGENTS.len()),
            KeyCode::Char('e') if inventory => cycle(page, 3, components::EVIDENCE.len()),
            KeyCode::Char('x') if inventory => {
                page.filters = [0; 4];
                reset(page);
            }
            _ => return false,
        }
        true
    }

    /// Keys that work everywhere. Returns false when the key quits.
    fn global(
        &mut self,
        code: KeyCode,
        all: &[Session],
        size: (u16, u16),
        active: Option<(usize, (View, &'static str))>,
    ) -> bool {
        let view = self.stack.last().unwrap().view;
        match code {
            KeyCode::Char('p') => {
                if let Some(next) = view.widened() {
                    self.stack.push(Page::new(next));
                }
            }
            KeyCode::Char('i') if !matches!(view, View::Inventory) => {
                self.stack.push(Page::new(View::Inventory))
            }
            // Help on top of help would take one Esc per press to leave.
            KeyCode::Char('?') if view != View::Help => self.stack.push(Page::new(View::Help)),
            KeyCode::Char('q') => return false,
            KeyCode::Char('/') => self.stack.push(Page::new(View::Find)),
            KeyCode::Char('c') if view.measured() => {
                self.toggle_unit(all, size, active.map(|(_, (target, _))| target))
            }
            _ => {}
        }
        true
    }

    /// Switches between USD and requests, keeping the selection on the row
    /// that opens the same thing. Rows can appear or vanish with the unit
    /// (Nov 16 is unusual in dollars, not in requests), so the selection
    /// follows the target, not the position. If it vanished, the first row
    /// is selected rather than whichever row slid into its place.
    fn toggle_unit(&mut self, all: &[Session], (w, _): (u16, u16), before: Option<View>) {
        self.count = !self.count;
        let page = self.stack.last_mut().unwrap();
        let (_, _, rows) = content(page, all, self.count, screen::list_width(w), &self.rt);
        let targets: Vec<View> = rows.iter().filter_map(|r| r.open.map(|o| o.0)).collect();
        page.selected = before
            .and_then(|v| targets.iter().position(|t| *t == v))
            .unwrap_or(0);
        page.follow = true;
    }
}
