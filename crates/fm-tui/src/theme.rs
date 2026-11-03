//! Colour. Pastel section headers as on pi-automations' `/automations` page (`theme.ts`):
//! truecolor when the terminal says so, else the 256-colour cube; deeper shades on a light
//! background; plain bold when `NO_COLOR` is set.

use ratatui::style::{Color, Modifier, Style};

/// How much colour the terminal can draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
    /// `NO_COLOR`: bold and plain text only.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Background {
    #[default]
    Dark,
    Light,
}

/// The sections of the list page, each with its own soft colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SectionTone {
    Needs,
    Running,
    Coming,
    Watching,
    Off,
}

/// The colour names the screens use. Text is the terminal's own foreground.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiColor {
    Text,
    Accent,
    Success,
    Warning,
    Error,
    Muted,
}

type Rgb = (u8, u8, u8);

/// `(dark, light)` per section, exactly pi-automations' `SECTION_PASTEL`.
pub fn section_pastel(tone: SectionTone) -> (Rgb, Rgb) {
    match tone {
        SectionTone::Needs => ((244, 167, 185), (180, 70, 106)),
        SectionTone::Running => ((156, 203, 247), (47, 111, 168)),
        SectionTone::Coming => ((195, 177, 225), (107, 79, 160)),
        SectionTone::Watching => ((168, 230, 207), (46, 139, 106)),
        SectionTone::Off => ((200, 194, 180), (122, 116, 102)),
    }
}

fn ui_rgb(color: UiColor) -> Option<(Rgb, Rgb)> {
    Some(match color {
        UiColor::Text => return None,
        UiColor::Accent => ((156, 203, 247), (47, 111, 168)),
        UiColor::Success => ((150, 215, 160), (40, 125, 60)),
        UiColor::Warning => ((240, 200, 120), (150, 95, 0)),
        UiColor::Error => ((240, 128, 128), (180, 40, 40)),
        UiColor::Muted => ((140, 140, 150), (110, 110, 120)),
    })
}

/// The nearest colour in xterm's 6×6×6 cube.
pub fn ansi256(r: u8, g: u8, b: u8) -> u8 {
    let q = |x: u8| ((x as f32 / 255.0) * 5.0).round() as u8;
    16 + 36 * q(r) + 6 * q(g) + q(b)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub mode: ColorMode,
    pub background: Background,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            mode: ColorMode::TrueColor,
            background: Background::Dark,
        }
    }
}

impl Theme {
    pub const PLAIN: Theme = Theme {
        mode: ColorMode::None,
        background: Background::Dark,
    };

    /// Read the environment through `env`, so tests never touch the real one.
    pub fn detect(env: &dyn Fn(&str) -> Option<String>) -> Theme {
        Theme {
            mode: detect_mode(env),
            background: detect_background(env),
        }
    }

    pub fn from_env() -> Theme {
        Theme::detect(&|k| std::env::var(k).ok())
    }

    fn rgb(&self, pair: (Rgb, Rgb)) -> Option<Color> {
        let (r, g, b) = match self.background {
            Background::Dark => pair.0,
            Background::Light => pair.1,
        };
        match self.mode {
            ColorMode::TrueColor => Some(Color::Rgb(r, g, b)),
            ColorMode::Ansi256 => Some(Color::Indexed(ansi256(r, g, b))),
            ColorMode::None => None,
        }
    }

    pub fn fg(&self, color: UiColor) -> Style {
        match ui_rgb(color).and_then(|pair| self.rgb(pair)) {
            Some(c) => Style::default().fg(c),
            None => Style::default(),
        }
    }

    pub fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    /// A section heading: bold, in its pastel when the terminal can draw colour.
    pub fn section(&self, tone: SectionTone) -> Style {
        let bold = self.bold();
        match self.rgb(section_pastel(tone)) {
            Some(c) => bold.fg(c),
            None => bold,
        }
    }

    /// The selected row: a quiet background where there is colour, bold alone otherwise.
    pub fn selected(&self) -> Style {
        let pair = ((48, 52, 64), (226, 230, 238));
        match self.rgb(pair) {
            Some(c) => Style::default().bg(c).add_modifier(Modifier::BOLD),
            None => self.bold(),
        }
    }
}

fn detect_mode(env: &dyn Fn(&str) -> Option<String>) -> ColorMode {
    // https://no-color.org: present and not empty.
    if env("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return ColorMode::None;
    }
    if env("TERM").as_deref() == Some("dumb") {
        return ColorMode::None;
    }
    let colorterm = env("COLORTERM").unwrap_or_default().to_ascii_lowercase();
    if colorterm == "truecolor" || colorterm == "24bit" {
        return ColorMode::TrueColor;
    }
    // Windows Terminal draws 24-bit colour and says so only with this variable.
    if env("WT_SESSION").is_some() {
        return ColorMode::TrueColor;
    }
    ColorMode::Ansi256
}

fn detect_background(env: &dyn Fn(&str) -> Option<String>) -> Background {
    match env("FM_THEME").as_deref() {
        Some("light") => return Background::Light,
        Some("dark") => return Background::Dark,
        _ => {}
    }
    // `COLORFGBG=15;0`: foreground;background, as rxvt, Konsole and iTerm set it.
    if let Some(bg) =
        env("COLORFGBG").and_then(|v| v.rsplit(';').next().and_then(|b| b.parse::<u8>().ok()))
    {
        return if bg == 7 || bg == 15 || (bg >= 250) {
            Background::Light
        } else {
            Background::Dark
        };
    }
    Background::Dark
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn no_color_wins() {
        let t = Theme::detect(&env(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")]));
        assert_eq!(t.mode, ColorMode::None);
        assert_eq!(
            t.section(SectionTone::Needs),
            Style::default().add_modifier(Modifier::BOLD)
        );
        assert_eq!(t.fg(UiColor::Error), Style::default());
    }

    #[test]
    fn empty_no_color_is_ignored() {
        let t = Theme::detect(&env(&[("NO_COLOR", ""), ("COLORTERM", "24bit")]));
        assert_eq!(t.mode, ColorMode::TrueColor);
    }

    #[test]
    fn truecolor_section_is_pa_pastel() {
        let t = Theme::detect(&env(&[("COLORTERM", "truecolor")]));
        assert_eq!(
            t.section(SectionTone::Needs).fg,
            Some(Color::Rgb(244, 167, 185))
        );
        assert_eq!(
            t.section(SectionTone::Running).fg,
            Some(Color::Rgb(156, 203, 247))
        );
        assert_eq!(
            t.section(SectionTone::Coming).fg,
            Some(Color::Rgb(195, 177, 225))
        );
        assert_eq!(
            t.section(SectionTone::Watching).fg,
            Some(Color::Rgb(168, 230, 207))
        );
        assert_eq!(
            t.section(SectionTone::Off).fg,
            Some(Color::Rgb(200, 194, 180))
        );
    }

    #[test]
    fn without_colorterm_uses_the_cube() {
        let t = Theme::detect(&env(&[]));
        assert_eq!(t.mode, ColorMode::Ansi256);
        assert_eq!(
            t.section(SectionTone::Needs).fg,
            Some(Color::Indexed(ansi256(244, 167, 185)))
        );
    }

    #[test]
    fn windows_terminal_is_truecolor() {
        let t = Theme::detect(&env(&[("WT_SESSION", "abc")]));
        assert_eq!(t.mode, ColorMode::TrueColor);
    }

    #[test]
    fn light_background_uses_deeper_shades() {
        let t = Theme::detect(&env(&[("COLORTERM", "truecolor"), ("COLORFGBG", "0;15")]));
        assert_eq!(t.background, Background::Light);
        assert_eq!(
            t.section(SectionTone::Needs).fg,
            Some(Color::Rgb(180, 70, 106))
        );
        let t = Theme::detect(&env(&[("FM_THEME", "light")]));
        assert_eq!(t.background, Background::Light);
        let t = Theme::detect(&env(&[("COLORFGBG", "15;0")]));
        assert_eq!(t.background, Background::Dark);
    }

    #[test]
    fn cube_matches_pa() {
        assert_eq!(ansi256(0, 0, 0), 16);
        assert_eq!(ansi256(255, 255, 255), 231);
        assert_eq!(ansi256(244, 167, 185), 16 + 36 * 5 + 6 * 3 + 4);
    }
}
