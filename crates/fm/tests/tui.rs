//! The terminal UI drawn from the real host on a fixture, as text through ratatui's `TestBackend`.

mod common;

use common::Fx;
use fm_model::Host;
use fm_tui::app::App;
use fm_tui::glyphs::Glyphs;
use fm_tui::theme::Theme;
use fm_tui::view;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// The list page as text, one line per row, trailing spaces trimmed.
fn list_page(host: &mut dyn Host, w: u16, h: u16) -> String {
    let now = chrono::Local::now().fixed_offset();
    let mut app = App::load(host, now, Theme::PLAIN, Glyphs::UNICODE);
    app.page_rows = view::body_rows(h as usize);
    app.prepare(host, w);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| view::draw(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..h)
        .map(|y| {
            let line: String = (0..w).map(|x| buf[(x, y)].symbol()).collect();
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_list_page_draws_armed_changed_and_off_automations_from_the_real_host() {
    let fx = Fx::new();
    fx.manual("ready");
    let edited = fx.manual("edited");
    fx.manual("fresh");
    let mut host = fx.host();
    host.arm(&fx.user_key("ready")).unwrap();
    host.arm(&fx.user_key("edited")).unwrap();
    let text = std::fs::read_to_string(&edited).unwrap();
    std::fs::write(&edited, format!("{text}Keep it short.\n")).unwrap();

    let page = list_page(&mut host, 100, 24);
    println!("{page}");
    for name in ["ready", "edited", "fresh"] {
        assert!(page.contains(name), "{page}");
    }
    assert!(page.contains("changed since armed"), "{page}");
}
