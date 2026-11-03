//! Display-width helpers. Everything here counts terminal columns, not bytes or chars.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn width(s: &str) -> usize {
    s.width()
}

/// Control characters out (an agent's text must never move the cursor), whitespace runs to one
/// space.
pub fn one_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if c.is_control() {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

/// The first `max` columns of `s`.
pub fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > max {
            break;
        }
        w += cw;
        out.push(c);
    }
    out
}

/// `s` in at most `max` columns, ending in `ellipsis` when it was cut.
pub fn ellipsize(s: &str, max: usize, ellipsis: &str) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let ew = ellipsis.width();
    if max <= ew {
        return truncate(ellipsis, max);
    }
    let mut out = truncate(s, max - ew).trim_end().to_string();
    out.push_str(ellipsis);
    out
}

/// `s` padded with spaces to exactly `n` columns (cut when longer).
pub fn pad(s: &str, n: usize) -> String {
    let t = truncate(s, n);
    let w = t.width();
    format!("{t}{}", " ".repeat(n.saturating_sub(w)))
}

/// Words to lines of at most `max` columns, at spaces. A word wider than a line is cut hard.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.width() > max {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let head = truncate(&word, max);
            let rest = word[head.len()..].to_string();
            if head.is_empty() {
                break;
            }
            out.push(head);
            word = rest;
        }
        if word.is_empty() {
            continue;
        }
        if line.is_empty() {
            line = word;
        } else if line.width() + 1 + word.width() <= max {
            line.push(' ');
            line.push_str(&word);
        } else {
            out.push(std::mem::replace(&mut line, word));
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Text cut into lines of exactly `max` columns, never at spaces: for commands, which must be shown
/// whole.
pub fn wrap_hard(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut out = Vec::new();
    let mut line = String::new();
    let mut w = 0;
    for c in text.chars() {
        if c.is_control() {
            continue;
        }
        let cw = c.width().unwrap_or(0);
        if w + cw > max && !line.is_empty() {
            out.push(std::mem::take(&mut line));
            w = 0;
        }
        line.push(c);
        w += cw;
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

/// A command in lines of at most `max` columns, broken at spaces where it can be and mid-word only
/// when a word is wider than a line. Every character is kept: a command must be shown whole.
pub fn wrap_command(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let chars: Vec<char> = text.chars().filter(|c| !c.is_control()).collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut w = 0;
        let mut end = start;
        while end < chars.len() {
            let cw = chars[end].width().unwrap_or(0);
            if w + cw > max {
                break;
            }
            w += cw;
            end += 1;
        }
        if end == chars.len() {
            out.push(chars[start..].iter().collect());
            break;
        }
        if chars[end] == ' ' && end > start {
            out.push(chars[start..end].iter().collect());
            start = end + 1;
            continue;
        }
        // Break after the last space on this line, keeping the space at the line's end.
        match chars[start..end].iter().rposition(|c| *c == ' ') {
            Some(i) if i > 0 => {
                out.push(chars[start..start + i].iter().collect());
                start += i + 1;
            }
            _ => {
                out.push(chars[start..end.max(start + 1)].iter().collect());
                start = end.max(start + 1);
            }
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// The nearest ASCII character for each glyph the screens draw, one column each, for terminals and
/// fonts without them. Anything else is left as it is.
pub fn asciify(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    s.chars()
        .map(|c| match c {
            '·' | '—' | '–' | '─' => '-',
            '↑' => '^',
            '↓' => 'v',
            '…' => '~',
            '◄' | '←' => '<',
            '►' | '→' | '▶' | '❯' | '↗' => '>',
            '│' => '|',
            '┐' | '┘' | '┌' | '└' | '┴' | '┬' | '├' | '┤' | '┼' => '+',
            '✓' => '+',
            '✗' => 'x',
            '⏸' | '‖' => '=',
            '↷' => '~',
            '‼' => '!',
            '●' => '*',
            '○' | '◌' => 'o',
            '■' => '#',
            '“' | '”' => '"',
            '‘' | '’' => '\'',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_wrap_at_spaces_and_keep_every_character() {
        let cmd = "claude -p --output-format stream-json --model claude-sonnet-5";
        let lines = wrap_command(cmd, 30);
        assert_eq!(
            lines,
            vec![
                "claude -p --output-format",
                "stream-json --model",
                "claude-sonnet-5"
            ]
        );
        assert_eq!(lines.join(" "), cmd);
        let long = "a-very-long-flag-without-any-spaces";
        assert_eq!(wrap_command(long, 10).concat(), long);
        assert_eq!(wrap_command("", 10), vec![String::new()]);
    }

    #[test]
    fn asciify_keeps_width() {
        let s = "✓ Gather ─┐ · ↑↓ … ↗ link";
        let a = asciify(s);
        assert!(a.is_ascii());
        assert_eq!(a.width(), s.width());
    }

    #[test]
    fn ellipsize_counts_columns() {
        assert_eq!(ellipsize("hello world", 8, "…"), "hello w…");
        assert_eq!(ellipsize("hello", 8, "…"), "hello");
        assert_eq!(ellipsize("hello", 1, "…"), "…");
        assert_eq!(ellipsize("hello", 0, "…"), "");
        assert_eq!(ellipsize("日本語テキスト", 6, "…"), "日本…");
    }

    #[test]
    fn pad_and_truncate() {
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad("abcdef", 4), "abcd");
        assert_eq!(truncate("日本", 3), "日");
    }

    #[test]
    fn wrap_at_spaces_and_cut_long_words() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert!(wrap("", 4).is_empty());
    }

    #[test]
    fn wrap_hard_keeps_every_character() {
        let cmd = "claude -p --model x";
        let lines = wrap_hard(cmd, 6);
        assert_eq!(lines.concat(), cmd);
        assert!(lines.iter().all(|l| l.width() <= 6));
    }

    #[test]
    fn one_line_strips_controls() {
        assert_eq!(one_line("  a\n\tb\x1b[31m c "), "a b[31m c");
    }
}
