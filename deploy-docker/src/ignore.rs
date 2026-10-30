//! `.dockerignore` handling for the **size preflight only**. Buildx applies the real file when it
//! builds; this module decides how much of the tree the 256 MB limit counts.
//!
//! The rule is *never under-count*. Only patterns whose meaning is certain exclude anything:
//! literal path segments, `*` inside a segment and whole-segment `**`, anchored at the context
//! root like Docker's. Anything else (`?`, `[..]`, `\`, `.`/`..` segments) is ignored for sizing,
//! which can only over-count. A `!` exception line could re-include an excluded path, so its
//! presence turns filtering off entirely and the whole tree is counted. The Dockerfile and the
//! ignore files are always sent by Docker, so they are always counted.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoreRules {
    patterns: Vec<Vec<String>>,
    /// Why filtering is partly or wholly off (for the deployment log).
    pub notes: Vec<String>,
    /// True when a `!` line disabled filtering.
    pub disabled: bool,
}

impl IgnoreRules {
    pub fn none() -> IgnoreRules {
        IgnoreRules::default()
    }

    pub fn parse(text: &str) -> IgnoreRules {
        let mut r = IgnoreRules::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('!') {
                r.disabled = true;
                r.notes.push(format!(
                    "{line:?} re-includes files; the whole context is counted"
                ));
                continue;
            }
            let segs: Vec<String> = line
                .trim_start_matches('/')
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            let understood = !segs.is_empty()
                && segs.iter().all(|s| {
                    s != "."
                        && s != ".."
                        && !s.contains(['?', '[', '\\', '{'])
                        && (s == "**" || !s.contains("**"))
                });
            if understood {
                r.patterns.push(segs);
            } else {
                r.notes.push(format!(
                    "pattern {line:?} is not interpreted for sizing; it excludes nothing here"
                ));
            }
        }
        if r.disabled {
            r.patterns.clear();
        }
        r
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Does the context-relative path (as segments) match, itself or through a parent folder?
    pub fn excludes(&self, rel: &[String]) -> bool {
        self.patterns
            .iter()
            .any(|p| (1..=rel.len()).any(|k| seg_match(p, &rel[..k])))
    }
}

fn seg_match(pat: &[String], path: &[String]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((p, rest)) if p == "**" => {
            // zero or more whole segments
            (0..=path.len()).any(|n| seg_match(rest, &path[n..]))
        }
        Some((p, rest)) => match path.split_first() {
            Some((s, tail)) => glob(p.as_bytes(), s.as_bytes()) && seg_match(rest, tail),
            None => false,
        },
    }
}

/// `*` matches any run of characters within one segment; everything else is literal.
fn glob(p: &[u8], s: &[u8]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|n| glob(rest, &s[n..])),
        Some((c, rest)) => s.first() == Some(c) && glob(rest, &s[1..]),
    }
}

/// Paths (context-relative) Docker always sends whatever the ignore file says.
pub fn always_sent(dockerfile_rel: &Path) -> Vec<PathBuf> {
    let mut sibling = dockerfile_rel.as_os_str().to_owned();
    sibling.push(".dockerignore");
    vec![
        dockerfile_rel.to_path_buf(),
        PathBuf::from(sibling),
        PathBuf::from(".dockerignore"),
    ]
}
