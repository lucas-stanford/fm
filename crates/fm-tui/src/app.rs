//! The state machine behind every screen. Keys and refreshes change it; `view` draws it. Nothing
//! here draws, and nothing in `view` calls the host.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fm_model::{
    AutomationState, CliId, ConsentPreview, Host, LiveRun, RunLink, RunProgress, RunRecord, Status,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::glyphs::Glyphs;
use crate::theme::{Theme, UiColor};
use crate::triage::{Action, Triage, actions_for, key_action, triage};
use crate::words::Now;

/// How long a message stays on the hint line.
pub const FLASH_FOR: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Esc,
    Tab,
    Char(char),
    Quit,
}

impl Key {
    /// `None` for releases (Windows reports them) and keys nothing uses.
    pub fn from_event(ev: KeyEvent) -> Option<Key> {
        if ev.kind == KeyEventKind::Release {
            return None;
        }
        if ev.modifiers.contains(KeyModifiers::CONTROL) {
            return matches!(ev.code, KeyCode::Char('c') | KeyCode::Char('C')).then_some(Key::Quit);
        }
        Some(match ev.code {
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Esc,
            KeyCode::Tab => Key::Tab,
            KeyCode::Char(c) => Key::Char(c),
            _ => return None,
        })
    }

    /// The key as the hint line and messages name it.
    fn name(self) -> String {
        match self {
            Key::Char(' ') => "space".into(),
            Key::Char(c) => c.to_string(),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flash {
    pub text: String,
    pub tone: UiColor,
    pub at: Option<Instant>,
}

/// What the loop must do outside the state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Quit,
    /// Hand the terminal to an editor on `path`, then call [`App::after_edit`].
    Edit {
        key: String,
        path: PathBuf,
    },
    Open(String),
}

/// One run's page.
#[derive(Debug, Clone, PartialEq)]
pub struct RunPage {
    pub key: String,
    /// Newest first.
    pub runs: Vec<RunRecord>,
    /// Into [`App::run_items`]: the live run first while there is one.
    pub selected: usize,
    pub graph: bool,
    pub scroll: usize,
    /// `(width, chart, progress, answer)` of the last `render_chart`.
    pub chart: Option<ChartCache>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartCache {
    pub width: u16,
    pub chart: String,
    pub progress: Option<RunProgress>,
    pub drawn: Result<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    List,
    Automation { key: String, index: usize },
    Run(Box<RunPage>),
    Check { key: String, scroll: usize },
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChoice {
    /// `None`: use the default.
    pub cli: Option<CliId>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    Consent {
        key: String,
        preview: Box<Result<ConsentPreview, String>>,
        scroll: usize,
    },
    Confirm {
        key: String,
        what: Confirm,
    },
    Agents {
        key: String,
        items: Vec<AgentChoice>,
        index: usize,
    },
    Links {
        links: Vec<RunLink>,
        index: usize,
    },
}

/// What the run page shows for one item of its list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RunItem<'a> {
    Live(&'a Status, Option<&'a LiveRun>),
    Record(&'a RunRecord),
}

pub struct App {
    pub now: Now,
    pub statuses: Vec<Status>,
    pub triage: Triage,
    pub watcher: String,
    pub live: BTreeMap<String, LiveRun>,
    /// Recent runs per automation, read when its page opens.
    pub records: BTreeMap<String, Vec<RunRecord>>,
    pub selected: Option<String>,
    selected_index: usize,
    /// The list is underneath; the last screen is on top.
    pub stack: Vec<Screen>,
    pub overlay: Option<Overlay>,
    pub flash: Option<Flash>,
    pub glyphs: Glyphs,
    pub theme: Theme,
    pub quit: bool,
    /// Body rows of the last draw, for page up and down.
    pub page_rows: usize,
    /// The furthest the page on screen can scroll, as the last draw found it.
    pub scroll_max: Cell<usize>,
}

impl App {
    pub fn new(now: Now, theme: Theme, glyphs: Glyphs) -> App {
        App {
            now,
            statuses: vec![],
            triage: Triage { sections: vec![] },
            watcher: String::new(),
            live: BTreeMap::new(),
            records: BTreeMap::new(),
            selected: None,
            selected_index: 0,
            stack: vec![],
            overlay: None,
            flash: None,
            glyphs,
            theme,
            quit: false,
            page_rows: 20,
            scroll_max: Cell::new(0),
        }
    }

    /// A ready app: data read, the first row selected.
    pub fn load(host: &mut dyn Host, now: Now, theme: Theme, glyphs: Glyphs) -> App {
        let mut app = App::new(now, theme, glyphs);
        app.refresh(host);
        app
    }

    pub fn screen(&self) -> &Screen {
        self.stack.last().unwrap_or(&Screen::List)
    }

    pub fn status(&self, key: &str) -> Option<&Status> {
        self.statuses.iter().find(|s| s.key == key)
    }

    pub fn selected_status(&self) -> Option<&Status> {
        self.selected.as_deref().and_then(|k| self.status(k))
    }

    pub fn any_running(&self) -> bool {
        self.statuses.iter().any(|s| s.running.is_some())
    }

    /// How often to re-read: every second while something runs, else now and then.
    pub fn refresh_every(&self, idle: Duration, busy: Duration) -> Duration {
        if self.any_running() { busy } else { idle }
    }

    fn order(&self) -> Vec<String> {
        self.triage.order().map(|e| e.key.clone()).collect()
    }

    /// Re-read everything; the cursor follows its automation.
    pub fn refresh(&mut self, host: &mut dyn Host) {
        self.statuses = host.list();
        self.triage = triage(&self.statuses);
        self.watcher = host.watcher_status();
        let running: Vec<String> = self
            .statuses
            .iter()
            .filter(|s| s.running.is_some())
            .map(|s| s.key.clone())
            .collect();
        self.live.retain(|k, _| running.contains(k));
        for key in running {
            match host.live(&key) {
                Some(l) => {
                    self.live.insert(key, l);
                }
                None => {
                    self.live.remove(&key);
                }
            }
        }
        let order = self.order();
        match self
            .selected
            .as_ref()
            .and_then(|k| order.iter().position(|o| o == k))
        {
            Some(i) => self.selected_index = i,
            None => {
                let i = self.selected_index.min(order.len().saturating_sub(1));
                self.selected = order.get(i).cloned();
                self.selected_index = i;
            }
        }
        // Screens whose automation has gone close; the rest re-read what they show.
        let known: Vec<String> = self.statuses.iter().map(|s| s.key.clone()).collect();
        let before = self.stack.len();
        self.stack.retain(|s| match s {
            Screen::Automation { key, .. } | Screen::Check { key, .. } => known.contains(key),
            Screen::Run(p) => known.contains(&p.key),
            _ => true,
        });
        if self.stack.len() != before {
            self.say("that automation is no longer listed", UiColor::Warning);
        }
        let keys: Vec<String> = self
            .stack
            .iter()
            .filter_map(|s| match s {
                Screen::Automation { key, .. } => Some(key.clone()),
                Screen::Run(p) => Some(p.key.clone()),
                _ => None,
            })
            .collect();
        for key in keys {
            let runs = host.runs(&key, 20);
            self.records.insert(key, runs);
        }
        let records = &self.records;
        let statuses = &self.statuses;
        for s in self.stack.iter_mut() {
            if let Screen::Run(p) = s {
                // A live run that ends keeps its place: it is now the newest record.
                p.runs = records.get(&p.key).cloned().unwrap_or_default();
                let live = statuses
                    .iter()
                    .any(|s| s.key == p.key && s.running.is_some());
                let n = p.runs.len() + usize::from(live);
                p.selected = p.selected.min(n.saturating_sub(1));
            }
        }
    }

    /// Fetch what drawing needs from the host at this width: the chart, when the graph is shown.
    pub fn prepare(&mut self, host: &mut dyn Host, width: u16) {
        let pane = width.saturating_sub(4);
        let wanted = match self.screen() {
            Screen::Run(p) if p.graph => self.chart_source(p),
            _ => None,
        };
        let Some((chart, progress)) = wanted else {
            return;
        };
        if let Some(Screen::Run(p)) = self.stack.last_mut() {
            let fresh = p
                .chart
                .as_ref()
                .is_some_and(|c| c.width == pane && c.chart == chart && c.progress == progress);
            if !fresh {
                let drawn = host.render_chart(&chart, progress.as_ref(), pane);
                p.chart = Some(ChartCache {
                    width: pane,
                    chart,
                    progress,
                    drawn,
                });
            }
        }
    }

    /// The items of a run page: the live run first, while there is one, then the records.
    pub fn run_items<'a>(&'a self, page: &'a RunPage) -> Vec<RunItem<'a>> {
        let mut out = Vec::new();
        if let Some(s) = self.status(&page.key).filter(|s| s.running.is_some()) {
            out.push(RunItem::Live(s, self.live.get(&page.key)));
        }
        out.extend(page.runs.iter().map(RunItem::Record));
        out
    }

    /// The chart and progress of the run the page shows.
    pub fn chart_source(&self, page: &RunPage) -> Option<(String, Option<RunProgress>)> {
        match self.run_items(page).get(page.selected)? {
            RunItem::Live(_, live) => {
                let p = (*live)?.progress.clone()?;
                Some((p.chart.clone(), Some(p)))
            }
            RunItem::Record(r) => {
                let chart = r.chart.clone()?;
                Some((
                    chart.clone(),
                    Some(RunProgress {
                        chart,
                        steps: r.steps.clone(),
                        taken: r.taken.clone(),
                        notes: r.notes.clone(),
                        message: r.message.clone(),
                    }),
                ))
            }
        }
    }

    /// Every link the shown run published: its message's, then its notes'.
    pub fn run_links(&self, page: &RunPage) -> Vec<RunLink> {
        let (message, notes) = match self.run_items(page).get(page.selected) {
            Some(RunItem::Live(_, Some(l))) => match &l.progress {
                Some(p) => (p.message.clone(), p.notes.clone()),
                None => (None, vec![]),
            },
            Some(RunItem::Record(r)) => (r.message.clone(), r.notes.clone()),
            _ => (None, vec![]),
        };
        let mut out: Vec<RunLink> = message.map(|m| m.links).unwrap_or_default();
        for n in notes {
            for l in n.links {
                if !out.iter().any(|o| o.url == l.url) {
                    out.push(l);
                }
            }
        }
        out
    }

    pub fn say(&mut self, text: impl Into<String>, tone: UiColor) {
        self.flash = Some(Flash {
            text: text.into(),
            tone,
            at: Some(Instant::now()),
        });
    }

    fn answer(&mut self, result: Result<String, String>) {
        match result {
            Ok(t) => self.say(t, UiColor::Success),
            Err(t) => self.say(t, UiColor::Error),
        }
    }

    /// Clear a message that has been up long enough.
    pub fn expire_flash(&mut self, now: Instant) {
        if self
            .flash
            .as_ref()
            .and_then(|f| f.at)
            .is_some_and(|at| now.duration_since(at) >= FLASH_FOR)
        {
            self.flash = None;
        }
    }

    pub fn handle_event(&mut self, host: &mut dyn Host, ev: KeyEvent) -> Effect {
        match Key::from_event(ev) {
            Some(k) => self.handle_key(host, k),
            None => Effect::None,
        }
    }

    pub fn handle_key(&mut self, host: &mut dyn Host, key: Key) -> Effect {
        if key == Key::Quit {
            self.quit = true;
            return Effect::Quit;
        }
        self.flash = None;
        if self.overlay.is_some() {
            return self.overlay_key(host, key);
        }
        match self.screen().clone() {
            Screen::List => self.list_key(host, key),
            Screen::Automation { key: k, index } => self.automation_key(host, key, &k, index),
            Screen::Run(_) => self.run_key(host, key),
            Screen::Check { key: k, .. } => self.check_key(host, key, &k),
            Screen::Help => {
                if matches!(key, Key::Char('q')) {
                    return self.quit();
                }
                self.stack.pop();
                Effect::None
            }
        }
    }

    fn quit(&mut self) -> Effect {
        self.quit = true;
        Effect::Quit
    }

    fn move_selection(&mut self, delta: isize) {
        let order = self.order();
        if order.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|k| order.iter().position(|o| o == k))
            .unwrap_or(0) as isize;
        let next = (at + delta).clamp(0, order.len() as isize - 1) as usize;
        self.selected = Some(order[next].clone());
        self.selected_index = next;
    }

    fn list_key(&mut self, host: &mut dyn Host, key: Key) -> Effect {
        let page = self.page_rows.max(1) as isize;
        match key {
            Key::Esc | Key::Char('q') => return self.quit(),
            Key::Up | Key::Char('k') => self.move_selection(-1),
            Key::Down | Key::Char('j') => self.move_selection(1),
            Key::PageUp => self.move_selection(-page),
            Key::PageDown => self.move_selection(page),
            Key::Home => self.move_selection(-10_000),
            Key::End => self.move_selection(10_000),
            Key::Char('?') => self.stack.push(Screen::Help),
            Key::Char('R') => {
                self.refresh(host);
                self.say("read again", UiColor::Muted);
            }
            Key::Enter => {
                if let Some(k) = self.selected.clone() {
                    self.open_automation(host, &k);
                }
            }
            Key::Char(_) => {
                let Some(s) = self.selected_status().cloned() else {
                    return Effect::None;
                };
                let name = key.name();
                match key_action(&s, &name) {
                    Some(action) => return self.act(host, action, &s.key),
                    None => {
                        if let Some(why) = why_not(&s, &name) {
                            self.say(why, UiColor::Warning);
                        }
                    }
                }
            }
            _ => {}
        }
        Effect::None
    }

    fn open_automation(&mut self, host: &mut dyn Host, key: &str) {
        let runs = host.runs(key, 20);
        self.records.insert(key.to_string(), runs);
        self.stack.push(Screen::Automation {
            key: key.to_string(),
            index: 0,
        });
    }

    fn open_run(&mut self, host: &mut dyn Host, key: &str) {
        let runs = host.runs(key, 20);
        self.records.insert(key.to_string(), runs.clone());
        if let Some(l) = host.live(key) {
            self.live.insert(key.to_string(), l);
        }
        self.stack.push(Screen::Run(Box::new(RunPage {
            key: key.to_string(),
            runs,
            selected: 0,
            graph: false,
            scroll: 0,
            chart: None,
        })));
    }

    /// Do `action` to the automation `key`, from any screen.
    pub fn act(&mut self, host: &mut dyn Host, action: Action, key: &str) -> Effect {
        let name = self.status(key).map(|s| s.name.clone()).unwrap_or_default();
        match action {
            Action::ShowRun | Action::Runs => self.open_run(host, key),
            Action::Check => self.stack.push(Screen::Check {
                key: key.to_string(),
                scroll: 0,
            }),
            Action::Stop => {
                self.overlay = Some(Overlay::Confirm {
                    key: key.to_string(),
                    what: Confirm::Stop,
                })
            }
            Action::Arm => {
                self.overlay = Some(Overlay::Consent {
                    key: key.to_string(),
                    preview: Box::new(host.preview(key)),
                    scroll: 0,
                })
            }
            Action::Agent => {
                let mut items = vec![AgentChoice {
                    cli: None,
                    version: None,
                }];
                items.extend(host.agents().into_iter().map(|(cli, version)| AgentChoice {
                    cli: Some(cli),
                    version,
                }));
                let current = self.status(key).and_then(|s| s.agent.clone());
                let index = items
                    .iter()
                    .position(|i| i.cli.map(|c| c.as_str().to_string()) == current)
                    .unwrap_or(0);
                self.overlay = Some(Overlay::Agents {
                    key: key.to_string(),
                    items,
                    index,
                });
            }
            Action::Edit => match host.edit_path(key) {
                Some(path) => {
                    return Effect::Edit {
                        key: key.to_string(),
                        path,
                    };
                }
                None => self.say(format!("{name} has no file to edit"), UiColor::Warning),
            },
            Action::Run => {
                let r = host.run_now(key);
                self.answer(r);
                self.refresh(host);
            }
            Action::PauseWatch | Action::ResumeWatch => {
                let r = host.pause_watch(key, action == Action::PauseWatch);
                self.answer(r);
                self.refresh(host);
            }
            Action::Disarm => {
                let r = host.disarm(key);
                self.answer(r);
                self.refresh(host);
            }
            Action::Enable | Action::Disable => {
                let r = host.set_enabled(key, action == Action::Enable);
                self.answer(r);
                self.refresh(host);
            }
        }
        Effect::None
    }

    fn automation_key(&mut self, host: &mut dyn Host, key: Key, k: &str, index: usize) -> Effect {
        let Some(s) = self.status(k).cloned() else {
            self.stack.pop();
            return Effect::None;
        };
        let menu = actions_for(&s);
        let set_index = |app: &mut App, i: usize| {
            if let Some(Screen::Automation { index, .. }) = app.stack.last_mut() {
                *index = i.min(menu.len().saturating_sub(1));
            }
        };
        match key {
            Key::Esc => {
                self.stack.pop();
            }
            Key::Char('q') => return self.quit(),
            Key::Char('?') => self.stack.push(Screen::Help),
            Key::Up | Key::Char('k') => set_index(self, index.saturating_sub(1)),
            Key::Down | Key::Char('j') => set_index(self, index + 1),
            Key::Home => set_index(self, 0),
            Key::End => set_index(self, usize::MAX),
            Key::Enter => {
                if let Some(a) = menu.get(index) {
                    return self.act(host, a.action, k);
                }
            }
            Key::Char(_) => {
                let name = key.name();
                if let Some(i) = menu.iter().position(|a| a.key == Some(name.as_str())) {
                    set_index(self, i);
                    return self.act(host, menu[i].action, k);
                }
                match key_action(&s, &name) {
                    Some(action) => return self.act(host, action, k),
                    None => {
                        if let Some(why) = why_not(&s, &name) {
                            self.say(why, UiColor::Warning);
                        }
                    }
                }
            }
            _ => {}
        }
        Effect::None
    }

    fn run_page(&mut self) -> Option<&mut RunPage> {
        match self.stack.last_mut() {
            Some(Screen::Run(p)) => Some(p),
            _ => None,
        }
    }

    fn run_key(&mut self, host: &mut dyn Host, key: Key) -> Effect {
        let Some(Screen::Run(page)) = self.stack.last().cloned() else {
            return Effect::None;
        };
        let n = self.run_items(&page).len();
        let rows = self.page_rows.max(1);
        let max = self.scroll_max.get();
        let live = matches!(
            self.run_items(&page).get(page.selected),
            Some(RunItem::Live(..))
        );
        match key {
            Key::Esc => {
                self.stack.pop();
            }
            Key::Char('q') => return self.quit(),
            Key::Char('?') => self.stack.push(Screen::Help),
            Key::Up | Key::Char('k') => {
                if let Some(p) = self.run_page() {
                    p.selected = p.selected.saturating_sub(1);
                    p.scroll = 0;
                }
            }
            Key::Down | Key::Char('j') => {
                if let Some(p) = self.run_page() {
                    p.selected = (p.selected + 1).min(n.saturating_sub(1));
                    p.scroll = 0;
                }
            }
            Key::PageUp => {
                if let Some(p) = self.run_page() {
                    p.scroll = p.scroll.saturating_sub(rows);
                }
            }
            Key::PageDown => {
                if let Some(p) = self.run_page() {
                    p.scroll = (p.scroll + rows).min(max);
                }
            }
            Key::Home => {
                if let Some(p) = self.run_page() {
                    p.scroll = 0;
                }
            }
            Key::End => {
                if let Some(p) = self.run_page() {
                    p.scroll = max;
                }
            }
            Key::Char('g') => {
                if self.chart_source(&page).is_some() {
                    if let Some(p) = self.run_page() {
                        p.graph = !p.graph;
                        p.scroll = 0;
                    }
                } else {
                    self.say(
                        "g · this run is not tracked on a flowchart",
                        UiColor::Warning,
                    );
                }
            }
            Key::Char('o') => {
                let links = self.run_links(&page);
                match links.len() {
                    0 => self.say("o · this run has no links", UiColor::Warning),
                    1 => return Effect::Open(links[0].url.clone()),
                    _ => self.overlay = Some(Overlay::Links { links, index: 0 }),
                }
            }
            Key::Char('x') => {
                if live {
                    self.overlay = Some(Overlay::Confirm {
                        key: page.key.clone(),
                        what: Confirm::Stop,
                    });
                } else {
                    self.say("x · this run has finished", UiColor::Warning);
                }
            }
            Key::Char('e') => return self.act(host, Action::Edit, &page.key),
            Key::Char('R') => self.refresh(host),
            _ => {}
        }
        Effect::None
    }

    fn check_key(&mut self, host: &mut dyn Host, key: Key, k: &str) -> Effect {
        let rows = self.page_rows.max(1);
        let max = self.scroll_max.get();
        let scroll = |app: &mut App, f: &dyn Fn(usize) -> usize| {
            if let Some(Screen::Check { scroll, .. }) = app.stack.last_mut() {
                *scroll = f(*scroll);
            }
        };
        match key {
            Key::Esc => {
                self.stack.pop();
            }
            Key::Char('q') => return self.quit(),
            Key::Up | Key::Char('k') => scroll(self, &|s| s.saturating_sub(1)),
            Key::Down | Key::Char('j') => scroll(self, &|s| (s + 1).min(max)),
            Key::PageUp => scroll(self, &|s| s.saturating_sub(rows)),
            Key::PageDown => scroll(self, &|s| (s + rows).min(max)),
            Key::End => scroll(self, &|_| max),
            Key::Home => scroll(self, &|_| 0),
            Key::Char('e') => return self.act(host, Action::Edit, k),
            _ => {}
        }
        Effect::None
    }

    fn overlay_key(&mut self, host: &mut dyn Host, key: Key) -> Effect {
        let Some(overlay) = self.overlay.clone() else {
            return Effect::None;
        };
        match overlay {
            Overlay::Consent {
                key: k,
                preview,
                scroll,
            } => match key {
                // Only y says yes: the key that opened the preview must never also answer it.
                Key::Char('y') | Key::Char('Y') if preview.as_ref().is_ok() => {
                    self.overlay = None;
                    let r = host.arm(&k);
                    self.answer(r);
                    self.refresh(host);
                }
                Key::Esc | Key::Char('n') | Key::Char('N') | Key::Char('q') => {
                    self.overlay = None;
                    self.say("not armed", UiColor::Muted);
                }
                Key::Enter | Key::Char(' ') => {
                    let text = match preview.as_ref() {
                        Ok(_) => "enter does not arm · y arms it, esc leaves it off".to_string(),
                        Err(e) => e.clone(),
                    };
                    self.say(text, UiColor::Warning);
                }
                Key::Up | Key::Char('k') => self.set_overlay_scroll(scroll.saturating_sub(1)),
                Key::Down | Key::Char('j') => {
                    self.set_overlay_scroll((scroll + 1).min(self.scroll_max.get()))
                }
                Key::PageUp => self.set_overlay_scroll(scroll.saturating_sub(self.page_rows)),
                Key::PageDown => {
                    self.set_overlay_scroll((scroll + self.page_rows).min(self.scroll_max.get()))
                }
                _ => {}
            },
            Overlay::Confirm { key: k, what } => match key {
                Key::Char('y') | Key::Char('Y') => {
                    self.overlay = None;
                    let r = match what {
                        Confirm::Stop => host.stop(&k),
                    };
                    self.answer(r);
                    self.refresh(host);
                }
                Key::Esc | Key::Char('n') | Key::Char('N') | Key::Char('q') => {
                    self.overlay = None;
                    self.say("still running", UiColor::Muted);
                }
                _ => {}
            },
            Overlay::Agents {
                key: k,
                items,
                index,
            } => match key {
                Key::Up | Key::Char('k') => {
                    self.set_overlay_index(index.saturating_sub(1), items.len())
                }
                Key::Down | Key::Char('j') => self.set_overlay_index(index + 1, items.len()),
                Key::Enter => {
                    self.overlay = None;
                    if let Some(choice) = items.get(index) {
                        let r = host.set_agent(&k, choice.cli);
                        self.answer(r);
                        self.refresh(host);
                    }
                }
                Key::Esc | Key::Char('q') => self.overlay = None,
                _ => {}
            },
            Overlay::Links { links, index } => match key {
                Key::Up | Key::Char('k') => {
                    self.set_overlay_index(index.saturating_sub(1), links.len())
                }
                Key::Down | Key::Char('j') => self.set_overlay_index(index + 1, links.len()),
                Key::Enter => {
                    self.overlay = None;
                    if let Some(l) = links.get(index) {
                        return Effect::Open(l.url.clone());
                    }
                }
                Key::Char(c @ '1'..='9') => {
                    let i = c as usize - '1' as usize;
                    if let Some(l) = links.get(i) {
                        self.overlay = None;
                        return Effect::Open(l.url.clone());
                    }
                }
                Key::Esc | Key::Char('q') => self.overlay = None,
                _ => {}
            },
        }
        Effect::None
    }

    fn set_overlay_scroll(&mut self, to: usize) {
        if let Some(Overlay::Consent { scroll, .. }) = self.overlay.as_mut() {
            *scroll = to;
        }
    }

    fn set_overlay_index(&mut self, to: usize, len: usize) {
        let to = to.min(len.saturating_sub(1));
        match self.overlay.as_mut() {
            Some(Overlay::Agents { index, .. }) | Some(Overlay::Links { index, .. }) => *index = to,
            _ => {}
        }
    }

    /// After a link was handed to the browser, or could not be.
    pub fn after_open(&mut self, url: &str, result: Result<(), String>) {
        match result {
            Ok(()) => self.say(format!("opened {url}"), UiColor::Muted),
            Err(e) => self.say(e, UiColor::Error),
        }
    }

    /// After the editor closed: re-read, and say what the edit did to the automation.
    pub fn after_edit(
        &mut self,
        host: &mut dyn Host,
        key: &str,
        before: Option<Status>,
        editor: Result<(), String>,
    ) {
        self.refresh(host);
        if let Err(e) = editor {
            self.say(e, UiColor::Error);
            return;
        }
        let name = before.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        let Some(after) = self.status(key).cloned() else {
            self.say(format!("{name} is no longer listed"), UiColor::Warning);
            return;
        };
        let was = before.as_ref().map(|s| s.state);
        let (text, tone) = match after.state {
            AutomationState::Invalid => (
                format!(
                    "{} has a problem in its file — {}",
                    after.name,
                    after.reason.clone().unwrap_or_default()
                ),
                UiColor::Error,
            ),
            AutomationState::Off if was == Some(AutomationState::Armed) => (
                format!("{} changed — arm it again to run it", after.name),
                UiColor::Warning,
            ),
            _ if before.as_ref() == Some(&after) => {
                (format!("{} is unchanged", after.name), UiColor::Muted)
            }
            AutomationState::Armed => (format!("{} is armed", after.name), UiColor::Success),
            AutomationState::Off => (format!("{} is off", after.name), UiColor::Warning),
            AutomationState::Disabled => (format!("{} is disabled", after.name), UiColor::Muted),
        };
        self.say(text, tone);
    }
}

/// A key that does not apply to this row says why, in the automation's words.
pub fn why_not(s: &Status, key: &str) -> Option<String> {
    let n = &s.name;
    let broken = s.state == AutomationState::Invalid;
    let why = match key {
        "r" if s.running.is_some() => format!("{n} is already running · s shows it"),
        _ if broken && ["r", "a", "s", "l", "p", "x", "space"].contains(&key) => {
            format!("{n} cannot do that until its file is fixed · e edits it")
        }
        "r" if s.state == AutomationState::Off => {
            format!("{n} is not armed · space shows what it runs, then arms it")
        }
        "r" if s.state == AutomationState::Disabled => {
            format!("{n} is disabled · space enables it")
        }
        "x" => format!("{n} is not running"),
        "p" => format!("{n} has no watch to pause"),
        "s" | "l" => format!("{n} has not run yet"),
        "space" => format!("space has nothing to do for {n}"),
        _ => return None,
    };
    Some(format!("{key} · {why}"))
}
