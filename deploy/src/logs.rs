//! A bounded, redacting log sink (review gap #3, spec D8).
//!
//! Redaction happens **before** anything is stored, so a secret never sits in memory in a
//! form that could later be written out. It is best-effort and has no length floor: every
//! non-empty secret is replaced, longest first, so a short secret that is a substring of a
//! longer one cannot leave the longer one's tail behind. A secret split across two lines is
//! *not* detected (documented limit; the caller feeds whole lines).
//!
//! Size is bounded as a head and a tail. When lines are dropped the sink says so, with
//! counts, and the rendered text contains an explicit marker; truncation is never silent.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redactor {
    /// (secret, label) sorted longest first.
    secrets: Vec<(String, String)>,
}

impl Redactor {
    pub fn none() -> Redactor {
        Redactor { secrets: Vec::new() }
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
        Redactor { secrets: v }
    }

    /// Returns the redacted text and how many replacements were made.
    pub fn apply(&self, text: &str) -> (String, u32) {
        let mut out = text.to_string();
        let mut n = 0;
        for (secret, name) in &self.secrets {
            let marker = format!("[REDACTED:{name}]");
            let count = out.matches(secret.as_str()).count() as u32;
            if count > 0 {
                out = out.replace(secret.as_str(), &marker);
                n += count;
            }
        }
        (out, n)
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
}

impl LogSummary {
    pub fn truncated(&self) -> bool {
        self.lines_dropped > 0
    }
}

#[derive(Debug, Clone)]
pub struct LogSink {
    limits: LogLimits,
    redactor: Redactor,
    head: Vec<String>,
    head_bytes: usize,
    tail: VecDeque<String>,
    tail_bytes: usize,
    summary: LogSummary,
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

    pub fn push_line(&mut self, line: &str) {
        let (clean, n) = self.redactor.apply(line.trim_end_matches(['\r', '\n']));
        self.summary.redactions += n;
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

    pub fn push_text(&mut self, text: &str) {
        for l in text.lines() {
            self.push_line(l);
        }
    }

    pub fn capture(&self) -> LogCapture {
        let mut text = String::new();
        for l in &self.head {
            text.push_str(l);
            text.push('\n');
        }
        if self.summary.truncated() {
            text.push_str(&format!(
                "[… {} lines ({} bytes) truncated by Harpoon …]\n",
                self.summary.lines_dropped, self.summary.bytes_dropped
            ));
        }
        for l in &self.tail {
            text.push_str(l);
            text.push('\n');
        }
        let mut summary = self.summary;
        summary.bytes_kept = (self.head_bytes + self.tail_bytes) as u64;
        LogCapture { text, summary }
    }
}
