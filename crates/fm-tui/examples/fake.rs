//! The UI on `FakeHost`'s demo data: `cargo run -p fm-tui --example fake`.
//!
//! `FM_ASCII=1` draws ASCII only; `NO_COLOR=1` draws no colour; `FM_THEME=light` uses the deeper
//! shades. `e` opens sample files written to the system's temporary folder.

fn main() -> std::io::Result<()> {
    fm_tui::run_fake(fm_tui::TuiOptions::from_env())
}
