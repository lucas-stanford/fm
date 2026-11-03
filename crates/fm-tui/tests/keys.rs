//! The state machine driven key by key against `FakeHost`: nothing irreversible happens without its
//! confirm key, and every action reaches the host.

use std::io;

use fm_model::{AutomationState, CliId, Host};
use fm_tui::app::{App, Effect, Key, Overlay, Screen};
use fm_tui::editor::{self, Suspend};
use fm_tui::fake::FakeHost;
use fm_tui::glyphs::Glyphs;
use fm_tui::theme::{Theme, UiColor};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

fn setup() -> (FakeHost, App) {
    let mut host = FakeHost::new();
    let now = host.now;
    let app = App::load(&mut host, now, Theme::PLAIN, Glyphs::UNICODE);
    (host, app)
}

fn select(app: &mut App, host: &FakeHost, name: &str) -> String {
    let key = host.key_of(name);
    app.selected = Some(key.clone());
    key
}

fn press(app: &mut App, host: &mut FakeHost, keys: &[Key]) -> Effect {
    let mut last = Effect::None;
    for k in keys {
        last = app.handle_key(host, *k);
    }
    last
}

fn flash(app: &App) -> String {
    app.flash
        .as_ref()
        .map(|f| f.text.clone())
        .unwrap_or_default()
}

#[test]
fn arming_needs_the_preview_and_y() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "new-idea");
    press(&mut app, &mut host, &[Key::Char(' ')]);
    assert!(matches!(app.overlay, Some(Overlay::Consent { .. })));
    assert!(host.calls.is_empty(), "the preview alone arms nothing");

    // Enter alone never arms, nor does space again.
    press(&mut app, &mut host, &[Key::Enter, Key::Char(' ')]);
    assert!(host.calls.is_empty());
    assert!(matches!(app.overlay, Some(Overlay::Consent { .. })));
    assert!(flash(&app).contains("y arms it"), "{}", flash(&app));
    assert_eq!(host.status(&key).unwrap().state, AutomationState::Off);

    press(&mut app, &mut host, &[Key::Char('y')]);
    assert_eq!(host.calls, vec![format!("arm {key}")]);
    assert!(app.overlay.is_none());
    assert_eq!(app.status(&key).unwrap().state, AutomationState::Armed);
    assert_eq!(flash(&app), "new-idea is armed");
}

#[test]
fn esc_leaves_it_off() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "release-notes");
    press(&mut app, &mut host, &[Key::Char(' '), Key::Esc]);
    assert!(app.overlay.is_none());
    assert!(host.calls.is_empty());
    assert_eq!(host.status(&key).unwrap().state, AutomationState::Off);
    assert_eq!(flash(&app), "not armed");
}

#[test]
fn a_refused_preview_cannot_be_armed() {
    let (mut host, mut app) = setup();
    let key = host.key_of("broken-cron");
    app.overlay = Some(Overlay::Consent {
        key: key.clone(),
        preview: Box::new(host.preview(&key)),
        scroll: 0,
    });
    press(&mut app, &mut host, &[Key::Char('y')]);
    assert!(host.calls.is_empty());
}

#[test]
fn stopping_asks_first() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "daily-briefing");
    press(&mut app, &mut host, &[Key::Char('x')]);
    assert!(matches!(app.overlay, Some(Overlay::Confirm { .. })));
    assert!(host.calls.is_empty());

    press(&mut app, &mut host, &[Key::Enter, Key::Char('x')]);
    assert!(host.calls.is_empty(), "only y stops");

    press(&mut app, &mut host, &[Key::Char('n')]);
    assert!(app.overlay.is_none());
    assert!(host.calls.is_empty());
    assert!(app.status(&key).unwrap().running.is_some());

    press(&mut app, &mut host, &[Key::Char('x'), Key::Char('y')]);
    assert_eq!(host.calls, vec![format!("stop {key}")]);
    assert!(app.status(&key).unwrap().running.is_none());
    assert_eq!(flash(&app), "daily-briefing was stopped");
}

#[test]
fn stop_from_the_run_page_asks_too() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "daily-briefing");
    press(&mut app, &mut host, &[Key::Char('s')]);
    assert!(matches!(app.screen(), Screen::Run(_)));
    press(&mut app, &mut host, &[Key::Char('x')]);
    assert!(host.calls.is_empty());
    press(&mut app, &mut host, &[Key::Char('y')]);
    assert_eq!(host.calls, vec![format!("stop {key}")]);
    // The run ended while its page was open: the page stays, now on the aborted record.
    match app.screen() {
        Screen::Run(p) => {
            let items = app.run_items(p);
            assert!(
                matches!(items[p.selected], fm_tui::app::RunItem::Record(r) if r.outcome == fm_model::Outcome::Aborted)
            );
        }
        other => panic!("{other:?}"),
    }
    // A finished run cannot be stopped.
    press(&mut app, &mut host, &[Key::Char('x')]);
    assert!(app.overlay.is_none());
    assert_eq!(flash(&app), "x · this run has finished");
}

#[test]
fn agent_picker_sets_and_clears_the_override() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "standup-notes");
    press(&mut app, &mut host, &[Key::Char('a')]);
    let Some(Overlay::Agents { items, index, .. }) = app.overlay.clone() else {
        panic!("no picker");
    };
    // It opens on the agent in use.
    assert_eq!(items[index].cli, Some(CliId::Copilot));
    assert_eq!(items[0].cli, None, "use default comes first");

    press(&mut app, &mut host, &[Key::Down, Key::Enter]);
    assert_eq!(host.calls, vec![format!("set_agent {key} codex")]);
    assert_eq!(app.status(&key).unwrap().agent.as_deref(), Some("codex"));
    assert!(flash(&app).contains("Codex CLI"));

    press(&mut app, &mut host, &[Key::Char('a'), Key::Home]);
    for _ in 0..10 {
        press(&mut app, &mut host, &[Key::Up]);
    }
    press(&mut app, &mut host, &[Key::Enter]);
    assert_eq!(
        host.calls.last().unwrap(),
        &format!("set_agent {key} default")
    );
    assert_eq!(app.status(&key).unwrap().agent.as_deref(), Some("claude"));
}

#[test]
fn agent_picker_esc_changes_nothing() {
    let (mut host, mut app) = setup();
    select(&mut app, &host, "standup-notes");
    press(&mut app, &mut host, &[Key::Char('a'), Key::Down, Key::Esc]);
    assert!(app.overlay.is_none());
    assert!(host.calls.is_empty());
}

#[derive(Default)]
struct Screenshot {
    log: Vec<&'static str>,
}

impl Suspend for Screenshot {
    fn suspend(&mut self) -> io::Result<()> {
        self.log.push("suspend");
        Ok(())
    }
    fn resume(&mut self) -> io::Result<()> {
        self.log.push("resume");
        Ok(())
    }
}

#[test]
fn editing_suspends_runs_the_editor_and_shows_the_new_state() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "standup-notes");
    let Effect::Edit { key: k, path } = press(&mut app, &mut host, &[Key::Char('e')]) else {
        panic!("e did not ask for the editor");
    };
    assert_eq!(k, key);
    assert!(path.ends_with("AUTOMATION.md"));

    // A stand-in editor: what a real host notices after the file changed.
    let before = app.status(&key).cloned();
    let mut screen = Screenshot::default();
    let mut edited = vec![];
    let answer = editor::with_terminal_released(&mut screen, &mut || {
        edited.push(path.clone());
        Ok(())
    })
    .unwrap();
    host.mark_changed(&key, "AUTOMATION.md");
    app.after_edit(&mut host, &key, before, answer);

    assert_eq!(screen.log, vec!["suspend", "resume"]);
    assert_eq!(edited, vec![path]);
    assert_eq!(app.status(&key).unwrap().state, AutomationState::Off);
    assert_eq!(
        flash(&app),
        "standup-notes changed — arm it again to run it"
    );
    // It is now in Needs you, and space arms it after a preview.
    press(&mut app, &mut host, &[Key::Char(' ')]);
    assert!(matches!(app.overlay, Some(Overlay::Consent { .. })));
}

#[test]
fn editing_that_breaks_the_file_says_why() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "branch-review");
    let before = app.status(&key).cloned();
    host.mark_invalid(&key, "name: must match the folder name — rename one");
    app.after_edit(&mut host, &key, before, Ok(()));
    let f = app.flash.clone().unwrap();
    assert_eq!(f.tone, UiColor::Error);
    assert!(
        f.text.contains("name: must match the folder name"),
        "{}",
        f.text
    );
}

#[test]
fn an_editor_that_fails_is_reported_and_the_screen_comes_back() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "branch-review");
    let before = app.status(&key).cloned();
    let mut screen = Screenshot::default();
    let answer = editor::with_terminal_released(&mut screen, &mut || {
        editor::launch(
            &[vec!["fm-tui-no-such-editor".into()]],
            std::path::Path::new("x"),
        )
    })
    .unwrap();
    app.after_edit(&mut host, &key, before, answer);
    assert_eq!(screen.log, vec!["suspend", "resume"]);
    assert!(
        flash(&app).starts_with("no editor found"),
        "{}",
        flash(&app)
    );
}

#[test]
fn unchanged_after_edit() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "branch-review");
    let before = app.status(&key).cloned();
    app.after_edit(&mut host, &key, before, Ok(()));
    assert_eq!(flash(&app), "branch-review is unchanged");
}

#[test]
fn run_now_moves_it_to_running_now() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "standup-notes");
    press(&mut app, &mut host, &[Key::Char('r')]);
    assert_eq!(host.calls, vec![format!("run_now {key}")]);
    let entry = app.triage.order().find(|e| e.key == key).unwrap();
    assert_eq!(entry.section, fm_tui::triage::Section::Running);
    assert_eq!(
        app.selected.as_deref(),
        Some(key.as_str()),
        "the cursor follows it"
    );
    // A running automation is not run twice.
    press(&mut app, &mut host, &[Key::Char('r')]);
    assert_eq!(host.calls.len(), 1);
    assert_eq!(
        flash(&app),
        "r · standup-notes is already running · s shows it"
    );
}

#[test]
fn keys_that_do_not_apply_say_why_and_do_nothing() {
    let (mut host, mut app) = setup();
    select(&mut app, &host, "broken-cron");
    for k in ['r', 'x', 'p', ' ', 'a'] {
        press(&mut app, &mut host, &[Key::Char(k)]);
        assert!(!flash(&app).is_empty(), "{k}");
    }
    assert!(host.calls.is_empty());
    select(&mut app, &host, "branch-review");
    press(&mut app, &mut host, &[Key::Char('p')]);
    assert_eq!(flash(&app), "p · branch-review has no watch to pause");
    press(&mut app, &mut host, &[Key::Char('x')]);
    assert_eq!(flash(&app), "x · branch-review is not running");
}

#[test]
fn pause_and_resume_a_watch() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "pr-review");
    press(&mut app, &mut host, &[Key::Char('p')]);
    assert!(app.status(&key).unwrap().watch_paused_at.is_some());
    press(&mut app, &mut host, &[Key::Char('p')]);
    assert!(app.status(&key).unwrap().watch_paused_at.is_none());
    assert_eq!(
        host.calls,
        vec![
            format!("pause_watch {key} true"),
            format!("pause_watch {key} false")
        ]
    );
}

#[test]
fn space_disables_enables_and_disarms() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "branch-review");
    press(&mut app, &mut host, &[Key::Char(' ')]);
    assert_eq!(app.status(&key).unwrap().state, AutomationState::Disabled);
    press(&mut app, &mut host, &[Key::Char(' ')]);
    assert_eq!(app.status(&key).unwrap().state, AutomationState::Armed);
    // Disarm is on the automation page, without a key of its own for user scope.
    press(&mut app, &mut host, &[Key::Enter, Key::End, Key::Enter]);
    assert_eq!(host.calls.last().unwrap(), &format!("disarm {key}"));
    assert_eq!(app.status(&key).unwrap().state, AutomationState::Off);
}

#[test]
fn automation_page_menu_and_keys() {
    let (mut host, mut app) = setup();
    let key = select(&mut app, &host, "nightly-tests");
    press(&mut app, &mut host, &[Key::Enter]);
    assert!(matches!(app.screen(), Screen::Automation { .. }));
    // Its first item is "run it again now".
    press(&mut app, &mut host, &[Key::Enter]);
    assert_eq!(host.calls, vec![format!("run_now {key}")]);
    // c opens the check page from it; esc goes back one page at a time.
    press(&mut app, &mut host, &[Key::Char('c')]);
    assert!(matches!(app.screen(), Screen::Check { .. }));
    press(&mut app, &mut host, &[Key::Esc]);
    assert!(matches!(app.screen(), Screen::Automation { .. }));
    press(&mut app, &mut host, &[Key::Esc]);
    assert!(matches!(app.screen(), Screen::List));
}

#[test]
fn run_page_graph_links_and_history() {
    let (mut host, mut app) = setup();
    select(&mut app, &host, "daily-briefing");
    press(&mut app, &mut host, &[Key::Char('s'), Key::Char('g')]);
    let Screen::Run(p) = app.screen().clone() else {
        panic!()
    };
    assert!(p.graph);
    app.prepare(&mut host, 100);
    let Screen::Run(p) = app.screen().clone() else {
        panic!()
    };
    assert!(p.chart.as_ref().unwrap().drawn.is_ok());
    press(&mut app, &mut host, &[Key::Char('g')]);
    let Screen::Run(p) = app.screen().clone() else {
        panic!()
    };
    assert!(!p.graph);

    // The live run has one link: o opens it straight away.
    let effect = press(&mut app, &mut host, &[Key::Char('o')]);
    assert_eq!(
        effect,
        Effect::Open("https://calendar.example.com/e/5521".into())
    );

    // Yesterday's run has two (the same url twice counts once): o asks which.
    press(&mut app, &mut host, &[Key::Down, Key::Char('o')]);
    let Some(Overlay::Links { links, .. }) = app.overlay.clone() else {
        panic!("no links")
    };
    assert_eq!(links.len(), 2);
    let effect = press(&mut app, &mut host, &[Key::Char('2')]);
    assert_eq!(
        effect,
        Effect::Open("https://calendar.example.com/e/5521".into())
    );
    assert!(app.overlay.is_none());

    // A run with no chart says so.
    press(&mut app, &mut host, &[Key::Down, Key::Char('g')]);
    assert_eq!(flash(&app), "g · this run is not tracked on a flowchart");
    press(&mut app, &mut host, &[Key::Esc]);
    assert!(matches!(app.screen(), Screen::List));
}

#[test]
fn opening_a_link_reports_back() {
    let (_, mut app) = setup();
    app.after_open("https://x.example", Err("xdg-open could not start".into()));
    assert_eq!(flash(&app), "xdg-open could not start");
}

#[test]
fn moving_and_quitting() {
    let (mut host, mut app) = setup();
    let first = app.selected.clone();
    press(&mut app, &mut host, &[Key::Down, Key::Down]);
    assert_ne!(app.selected, first);
    press(&mut app, &mut host, &[Key::Home]);
    assert_eq!(app.selected, first);
    press(&mut app, &mut host, &[Key::End]);
    assert_eq!(app.selected, Some(host.key_of("weekly-cleanup")));
    press(&mut app, &mut host, &[Key::Char('?')]);
    assert!(matches!(app.screen(), Screen::Help));
    press(&mut app, &mut host, &[Key::Esc]);
    assert!(matches!(app.screen(), Screen::List));
    assert_eq!(press(&mut app, &mut host, &[Key::Char('q')]), Effect::Quit);
    assert!(app.quit);
}

#[test]
fn crossterm_events_map_to_keys() {
    let ev = |code, mods| KeyEvent::new(code, mods);
    assert_eq!(
        Key::from_event(ev(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(Key::Quit)
    );
    assert_eq!(
        Key::from_event(ev(KeyCode::Char('r'), KeyModifiers::NONE)),
        Some(Key::Char('r'))
    );
    assert_eq!(Key::from_event(ev(KeyCode::F(1), KeyModifiers::NONE)), None);
    // Windows reports releases too: they must not act twice.
    let mut release = ev(KeyCode::Char('y'), KeyModifiers::NONE);
    release.kind = KeyEventKind::Release;
    assert_eq!(Key::from_event(release), None);
    let (mut host, mut app) = setup();
    select(&mut app, &host, "daily-briefing");
    let mut x = ev(KeyCode::Char('x'), KeyModifiers::NONE);
    app.handle_event(&mut host, x);
    x.kind = KeyEventKind::Release;
    app.handle_event(&mut host, x);
    assert!(matches!(app.overlay, Some(Overlay::Confirm { .. })));
    assert!(host.calls.is_empty());
}

#[test]
fn refresh_cadence_follows_running() {
    use std::time::Duration;
    let (mut host, mut app) = setup();
    let busy = Duration::from_secs(1);
    let idle = Duration::from_secs(10);
    assert_eq!(app.refresh_every(idle, busy), busy);
    host.stop(&host.key_of("daily-briefing")).unwrap();
    app.refresh(&mut host);
    assert_eq!(app.refresh_every(idle, busy), idle);
}

#[test]
fn messages_expire() {
    let (_, mut app) = setup();
    app.say("hello", UiColor::Muted);
    let at = app.flash.as_ref().unwrap().at.unwrap();
    app.expire_flash(at + std::time::Duration::from_secs(1));
    assert!(app.flash.is_some());
    app.expire_flash(at + fm_tui::app::FLASH_FOR);
    assert!(app.flash.is_none());
}

#[test]
fn a_vanished_automation_closes_its_pages() {
    let (mut host, mut app) = setup();
    select(&mut app, &host, "pr-review");
    press(&mut app, &mut host, &[Key::Enter]);
    host.keep_only(&["standup-notes"]);
    app.refresh(&mut host);
    assert!(matches!(app.screen(), Screen::List));
    assert_eq!(flash(&app), "that automation is no longer listed");
    assert_eq!(app.selected, Some(host.key_of("standup-notes")));
}
