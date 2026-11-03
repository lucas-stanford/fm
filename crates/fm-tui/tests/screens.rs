//! Every screen drawn on ratatui's `TestBackend` at 80 and 120 columns, from `FakeHost`.

use fm_model::Host;
use fm_tui::app::{App, Key};
use fm_tui::fake::FakeHost;
use fm_tui::glyphs::Glyphs;
use fm_tui::theme::{SectionTone, Theme};
use fm_tui::view;
use insta::assert_snapshot;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

fn app_on(host: &mut FakeHost) -> App {
    let now = host.now;
    App::load(host, now, Theme::PLAIN, Glyphs::UNICODE)
}

/// The screen as text, one line per row, trailing spaces trimmed. Paths read the same on every OS.
fn draw(app: &mut App, host: &mut FakeHost, w: u16, h: u16) -> String {
    app.page_rows = view::body_rows(h as usize);
    app.prepare(host, w);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| view::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        let mut line = String::new();
        for x in 0..w {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.replace('\\', "/")
}

fn select(app: &mut App, name: &str) {
    let key = app
        .statuses
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.key.clone())
        .unwrap();
    app.selected = Some(key);
}

fn press(app: &mut App, host: &mut FakeHost, keys: &[Key]) {
    for k in keys {
        app.handle_key(host, *k);
    }
}

/// Snapshots at both widths: `<name>_80` and `<name>_120`.
fn both(name: &str, setup: impl Fn(&mut App, &mut FakeHost), h80: u16, h120: u16) {
    for (w, h) in [(80, h80), (120, h120)] {
        let mut host = FakeHost::new();
        let mut app = app_on(&mut host);
        setup(&mut app, &mut host);
        let text = draw(&mut app, &mut host, w, h);
        assert_snapshot!(format!("{name}_{w}"), text);
    }
}

#[test]
fn list_page() {
    both("list", |_, _| {}, 30, 30);
}

#[test]
fn list_page_selected_further_down() {
    both("list_selected", |app, _| select(app, "pr-review"), 24, 24);
}

#[test]
fn list_page_scrolls_with_the_selection() {
    both("list_short", |app, _| select(app, "new-idea"), 16, 16);
}

#[test]
fn list_page_all_quiet() {
    both(
        "list_quiet",
        |app, host| {
            host.keep_only(&[
                "standup-notes",
                "branch-review",
                "pr-review",
                "weekly-cleanup",
            ]);
            app.refresh(host);
        },
        14,
        14,
    );
}

#[test]
fn list_page_nothing_needs_you_but_one_runs() {
    both(
        "list_running",
        |app, host| {
            host.keep_only(&["daily-briefing", "standup-notes"]);
            host.set_watcher("off — no tick for 2h");
            app.refresh(host);
        },
        12,
        12,
    );
}

#[test]
fn list_page_empty() {
    both(
        "list_empty",
        |app, host| {
            host.keep_only(&[]);
            app.refresh(host);
        },
        8,
        8,
    );
}

#[test]
fn list_page_ascii() {
    let mut host = FakeHost::new();
    let now = host.now;
    let mut app = App::load(&mut host, now, Theme::PLAIN, Glyphs::ASCII);
    assert_snapshot!("list_ascii_80", draw(&mut app, &mut host, 80, 30));
}

#[test]
fn automation_page() {
    both(
        "automation_failed",
        |app, host| {
            select(app, "nightly-tests");
            press(app, host, &[Key::Enter]);
        },
        24,
        24,
    );
    both(
        "automation_standup",
        |app, host| {
            select(app, "standup-notes");
            press(app, host, &[Key::Enter, Key::Down, Key::Down]);
        },
        24,
        24,
    );
    both(
        "automation_invalid",
        |app, host| {
            select(app, "broken-cron");
            press(app, host, &[Key::Enter]);
        },
        14,
        14,
    );
}

#[test]
fn run_page_live() {
    both(
        "run_live",
        |app, host| {
            select(app, "daily-briefing");
            press(app, host, &[Key::Char('s')]);
        },
        40,
        40,
    );
}

#[test]
fn run_page_live_graph() {
    both(
        "run_live_graph",
        |app, host| {
            select(app, "daily-briefing");
            press(app, host, &[Key::Char('s'), Key::Char('g')]);
        },
        40,
        40,
    );
}

#[test]
fn run_page_finished_with_notes_and_message() {
    both(
        "run_record",
        |app, host| {
            select(app, "daily-briefing");
            press(app, host, &[Key::Char('s'), Key::Down]);
        },
        44,
        44,
    );
}

#[test]
fn run_page_graph_falls_back_to_steps() {
    both(
        "run_fallback",
        |app, host| {
            select(app, "nightly-tests");
            press(app, host, &[Key::Char('l'), Key::Char('g')]);
        },
        30,
        30,
    );
}

#[test]
fn run_page_graph_too_wide_falls_back() {
    let mut host = FakeHost::new();
    let mut app = app_on(&mut host);
    select(&mut app, "daily-briefing");
    press(&mut app, &mut host, &[Key::Char('s'), Key::Char('g')]);
    let text = draw(&mut app, &mut host, 60, 30);
    assert!(
        text.contains("the flowchart is 65 columns wide and the pane is 56"),
        "{text}"
    );
    assert!(text.contains("Draft summary"), "{text}");
}

#[test]
fn run_page_live_shows_the_answer_once() {
    let mut host = FakeHost::new();
    let key = host.key_of("daily-briefing");
    let said = "Briefing ready: 14 threads, 3 need a reply.";
    let ts = host.now.timestamp_millis() as u64;
    host.push_live(
        &key,
        fm_model::LiveEvent::Message {
            text: said.into(),
            ts,
        },
    );
    host.push_live(
        &key,
        fm_model::LiveEvent::Final {
            text: said.into(),
            ts,
        },
    );
    let mut app = app_on(&mut host);
    select(&mut app, "daily-briefing");
    press(&mut app, &mut host, &[Key::Char('s')]);
    let text = draw(&mut app, &mut host, 120, 40);
    assert_eq!(text.matches(said).count(), 1, "{text}");
    assert!(text.contains(&format!("answer · {said}")), "{text}");
}

#[test]
fn run_page_never_run() {
    both(
        "run_none",
        |app, host| {
            select(app, "docs-sync");
            let key = app.selected.clone().unwrap();
            app.act(host, fm_tui::triage::Action::Runs, &key);
        },
        10,
        10,
    );
}

#[test]
fn consent_preview() {
    both(
        "consent_new",
        |app, host| {
            select(app, "new-idea");
            press(app, host, &[Key::Char(' ')]);
        },
        28,
        28,
    );
    both(
        "consent_changed",
        |app, host| {
            select(app, "release-notes");
            press(app, host, &[Key::Char(' ')]);
        },
        24,
        24,
    );
}

#[test]
fn consent_preview_warns_about_pa_first() {
    let mut host = FakeHost::new();
    let mut app = app_on(&mut host);
    let key = host.key_of("standup-notes");
    host.disarm(&key).unwrap();
    app.refresh(&mut host);
    select(&mut app, "standup-notes");
    press(&mut app, &mut host, &[Key::Char(' ')]);
    let text = draw(&mut app, &mut host, 80, 24);
    assert_snapshot!("consent_pa_80", text);
    let body: Vec<&str> = text.lines().skip(2).filter(|l| !l.is_empty()).collect();
    assert!(body[0].contains("also armed in pi-automations"), "{text}");
}

#[test]
fn consent_preview_refused() {
    let mut host = FakeHost::new();
    let mut app = app_on(&mut host);
    let key = host.key_of("broken-cron");
    app.overlay = Some(fm_tui::app::Overlay::Consent {
        key: key.clone(),
        preview: Box::new(host.preview(&key)),
        scroll: 0,
    });
    assert_snapshot!("consent_refused_80", draw(&mut app, &mut host, 80, 10));
}

#[test]
fn stop_question() {
    both(
        "confirm_stop",
        |app, host| {
            select(app, "daily-briefing");
            press(app, host, &[Key::Char('x')]);
        },
        10,
        10,
    );
}

#[test]
fn agent_picker() {
    both(
        "agents",
        |app, host| {
            select(app, "standup-notes");
            press(app, host, &[Key::Char('a')]);
        },
        14,
        14,
    );
}

#[test]
fn link_picker() {
    both(
        "links",
        |app, host| {
            select(app, "daily-briefing");
            press(app, host, &[Key::Char('s'), Key::Down, Key::Char('o')]);
        },
        10,
        10,
    );
}

#[test]
fn check_page() {
    both(
        "check",
        |app, host| {
            select(app, "pr-review");
            press(app, host, &[Key::Char('c')]);
        },
        30,
        30,
    );
    both(
        "check_running",
        |app, host| {
            select(app, "daily-briefing");
            press(app, host, &[Key::Char('c')]);
        },
        30,
        30,
    );
}

#[test]
fn help_page() {
    both(
        "help",
        |app, host| press(app, host, &[Key::Char('?')]),
        34,
        34,
    );
}

#[test]
fn flash_replaces_the_hint_line() {
    let mut host = FakeHost::new();
    let mut app = app_on(&mut host);
    select(&mut app, "broken-cron");
    press(&mut app, &mut host, &[Key::Char('r')]);
    let text = draw(&mut app, &mut host, 80, 30);
    let last = text.lines().last().unwrap();
    assert_eq!(
        last,
        "r · broken-cron cannot do that until its file is fixed · e edits it"
    );
}

#[test]
fn scrolling_stops_at_the_end_and_comes_straight_back() {
    let mut host = FakeHost::new();
    let mut app = app_on(&mut host);
    select(&mut app, "daily-briefing");
    press(&mut app, &mut host, &[Key::Char('s'), Key::Down]);
    let top = draw(&mut app, &mut host, 80, 20);
    press(&mut app, &mut host, &[Key::End]);
    let end = draw(&mut app, &mut host, 80, 20);
    assert!(end.contains("run 20261028-080001-1a2b"), "{end}");
    assert!(end.contains("↑ more above"), "{end}");
    // Many more downs change nothing; one page up moves at once.
    press(&mut app, &mut host, &[Key::PageDown, Key::PageDown]);
    assert_eq!(draw(&mut app, &mut host, 80, 20), end);
    press(&mut app, &mut host, &[Key::PageUp]);
    let up = draw(&mut app, &mut host, 80, 20);
    assert_ne!(up, end);
    press(&mut app, &mut host, &[Key::Home]);
    assert_eq!(draw(&mut app, &mut host, 80, 20), top);
}

/// Every screen at every small size: nothing panics, and every row fits.
#[test]
fn tiny_sizes_never_panic() {
    type Setup = fn(&mut App, &mut FakeHost);
    let setups: Vec<(&str, Setup)> = vec![
        ("list", |_, _| {}),
        ("automation", |a, h| {
            select(a, "nightly-tests");
            press(a, h, &[Key::Enter]);
        }),
        ("run", |a, h| {
            select(a, "daily-briefing");
            press(a, h, &[Key::Char('s')]);
        }),
        ("graph", |a, h| {
            select(a, "daily-briefing");
            press(a, h, &[Key::Char('s'), Key::Char('g')]);
        }),
        ("record", |a, h| {
            select(a, "daily-briefing");
            press(
                a,
                h,
                &[Key::Char('s'), Key::Down, Key::Char('g'), Key::PageDown],
            );
        }),
        ("consent", |a, h| {
            select(a, "new-idea");
            press(a, h, &[Key::Char(' '), Key::Down, Key::Down]);
        }),
        ("confirm", |a, h| {
            select(a, "daily-briefing");
            press(a, h, &[Key::Char('x')]);
        }),
        ("agents", |a, h| {
            select(a, "standup-notes");
            press(a, h, &[Key::Char('a')]);
        }),
        ("links", |a, h| {
            select(a, "daily-briefing");
            press(a, h, &[Key::Char('s'), Key::Down, Key::Char('o')]);
        }),
        ("check", |a, h| {
            select(a, "daily-briefing");
            press(a, h, &[Key::Char('c'), Key::PageDown]);
        }),
        ("help", |a, h| press(a, h, &[Key::Char('?')])),
    ];
    for (name, setup) in setups {
        for (w, h) in [
            (40, 10),
            (20, 5),
            (1, 1),
            (0, 0),
            (10, 3),
            (40, 2),
            (200, 60),
        ] {
            let mut host = FakeHost::new();
            let mut app = app_on(&mut host);
            setup(&mut app, &mut host);
            let text = draw(&mut app, &mut host, w, h);
            assert_eq!(text.lines().count(), h as usize, "{name} {w}x{h}");
        }
    }
}

#[test]
fn tiny_40x10_list() {
    let mut host = FakeHost::new();
    let mut app = app_on(&mut host);
    assert_snapshot!("list_40", draw(&mut app, &mut host, 40, 10));
}

#[test]
fn section_headers_are_pastel_in_truecolor() {
    let mut host = FakeHost::new();
    let now = host.now;
    let theme = Theme::detect(&|k| (k == "COLORTERM").then(|| "truecolor".to_string()));
    let mut app = App::load(&mut host, now, theme, Glyphs::UNICODE);
    app.prepare(&mut host, 80);
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    term.draw(|f| view::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();
    let row_of = |text: &str| {
        (0..30u16)
            .find(|y| {
                let line: String = (0..80u16)
                    .map(|x| buf[(x, *y)].symbol().to_string())
                    .collect();
                line.trim() == text
            })
            .unwrap_or_else(|| panic!("no row {text}"))
    };
    for (title, tone) in [
        ("Needs you", SectionTone::Needs),
        ("Running now", SectionTone::Running),
        ("Coming up", SectionTone::Coming),
        ("Watching", SectionTone::Watching),
        ("Off and disabled", SectionTone::Off),
    ] {
        let y = row_of(title);
        let cell = &buf[(2, y)];
        let (dark, _) = fm_tui::theme::section_pastel(tone);
        assert_eq!(cell.fg, Color::Rgb(dark.0, dark.1, dark.2), "{title}");
        assert!(cell.modifier.contains(ratatui::style::Modifier::BOLD));
    }
}

#[test]
fn no_color_draws_no_colour_anywhere() {
    let mut host = FakeHost::new();
    let now = host.now;
    let theme = Theme::detect(&|k| match k {
        "NO_COLOR" => Some("1".into()),
        "COLORTERM" => Some("truecolor".into()),
        _ => None,
    });
    let mut app = App::load(&mut host, now, theme, Glyphs::UNICODE);
    select(&mut app, "daily-briefing");
    for keys in [vec![], vec![Key::Char('s'), Key::Char('g')]] {
        press(&mut app, &mut host, &keys);
        app.prepare(&mut host, 120);
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        term.draw(|f| view::draw(f, &app)).unwrap();
        let buf = term.backend().buffer();
        for cell in buf.content() {
            assert_eq!(cell.fg, Color::Reset);
            assert_eq!(cell.bg, Color::Reset);
        }
    }
}
