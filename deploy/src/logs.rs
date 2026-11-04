//! A bounded, redacting log sink (review gap #3, spec D6/D8).
//!
//! Redaction happens **before** anything is stored, so a secret never sits in memory in a form that
//! could later be written out. It is exact-value masking, best effort, with no length floor:
//!
//! * a **single pass** finds every occurrence of every secret and masks the union of the matches, so
//!   a shorter secret never leaves the tail of a longer one behind, overlapping secrets are masked as
//!   one span, and a marker is never re-scanned (a one-character secret cannot corrupt a marker);
//! * it works on a **stream**: output arrives in arbitrary chunks and a secret may straddle a chunk
//!   boundary or contain newlines, so the end of each chunk that could be the start of a secret is held
//!   back until the next chunk (or the end of the stream) decides it;
//! * it **fails closed**: if the redaction step fails, the raw text is withheld, never stored or shown,
//!   and the capture says so.
//!
//! Transformed secrets (base64, hashes, case changes, values a program splits across records) are not
//! detected: a documented limit, not a defect.
//!
//! Size is bounded as a head and a tail. When lines are dropped the sink says so, with counts, and the
//! rendered text contains an explicit marker; truncation is never silent.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactError(pub String);

#[derive(Clone, PartialEq, Eq)]
pub struct Redactor {
    /// (secret, label) sorted longest first.
    secrets: Vec<(String, String)>,
    /// Test double: every scan fails (see [`Redactor::failing`]).
    fail: bool,
}

/// `Debug` shows how many secrets there are, never what they are: a configuration that holds a
/// redactor can be formatted into an error or a panic message safely.
impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Redactor({} secrets, <redacted>{})",
            self.secrets.len(),
            if self.fail { ", failing" } else { "" }
        )
    }
}

impl Redactor {
    pub fn none() -> Redactor {
        Redactor {
            secrets: Vec::new(),
            fail: false,
        }
    }

    /// A redactor whose every scan fails, to prove that raw output is withheld.
    pub fn failing() -> Redactor {
        Redactor {
            secrets: Vec::new(),
            fail: true,
        }
    }

    /// `(name, value)` pairs. Empty values are ignored (they would match everywhere).
    pub fn new(secrets: &[(&str, &str)]) -> Redactor {
        let mut v: Vec<(String, String)> = secrets
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(n, value)| (value.to_string(), n.to_string()))
            .collect();
        v.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        v.dedup_by(|a, b| a.0 == b.0);
        Redactor {
            secrets: v,
            fail: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty() && !self.fail
    }

    /// Redact a whole text. Returns the redacted text and how many spans were masked.
    pub fn apply(&self, text: &str) -> (String, u32) {
        match self.scan(&mut String::new(), text, true) {
            Ok(r) => r,
            // A message that cannot be redacted is withheld whole.
            Err(_) => ("[withheld: the redaction step failed]".to_string(), 0),
        }
    }

    /// One step over a stream. `pending` is the text held back from the previous step (it may be the
    /// start of a secret); the safe prefix of `pending + chunk` is returned masked, the rest stays in
    /// `pending`. With `finish` nothing is held back.
    pub fn scan(&self, pending: &mut String, chunk: &str, finish: bool) -> Result<(String, u32), RedactError> {
        if self.fail {
            return Err(RedactError("forced failure".into()));
        }
        let mut buf = std::mem::take(pending);
        buf.push_str(chunk);
        if self.secrets.is_empty() {
            return Ok((buf, 0));
        }
        // Every match as a [start, end) span with its label, then merged into disjoint spans.
        let mut spans: Vec<(usize, usize, &str)> = Vec::new();
        for (i, _) in buf.char_indices() {
            let rest = &buf[i..];
            for (secret, name) in &self.secrets {
                if rest.starts_with(secret.as_str()) {
                    spans.push((i, i + secret.len(), name.as_str()));
                }
            }
        }
        let mut merged: Vec<(usize, usize, Vec<&str>)> = Vec::new();
        for (s, e, name) in spans {
            match merged.last_mut() {
                Some((_, me, names)) if s < *me => {
                    // A secret fully inside the span is not named (the longer one takes precedence);
                    // one that extends it is.
                    if e > *me {
                        *me = e;
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
                _ => merged.push((s, e, vec![name])),
            }
        }
        // Hold back from the first position whose remaining text is a proper prefix of some secret
        // (it could still become a match), and from the start of any span that reaches it.
        let mut cut = buf.len();
        if !finish {
            for (i, _) in buf.char_indices() {
                let rest = &buf[i..];
                if self
                    .secrets
                    .iter()
                    .any(|(s, _)| s.len() > rest.len() && s.starts_with(rest))
                {
                    cut = i;
                    break;
                }
            }
            for (s, e, _) in &merged {
                if *s < cut && cut < *e {
                    cut = *s;
                }
            }
            // A span that ends right at the cut may still be extended by an overlapping secret that
            // begins inside it and is not complete yet: that begin is itself a held-back prefix above.
        }
        let mut out = String::with_capacity(cut);
        let mut at = 0usize;
        let mut n = 0u32;
        for (s, e, names) in &merged {
            if *s >= cut {
                break;
            }
            out.push_str(&buf[at..*s]);
            out.push_str(&format!("[REDACTED:{}]", names.join("+")));
            at = *e;
            n += 1;
        }
        if at < cut {
            out.push_str(&buf[at..cut]);
        }
        *pending = buf[cut.max(at)..].to_string();
        Ok((out, n))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLimits {
    pub head_bytes: usize,
    pub tail_bytes: usize,
}

impl Default for LogLimits {
    /// 1 MB in total, as in the first-slice boundary doc.
    fn default() -> Self {
        LogLimits {
            head_bytes: 256 * 1024,
            tail_bytes: 768 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LogSummary {
    pub lines_seen: u64,
    pub bytes_kept: u64,
    pub lines_dropped: u64,
    pub bytes_dropped: u64,
    pub redactions: u32,
    /// The redaction step failed: everything from that point was withheld, not stored.
    pub withheld: bool,
}

impl LogSummary {
    pub fn truncated(&self) -> bool {
        self.lines_dropped > 0
    }
}

/// A stored line longer than this is split (after redaction) so one unterminated stream cannot grow
/// without bound.
const MAX_STORED_LINE: usize = 16 * 1024;
const WITHHELD: &str = "[output withheld: the redaction step failed, so the raw text was not stored]";

#[derive(Clone)]
pub struct LogSink {
    limits: LogLimits,
    redactor: Redactor,
    /// Raw text held back because it may be the start of a secret.
    pending: String,
    /// Redacted text of the line being assembled.
    partial: String,
    head: Vec<String>,
    head_bytes: usize,
    tail: VecDeque<String>,
    tail_bytes: usize,
    summary: LogSummary,
}

impl std::fmt::Debug for LogSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // not the held-back raw text, not the stored lines
        write!(f, "LogSink({} lines seen, <redacted>)", self.summary.lines_seen)
    }
}

/// What a sink holds at a point in time. `text` already contains the truncation marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogCapture {
    pub text: String,
    pub summary: LogSummary,
}

impl LogSink {
    pub fn new(limits: LogLimits, redactor: Redactor) -> LogSink {
        LogSink {
            limits,
            redactor,
            pending: String::new(),
            partial: String::new(),
            head: Vec::new(),
            head_bytes: 0,
            tail: VecDeque::new(),
            tail_bytes: 0,
            summary: LogSummary::default(),
        }
    }

    /// Redact a message that is *not* a log line (an error, a step note) with the same rules.
    pub fn redact(&self, text: &str) -> String {
        self.redactor.apply(text).0
    }

    /// Feed raw output in any chunking, newlines included. Redaction runs before anything is stored.
    pub fn push_chunk(&mut self, chunk: &str) {
        if self.summary.withheld {
            return;
        }
        match self.redactor.scan(&mut self.pending, chunk, false) {
            Ok((safe, n)) => {
                self.summary.redactions += n;
                self.absorb(&safe);
            }
            Err(_) => self.withhold(),
        }
    }

    /// One complete line (a trailing line break is dropped, one is added).
    pub fn push_line(&mut self, line: &str) {
        self.push_chunk(line.trim_end_matches(['\r', '\n']));
        self.push_chunk("\n");
    }

    pub fn push_text(&mut self, text: &str) {
        for l in text.lines() {
            self.push_line(l);
        }
    }

    fn withhold(&mut self) {
        self.summary.withheld = true;
        self.pending.clear();
        self.partial.clear();
    }

    /// Redacted text arrives here; whole lines are stored, the rest waits for its newline.
    fn absorb(&mut self, safe: &str) {
        self.partial.push_str(safe);
        while let Some(i) = self.partial.find('\n') {
            let line: String = self.partial[..i].to_string();
            self.partial.drain(..=i);
            self.store(line);
        }
        while self.partial.len() > MAX_STORED_LINE {
            let mut at = MAX_STORED_LINE;
            while !self.partial.is_char_boundary(at) {
                at -= 1;
            }
            let line: String = self.partial.drain(..at).collect();
            self.store(line);
        }
    }

    fn store(&mut self, clean: String) {
        self.summary.lines_seen += 1;
        let cost = clean.len() + 1;
        if self.head_bytes + cost <= self.limits.head_bytes && self.tail.is_empty() {
            self.head_bytes += cost;
            self.head.push(clean);
            return;
        }
        self.tail_bytes += cost;
        self.tail.push_back(clean);
        while self.tail_bytes > self.limits.tail_bytes {
            match self.tail.pop_front() {
                Some(dropped) => {
                    let c = dropped.len() + 1;
                    self.tail_bytes -= c;
                    self.summary.lines_dropped += 1;
                    self.summary.bytes_dropped += c as u64;
                }
                None => break,
            }
        }
    }

    pub fn capture(&self) -> LogCapture {
        // Anything still held back at the end of the stream is final: flush a copy of the state.
        let mut view = self.clone();
        if !view.summary.withheld {
            match view.redactor.scan(&mut view.pending, "", true) {
                Ok((safe, n)) => {
                    view.summary.redactions += n;
                    view.absorb(&safe);
                    if !view.partial.is_empty() {
                        let rest = std::mem::take(&mut view.partial);
                        view.store(rest);
                    }
                }
                Err(_) => view.withhold(),
            }
        }
        let mut text = String::new();
        for l in &view.head {
            text.push_str(l);
            text.push('\n');
        }
        if view.summary.truncated() {
            text.push_str(&format!(
                "[… {} lines ({} bytes) truncated by Harpoon …]\n",
                view.summary.lines_dropped, view.summary.bytes_dropped
            ));
        }
        for l in &view.tail {
            text.push_str(l);
            text.push('\n');
        }
        if view.summary.withheld {
            text.push_str(WITHHELD);
            text.push('\n');
        }
        let mut summary = view.summary;
        summary.bytes_kept = (view.head_bytes + view.tail_bytes) as u64;
        LogCapture { text, summary }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink(secrets: &[(&str, &str)]) -> LogSink {
        LogSink::new(LogLimits::default(), Redactor::new(secrets))
    }

    #[test]
    fn masks_every_length_with_no_floor() {
        for v in ["x", "abc", "12345678", &"k".repeat(40)] {
            let mut s = sink(&[("S", v)]);
            s.push_line(&format!("before {v} after"));
            let c = s.capture();
            assert!(!c.text.contains(v), "{v}: {}", c.text);
            assert!(c.text.contains("[REDACTED:S]"));
        }
    }

    #[test]
    fn a_secret_split_across_every_chunk_boundary_is_masked_and_order_is_kept() {
        let secret = "s3cr3t-value";
        let text = format!("alpha\nhead {secret} tail\nomega\n");
        for cut in 0..=text.len() {
            if !text.is_char_boundary(cut) {
                continue;
            }
            let mut s = sink(&[("K", secret)]);
            s.push_chunk(&text[..cut]);
            s.push_chunk(&text[cut..]);
            let c = s.capture();
            assert_eq!(c.text, "alpha\nhead [REDACTED:K] tail\nomega\n", "cut {cut}");
        }
        // one byte at a time
        let mut s = sink(&[("K", secret)]);
        for ch in text.chars() {
            s.push_chunk(&ch.to_string());
        }
        assert_eq!(s.capture().text, "alpha\nhead [REDACTED:K] tail\nomega\n");
    }

    #[test]
    fn a_multiline_secret_is_masked_across_its_lines() {
        let pem = "-----BEGIN KEY-----\nabc\ndef\n-----END KEY-----";
        let mut s = sink(&[("PEM", pem)]);
        for l in format!("start\n{pem}\nend").lines() {
            s.push_line(l);
        }
        let c = s.capture();
        assert!(!c.text.contains("abc") && !c.text.contains("BEGIN"), "{}", c.text);
        assert_eq!(c.text, "start\n[REDACTED:PEM]\nend\n");
    }

    #[test]
    fn overlapping_and_nested_secrets_leave_nothing_behind() {
        // one inside another: the longer wins
        let mut s = sink(&[("SHORT", "pass"), ("LONG", "password123")]);
        s.push_line("x password123 y pass z");
        assert_eq!(s.capture().text, "x [REDACTED:LONG] y [REDACTED:SHORT] z\n");
        // partial overlap: the union is masked
        let mut s = sink(&[("A", "abcd"), ("B", "cdef")]);
        s.push_line("..abcdef..");
        let t = s.capture().text;
        assert!(!t.contains("ab") && !t.contains("ef"), "{t}");
        assert!(t.contains("[REDACTED:A+B]"), "{t}");
    }

    #[test]
    fn a_short_secret_cannot_corrupt_a_marker_and_apply_is_single_pass() {
        let r = Redactor::new(&[("DB", "super-secret"), ("E", "E")]);
        let (out, n) = r.apply("E super-secret E");
        assert_eq!(out, "[REDACTED:E] [REDACTED:DB] [REDACTED:E]");
        assert_eq!(n, 3);
    }

    #[test]
    fn redaction_failure_withholds_everything_and_says_so() {
        let mut s = LogSink::new(LogLimits::default(), Redactor::failing());
        s.push_line("password=hunter2");
        s.push_line("more");
        let c = s.capture();
        assert!(c.summary.withheld);
        assert!(!c.text.contains("hunter2") && !c.text.contains("more"), "{}", c.text);
        assert!(c.text.contains("withheld"));
        assert!(!Redactor::failing().apply("hunter2").0.contains("hunter2"));
    }

    #[test]
    fn formatting_a_redactor_or_a_sink_never_prints_a_secret_or_held_back_text() {
        let mut s = sink(&[("K", "planted-secret-value")]);
        s.push_chunk("held back: planted-secret-v");
        let shown = format!("{s:?} {:?}", Redactor::new(&[("K", "planted-secret-value")]));
        assert!(!shown.contains("planted") && !shown.contains("held back"), "{shown}");
        let cfg = crate::engine::RunConfig {
            redactor: Redactor::new(&[("K", "planted-secret-value")]),
            ..crate::engine::RunConfig::default()
        };
        assert!(!format!("{cfg:?}").contains("planted"));
    }

    #[test]
    fn truncation_is_explicit_and_structure_is_kept() {
        let mut s = LogSink::new(
            LogLimits {
                head_bytes: 20,
                tail_bytes: 20,
            },
            Redactor::none(),
        );
        for i in 0..20 {
            s.push_line(&format!("line {i:02}"));
        }
        let c = s.capture();
        assert!(c.summary.truncated() && c.text.contains("truncated by Harpoon"));
        assert!(c.text.starts_with("line 00\n") && c.text.trim_end().ends_with("line 19"));
    }
}
