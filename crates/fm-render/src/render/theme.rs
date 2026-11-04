//! Status colours for the light and dark themes.

use crate::state::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub status: Status,
    /// Mermaid `classDef` name. Prefixed so it cannot collide with an author's.
    pub class: &'static str,
    pub fill: &'static str,
    pub stroke: &'static str,
    pub text: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

const LIGHT: &[Style] = &[
    style(Status::Pending, "#f3f2f1", "#8a8886", "#605e5c"),
    style(Status::Active, "#fff4ce", "#c77700", "#3b3a39"),
    style(Status::Waiting, "#deecf9", "#0078d4", "#3b3a39"),
    style(Status::Done, "#dff6dd", "#107c10", "#3b3a39"),
    style(Status::Failed, "#fde7e9", "#c4314b", "#3b3a39"),
    style(Status::Skipped, "#f3f2f1", "#8a8886", "#a19f9d"),
    style(Status::Interrupted, "#fde7e9", "#8a8886", "#605e5c"),
];

const DARK: &[Style] = &[
    style(Status::Pending, "#2d2d2d", "#6b6b6b", "#c8c6c4"),
    style(Status::Active, "#4a3c00", "#f7a800", "#f3f2f1"),
    style(Status::Waiting, "#0b3c5d", "#4aa3e0", "#f3f2f1"),
    style(Status::Done, "#123d12", "#4ad04a", "#f3f2f1"),
    style(Status::Failed, "#4d1520", "#f1707f", "#f3f2f1"),
    style(Status::Skipped, "#2d2d2d", "#6b6b6b", "#8a8886"),
    style(Status::Interrupted, "#4d1520", "#6b6b6b", "#c8c6c4"),
];

const fn style(
    status: Status,
    fill: &'static str,
    stroke: &'static str,
    text: &'static str,
) -> Style {
    Style {
        status,
        class: class_of(status),
        fill,
        stroke,
        text,
    }
}

const fn class_of(status: Status) -> &'static str {
    match status {
        Status::Pending => "fm_pending",
        Status::Active => "fm_active",
        Status::Waiting => "fm_waiting",
        Status::Done => "fm_done",
        Status::Failed => "fm_failed",
        Status::Skipped => "fm_skipped",
        Status::Interrupted => "fm_interrupted",
    }
}

pub fn palette(theme: Theme) -> &'static [Style] {
    match theme {
        Theme::Light => LIGHT,
        Theme::Dark => DARK,
    }
}

pub const fn taken_edge_stroke(theme: Theme) -> &'static str {
    match theme {
        Theme::Light => "#107c10",
        Theme::Dark => "#4ad04a",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_themes_describe_every_status_in_the_same_order() {
        let light: Vec<Status> = LIGHT.iter().map(|s| s.status).collect();
        let dark: Vec<Status> = DARK.iter().map(|s| s.status).collect();
        assert_eq!(light, Status::ALL.to_vec());
        assert_eq!(dark, Status::ALL.to_vec());
        for style in LIGHT.iter().chain(DARK) {
            assert!(style.fill.starts_with('#'), "{style:?}");
            assert!(style.stroke.starts_with('#'), "{style:?}");
        }
    }

    #[test]
    fn class_names_are_unique_and_namespaced() {
        let mut classes: Vec<&str> = Status::ALL.iter().map(|s| class_of(*s)).collect();
        let total = classes.len();
        assert!(classes.iter().all(|class| class.starts_with("fm_")));
        classes.sort_unstable();
        classes.dedup();
        assert_eq!(classes.len(), total, "two statuses share a class name");
    }

    #[test]
    fn theme_names_parse_and_dark_is_the_default() {
        assert_eq!(Theme::parse("light"), Some(Theme::Light));
        assert_eq!(Theme::parse("dark"), Some(Theme::Dark));
        assert_eq!(Theme::parse("sepia"), None);
        assert_eq!(Theme::default(), Theme::Dark);
    }
}
