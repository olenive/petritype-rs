//! Request lines, read with a cap on their length (since 2026-11-28), for
//! the net process on stdin ([`main`](crate::main)) and for a viewer of a
//! [`HostServer`](crate::HostServer) on TCP alike.
//!
//! A line is at most [`DEFAULT_MAX_LINE_BYTES`] long without its newline,
//! unless the caller gives another cap. A longer one is skipped up to its
//! newline as it comes in, never held whole, and is answered as a line that
//! is not JSON is, `invalid_params` with the `id` `""`, since none could be
//! read; the lines after it are read as ever. A line that is not UTF-8 is
//! answered the same way. This is PROTOCOL.md §2 as the Python runner
//! reads it.

use std::io::{self, BufRead};

/// The longest request line read when nothing says otherwise, in bytes
/// without its newline: 4 MiB, as in the Python runner.
pub(crate) const DEFAULT_MAX_LINE_BYTES: usize = 4 << 20;

/// The environment variable that sets the cap of the net process, in
/// bytes, when it holds a positive integer: the name the Python runner
/// reads.
pub(crate) const MAX_LINE_ENV: &str = "PETRITYPE_RUNNER_MAX_LINE_BYTES";

/// The cap a value of [`MAX_LINE_ENV`] sets: a positive integer, else the
/// default.
pub(crate) fn max_line_bytes(value: Option<&str>) -> usize {
    value
        .and_then(|bytes| bytes.trim().parse::<usize>().ok())
        .filter(|bytes| *bytes > 0)
        .unwrap_or(DEFAULT_MAX_LINE_BYTES)
}

/// One line that is not blank.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Line {
    /// Its text, without the newline and a carriage return before it.
    Text(String),
    /// A line that could not be read as text, too long or not UTF-8: the
    /// message of its `invalid_params` answer.
    Refused(String),
}

/// The lines of a reader, each at most `max` bytes long without its
/// newline, until end of file or a read that fails. A last line without a
/// newline counts. Blank lines are passed over.
///
/// What it holds is one line up to the cap, and the reader's own buffer:
/// a line that passes the cap is let go of there, and the rest of it is
/// read through and dropped up to its newline.
pub(crate) struct Lines<R> {
    reader: R,
    max: usize,
    /// The line so far.
    line: Vec<u8>,
    /// Whether the line so far passed the cap, and is being skipped.
    skipping: bool,
    /// End of file, or a read that failed: nothing more comes.
    done: bool,
}

impl<R: BufRead> Lines<R> {
    pub(crate) fn new(reader: R, max: usize) -> Lines<R> {
        Lines {
            reader,
            max,
            line: Vec::new(),
            skipping: false,
            done: false,
        }
    }

    /// The next line, blank or not, or `None` at the end.
    fn next_line(&mut self) -> Option<Line> {
        while !self.done {
            let chunk = match self.reader.fill_buf() {
                Ok(chunk) => chunk,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.done = true;
                    break;
                }
            };
            if chunk.is_empty() {
                self.done = true;
                break;
            }
            let (taken, ended) = match chunk.iter().position(|&byte| byte == b'\n') {
                Some(at) => (at, true),
                None => (chunk.len(), false),
            };
            if !self.skipping {
                if self.line.len() + taken > self.max {
                    // Past the cap: what was read of it goes, and so does
                    // the rest, as it comes.
                    self.skipping = true;
                    self.line = Vec::new();
                } else {
                    self.line.extend_from_slice(&chunk[..taken]);
                }
            }
            self.reader.consume(taken + usize::from(ended));
            if ended {
                return Some(self.finish());
            }
        }
        // End of file: a line without its newline counts.
        (self.skipping || !self.line.is_empty()).then(|| self.finish())
    }

    /// The line just ended, and a fresh start for the next.
    fn finish(&mut self) -> Line {
        if std::mem::take(&mut self.skipping) {
            return Line::Refused(format!(
                "a request line is at most {} bytes long; a longer one was discarded unread",
                self.max
            ));
        }
        match String::from_utf8(std::mem::take(&mut self.line)) {
            Ok(mut text) => {
                if text.ends_with('\r') {
                    text.pop();
                }
                Line::Text(text)
            }
            Err(_) => Line::Refused("a request line must be UTF-8".into()),
        }
    }
}

impl<R: BufRead> Iterator for Lines<R> {
    type Item = Line;

    fn next(&mut self) -> Option<Line> {
        loop {
            match self.next_line()? {
                Line::Text(text) if text.trim().is_empty() => {}
                line => return Some(line),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Read};

    use super::*;

    /// `data` read through a buffer of `size` bytes, a chunk at a time.
    fn lines(data: &[u8], size: usize, max: usize) -> Vec<Line> {
        Lines::new(BufReader::with_capacity(size, data), max).collect()
    }

    fn text(text: &str) -> Line {
        Line::Text(text.to_owned())
    }

    fn too_long(max: usize) -> Line {
        Line::Refused(format!(
            "a request line is at most {max} bytes long; a longer one was discarded unread"
        ))
    }

    #[test]
    fn a_line_over_the_cap_is_refused_and_the_lines_around_it_are_read() {
        let a = "a".repeat(10);
        let d = "d".repeat(3);
        let data = format!(
            "{a}\n{}\n{}\n{d}\n{}",
            "b".repeat(11),
            "c".repeat(25),
            "e".repeat(12)
        );
        for size in [1, 3, 7, 4096, 1 << 16] {
            assert_eq!(
                lines(data.as_bytes(), size, 10),
                [text(&a), too_long(10), too_long(10), text(&d), too_long(10)],
                "read {size} bytes at a time"
            );
            assert_eq!(lines(&[b'x'; 11], size, 10), [too_long(10)]);
        }
    }

    #[test]
    fn blank_lines_are_passed_over_and_a_carriage_return_goes() {
        let data = b"\n  \r\n{\"id\": \"1\"}\r\nlast";
        assert_eq!(
            lines(data, 4096, 100),
            [text("{\"id\": \"1\"}"), text("last")]
        );
        // The cap counts the carriage return, as the bytes before the
        // newline.
        assert_eq!(lines(b"abc\r\n", 4096, 3), [too_long(3)]);
        assert_eq!(lines(b"abc\r\n", 4096, 4), [text("abc")]);
    }

    #[test]
    fn a_line_that_is_not_utf8_is_refused_and_the_next_is_read() {
        let data = b"\xff\xfe\n{}\n";
        assert_eq!(
            lines(data, 4096, 100),
            [
                Line::Refused("a request line must be UTF-8".into()),
                text("{}")
            ]
        );
    }

    /// A reader of 64 MiB of one line, then `{}`, made as it is read.
    struct Endless {
        left: usize,
        tail: &'static [u8],
    }

    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.left == 0 {
                let n = self.tail.len().min(buf.len());
                buf[..n].copy_from_slice(&self.tail[..n]);
                self.tail = &self.tail[n..];
                return Ok(n);
            }
            let n = self.left.min(buf.len());
            buf[..n].fill(b'z');
            self.left -= n;
            Ok(n)
        }
    }

    #[test]
    fn a_line_over_the_cap_is_never_held() {
        let reader = Endless {
            left: 64 << 20,
            tail: b"\n{}\n",
        };
        let mut read = Lines::new(BufReader::new(reader), DEFAULT_MAX_LINE_BYTES);
        assert_eq!(read.next(), Some(too_long(DEFAULT_MAX_LINE_BYTES)));
        assert!(
            read.line.capacity() <= DEFAULT_MAX_LINE_BYTES,
            "held {} bytes",
            read.line.capacity()
        );
        assert_eq!(read.next(), Some(text("{}")));
        assert_eq!(read.next(), None);
    }

    #[test]
    fn the_cap_from_the_environment() {
        assert_eq!(max_line_bytes(None), DEFAULT_MAX_LINE_BYTES);
        assert_eq!(DEFAULT_MAX_LINE_BYTES, 4 << 20);
        assert_eq!(max_line_bytes(Some("1000")), 1000);
        assert_eq!(max_line_bytes(Some(" 1000 ")), 1000);
        for bad in ["0", "-5", "big", ""] {
            assert_eq!(max_line_bytes(Some(bad)), DEFAULT_MAX_LINE_BYTES, "{bad:?}");
        }
    }
}
