// SPDX-License-Identifier: Apache-2.0
//
// Log input: standard input, or a file followed the way `tail -F` follows it.
// The file follower survives rotation (the path now names a different inode:
// finish the old file, then read the new one from its start) and truncation
// (the file got shorter than what was read: start again from the top). A
// missing file is waited for. Bytes that are not UTF-8 are replaced, never
// fatal: a damaged line is one more `Other` line.

use std::fs::File;
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::Duration;

/// What an input source delivers to the detector.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Line(String),
    /// The source ended (standard input closed). The reading turns unmeasured.
    Closed,
}

pub struct FileFollower {
    path: PathBuf,
    file: Option<File>,
    ino: u64,
    dev: u64,
    pos: u64,
    partial: Vec<u8>,
    /// Read the first file from its start (true) or from its end (false).
    /// Every file opened after a rotation is read from its start.
    from_start: bool,
}

impl FileFollower {
    pub fn new(path: impl Into<PathBuf>, from_start: bool) -> Self {
        Self {
            path: path.into(),
            file: None,
            ino: 0,
            dev: 0,
            pos: 0,
            partial: Vec::new(),
            from_start,
        }
    }

    /// Read whatever is available now and return the complete lines.
    pub fn poll(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.file.is_none() && !self.open() {
            // A file that appears later is new in full: read it from its start.
            self.from_start = true;
            return lines;
        }
        self.drain(&mut lines);
        match std::fs::metadata(&self.path) {
            Ok(meta) if meta.ino() != self.ino || meta.dev() != self.dev => {
                // Rotated: the old file is drained; switch to the new one.
                self.flush_partial(&mut lines);
                self.file = None;
                self.from_start = true;
                if self.open() {
                    self.drain(&mut lines);
                }
            }
            Ok(meta) if meta.len() < self.pos => {
                // Truncated in place (copytruncate): start again from the top.
                self.partial.clear();
                if let Some(f) = self.file.as_mut() {
                    if f.seek(SeekFrom::Start(0)).is_ok() {
                        self.pos = 0;
                    }
                }
                self.drain(&mut lines);
            }
            _ => {} // unchanged, or the path is gone: keep the open file
        }
        lines
    }

    fn open(&mut self) -> bool {
        let Ok(mut f) = File::open(&self.path) else {
            return false;
        };
        let Ok(meta) = f.metadata() else {
            return false;
        };
        self.ino = meta.ino();
        self.dev = meta.dev();
        self.pos = if self.from_start {
            0
        } else {
            f.seek(SeekFrom::End(0)).unwrap_or(0)
        };
        self.partial.clear();
        self.file = Some(f);
        true
    }

    fn drain(&mut self, lines: &mut Vec<String>) {
        let Some(f) = self.file.as_mut() else { return };
        let mut buf = [0u8; 64 * 1024];
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    self.pos += n as u64;
                    self.partial.extend_from_slice(&buf[..n]);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        while let Some(nl) = self.partial.iter().position(|&b| b == b'\n') {
            let rest = self.partial.split_off(nl + 1);
            let line = std::mem::replace(&mut self.partial, rest);
            lines.push(to_line(&line));
        }
    }

    fn flush_partial(&mut self, lines: &mut Vec<String>) {
        if !self.partial.is_empty() {
            let line = std::mem::take(&mut self.partial);
            lines.push(to_line(&line));
        }
    }

    /// Follow forever, sending each line. Returns when the receiver is gone.
    pub fn run(mut self, poll_every: Duration, mut send: impl FnMut(Input) -> bool) {
        loop {
            for line in self.poll() {
                if !send(Input::Line(line)) {
                    return;
                }
            }
            std::thread::sleep(poll_every);
        }
    }
}

fn to_line(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    s.trim_end_matches(['\n', '\r']).to_string()
}

/// Read a stream (standard input) to its end, sending each line, then `Closed`.
pub fn read_stream(reader: impl Read, mut send: impl FnMut(Input) -> bool) {
    let mut r = io::BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                if !send(Input::Line(to_line(&buf))) {
                    return;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    send(Input::Closed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "peer-health-follow-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn append(path: &PathBuf, text: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn follows_appends_rotation_and_truncation() {
        let dir = scratch("rotate");
        let path = dir.join("router.log");
        append(&path, "old history\n");
        let mut f = FileFollower::new(&path, false);
        // First open from the end: existing history is skipped.
        assert!(f.poll().is_empty());
        append(&path, "one\ntw");
        assert_eq!(f.poll(), vec!["one"]);
        append(&path, "o\n");
        assert_eq!(f.poll(), vec!["two"]);

        // Truncation in place.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(0)
            .unwrap();
        append(&path, "cut\n");
        assert_eq!(f.poll(), vec!["cut"]);

        // Rotation: the old file gets a last line, then a new file appears.
        append(&path, "last in old\n");
        std::fs::rename(&path, dir.join("router.log.1")).unwrap();
        append(&dir.join("router.log.1"), "late write to old\n");
        append(&path, "first in new\n");
        assert_eq!(
            f.poll(),
            vec!["last in old", "late write to old", "first in new"]
        );
        append(&path, "second in new\n");
        assert_eq!(f.poll(), vec!["second in new"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_appears_later_is_read_from_its_start() {
        let dir = scratch("late");
        let path = dir.join("router.log");
        let mut f = FileFollower::new(&path, false);
        assert!(f.poll().is_empty(), "missing file is waited for");
        append(&path, "born\n");
        assert_eq!(f.poll(), vec!["born"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_start_reads_history_and_bad_bytes_are_replaced() {
        let dir = scratch("start");
        let path = dir.join("router.log");
        std::fs::write(&path, b"a\r\n\xff\xfe b\n").unwrap();
        let mut f = FileFollower::new(&path, true);
        assert_eq!(
            f.poll(),
            vec!["a".to_string(), "\u{fffd}\u{fffd} b".to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stream_ends_with_closed() {
        let mut got = Vec::new();
        read_stream(&b"x\ny"[..], |i| {
            got.push(i);
            true
        });
        assert_eq!(
            got,
            vec![
                Input::Line("x".into()),
                Input::Line("y".into()),
                Input::Closed
            ]
        );
    }
}
