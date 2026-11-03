//! Reading a child's output: lines from standard output, a bounded head and tail.

use std::io::{ErrorKind, Read};
use std::sync::mpsc::SyncSender;

use super::{MAX_LINE_BYTES, STDERR_MAX_BYTES};

/// What a reader thread tells the waiting thread.
pub(super) enum Event {
    /// One line of standard output, with its line ending if it had one.
    Line(Vec<u8>),
    StdoutDone,
    StderrDone(Tail),
}

/// Split `r` into lines and send each one; a line longer than [`MAX_LINE_BYTES`] goes in pieces.
pub(super) fn read_lines(r: impl Read, tx: &SyncSender<Event>) {
    let _ = split_lines(r, MAX_LINE_BYTES, |line| tx.send(Event::Line(line)).is_ok());
    let _ = tx.send(Event::StdoutDone);
}

/// Keep the last [`STDERR_MAX_BYTES`] of `r` and send them at its end.
pub(super) fn read_tail(mut r: impl Read, tx: &SyncSender<Event>) {
    let mut tail = Tail::new(STDERR_MAX_BYTES);
    let mut buf = [0u8; 8192];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => tail.push(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = tx.send(Event::StderrDone(tail));
}

/// Call `emit` with each line of `r`. Stops early when `emit` returns false.
pub(super) fn split_lines(
    mut r: impl Read,
    max_line: usize,
    mut emit: impl FnMut(Vec<u8>) -> bool,
) -> bool {
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let mut rest = &buf[..n];
        while !rest.is_empty() {
            let room = max_line.saturating_sub(pending.len()).max(1);
            let window = &rest[..rest.len().min(room)];
            match window.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    pending.extend_from_slice(&window[..=i]);
                    rest = &rest[i + 1..];
                    if !emit(std::mem::take(&mut pending)) {
                        return false;
                    }
                }
                None => {
                    pending.extend_from_slice(window);
                    rest = &rest[window.len()..];
                    if pending.len() >= max_line && !emit(std::mem::take(&mut pending)) {
                        return false;
                    }
                }
            }
        }
    }
    if !pending.is_empty() {
        return emit(pending);
    }
    true
}

/// A line as text: line ending removed, invalid UTF-8 replaced.
pub(super) fn line_text(bytes: &[u8]) -> String {
    let b = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let b = b.strip_suffix(b"\r").unwrap_or(b);
    String::from_utf8_lossy(b).into_owned()
}

/// The first `max` bytes, and a count of all of them.
pub(super) struct Head {
    buf: Vec<u8>,
    max: usize,
    total: u64,
    truncated: bool,
}

impl Head {
    pub(super) fn new(max: usize) -> Self {
        Self {
            buf: Vec::new(),
            max,
            total: 0,
            truncated: false,
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        let room = self.max - self.buf.len();
        if bytes.len() > room {
            self.truncated = true;
            self.buf.extend_from_slice(&bytes[..room]);
        } else {
            self.buf.extend_from_slice(bytes);
        }
    }

    pub(super) fn total(&self) -> u64 {
        self.total
    }

    pub(super) fn truncated(&self) -> bool {
        self.truncated
    }

    /// The kept bytes as text. A character cut by the cap is dropped, not replaced.
    pub(super) fn text(&self) -> String {
        let mut b = self.buf.as_slice();
        if self.truncated {
            // Only an incomplete sequence at the very end (`error_len() == None`) is dropped.
            if let Err(e) = std::str::from_utf8(b)
                && e.error_len().is_none()
            {
                b = &b[..e.valid_up_to()];
            }
        }
        String::from_utf8_lossy(b).into_owned()
    }
}

/// The last `max` bytes.
pub(super) struct Tail {
    buf: Vec<u8>,
    max: usize,
    cut: bool,
}

impl Tail {
    pub(super) fn new(max: usize) -> Self {
        Self {
            buf: Vec::new(),
            max,
            cut: false,
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        // Trim in batches so a chatty child does not shift the buffer on every read.
        if self.buf.len() > self.max.saturating_mul(2).max(8192) {
            self.trim();
        }
    }

    fn trim(&mut self) {
        if self.buf.len() > self.max {
            let from = self.buf.len() - self.max;
            self.buf.drain(..from);
            self.cut = true;
        }
    }

    /// The kept bytes as text. A character cut at the start is dropped, not replaced.
    pub(super) fn text(mut self) -> String {
        self.trim();
        let mut b = self.buf.as_slice();
        if self.cut {
            let skip = b
                .iter()
                .take(3)
                .take_while(|&&c| c & 0b1100_0000 == 0b1000_0000)
                .count();
            b = &b[skip..];
        }
        String::from_utf8_lossy(b).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8], max: usize) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        split_lines(input, max, |l| {
            out.push(l);
            true
        });
        out
    }

    #[test]
    fn splits_on_newlines_and_keeps_a_last_partial_line() {
        assert_eq!(
            lines(b"one\ntwo\r\nthree", 100),
            vec![b"one\n".to_vec(), b"two\r\n".to_vec(), b"three".to_vec()]
        );
        assert!(lines(b"", 100).is_empty());
    }

    #[test]
    fn a_line_longer_than_the_cap_goes_in_pieces() {
        assert_eq!(
            lines(b"abcdefg\nhi\n", 3),
            vec![
                b"abc".to_vec(),
                b"def".to_vec(),
                b"g\n".to_vec(),
                b"hi\n".to_vec()
            ]
        );
    }

    #[test]
    fn stops_when_the_receiver_is_gone() {
        let mut n = 0;
        let finished = split_lines(&b"a\nb\nc\n"[..], 100, |_| {
            n += 1;
            n < 2
        });
        assert!(!finished);
        assert_eq!(n, 2);
    }

    #[test]
    fn line_text_drops_the_line_ending_and_survives_bad_utf8() {
        assert_eq!(line_text(b"hi\r\n"), "hi");
        assert_eq!(line_text(b"hi\n"), "hi");
        assert_eq!(line_text(b"hi"), "hi");
        assert_eq!(line_text(b"\xffok\n"), "\u{fffd}ok");
    }

    #[test]
    fn head_keeps_the_first_bytes_and_counts_all() {
        let mut h = Head::new(5);
        h.push(b"abc");
        h.push(b"defgh");
        assert_eq!(h.text(), "abcde");
        assert_eq!(h.total(), 8);
        assert!(h.truncated());

        let mut h = Head::new(5);
        h.push(b"abcde");
        assert!(!h.truncated());
    }

    #[test]
    fn head_drops_a_character_cut_by_the_cap() {
        let mut h = Head::new(4);
        h.push("abé!".as_bytes()); // é is two bytes: a b 0xC3 | 0xA9 !
        h.push(b"");
        let mut h2 = Head::new(3);
        h2.push("abé".as_bytes());
        assert_eq!(h.text(), "abé");
        assert_eq!(h2.text(), "ab");
    }

    #[test]
    fn tail_keeps_the_last_bytes_on_a_character_boundary() {
        let mut t = Tail::new(4);
        for _ in 0..10_000 {
            t.push(b"x");
        }
        t.push("éé".as_bytes());
        assert_eq!(t.text(), "éé");

        let mut t = Tail::new(3);
        t.push("aéé".as_bytes()); // last 3 bytes: 0xA9 é
        assert_eq!(t.text(), "é");
    }
}
