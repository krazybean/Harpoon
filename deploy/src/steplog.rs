//! The recorded, redacted log of one deployment step (Milestone E, so the desktop can show recorded
//! evidence after the process that produced it is gone).
//!
//! One `deplog` record per (deployment, step), written after the run from the engine's
//! [`crate::logs::LogCapture`], which has **already been redacted** (or withheld) before it was stored in
//! memory: nothing is scrubbed afterwards. The text is bounded to [`MAX_BYTES`]; a capture that was
//! truncated by the sink or by this bound says so. A capture whose redaction failed is recorded as withheld
//! with no text. This is evidence of what happened during the run, not the application's own runtime logs.

use crate::ids::DeploymentId;
use crate::journal_format::frame;
use crate::state::Step;

pub const MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepLogRec {
    pub deployment: DeploymentId,
    pub step: Step,
    pub text: String,
    /// The redaction step failed: the raw output was never stored.
    pub withheld: bool,
    /// Lines were dropped by the sink, or the text was cut to the bound.
    pub truncated: bool,
    pub at_ms: u64,
}

impl StepLogRec {
    /// Build a record from a redacted capture, bounding the text (on a character boundary, keeping the tail:
    /// the end of a failed build is what a developer needs).
    pub fn from_capture(
        deployment: DeploymentId,
        step: Step,
        text: &str,
        withheld: bool,
        truncated: bool,
        at_ms: u64,
    ) -> StepLogRec {
        let (text, cut) = if withheld {
            (String::new(), false)
        } else if text.len() > MAX_BYTES {
            let mut at = text.len() - MAX_BYTES;
            while !text.is_char_boundary(at) {
                at += 1;
            }
            (
                format!(
                    "[… earlier output cut to the last {MAX_BYTES} bytes …]\n{}",
                    &text[at..]
                ),
                true,
            )
        } else {
            (text.to_string(), false)
        };
        StepLogRec {
            deployment,
            step,
            text,
            withheld,
            truncated: truncated || cut,
            at_ms,
        }
    }
}

pub fn step_from_str(s: &str) -> Option<Step> {
    Step::ORDER.into_iter().find(|x| x.as_str() == s)
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<String> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b: Option<Vec<u8>> = (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok())
        .collect();
    String::from_utf8(b?).ok()
}

pub fn encode(r: &StepLogRec) -> String {
    frame(&format!(
        "deplog\t1\t{}\t{}\t{}\t{}\t{}\t{}",
        r.deployment,
        r.step.as_str(),
        u8::from(r.withheld),
        u8::from(r.truncated),
        r.at_ms,
        if r.text.is_empty() {
            "-".to_string()
        } else {
            hex(&r.text)
        }
    ))
}

pub fn decode(f: &[&str]) -> Result<StepLogRec, String> {
    if f.len() != 8 || f[0] != "deplog" || f[1] != "1" {
        return Err("a step log record is malformed".into());
    }
    let flag = |s: &str| match s {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err("a flag is not 0 or 1".to_string()),
    };
    let text = if f[7] == "-" {
        String::new()
    } else {
        unhex(f[7]).ok_or("the log text is not valid")?
    };
    if text.len() > MAX_BYTES + 128 {
        return Err("a step log is larger than the bound".into());
    }
    Ok(StepLogRec {
        deployment: f[2].parse().map_err(|_| "bad deployment id".to_string())?,
        step: step_from_str(f[3]).ok_or("unknown step")?,
        withheld: flag(f[4])?,
        truncated: flag(f[5])?,
        at_ms: f[6].parse().map_err(|_| "bad time".to_string())?,
        text,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogState {
    pub logs: Vec<StepLogRec>,
}

impl LogState {
    /// One record per (deployment, step); a second is a contradiction.
    pub fn apply(&mut self, r: &StepLogRec) -> Result<(), String> {
        if self
            .logs
            .iter()
            .any(|x| x.deployment == r.deployment && x.step == r.step)
        {
            return Err(format!(
                "deployment {} has two logs for step {}",
                r.deployment,
                r.step.as_str()
            ));
        }
        self.logs.push(r.clone());
        Ok(())
    }
    pub fn of(&self, d: &DeploymentId) -> Vec<&StepLogRec> {
        self.logs.iter().filter(|x| x.deployment == *d).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep() -> DeploymentId {
        "dep_00000000000000000000000010".parse().unwrap()
    }

    #[test]
    fn records_round_trip_are_bounded_and_one_per_step() {
        let r = StepLogRec::from_capture(
            dep(),
            Step::Build,
            "line one\n[REDACTED:K] two\nline é\n",
            false,
            false,
            5,
        );
        let enc = encode(&r);
        let body = enc.split_once('\t').unwrap().1;
        let f: Vec<&str> = body.split('\t').collect();
        assert_eq!(decode(&f).unwrap(), r);
        // a long capture keeps its tail, says it was cut, and stays under the bound
        let big = format!("{}END\n", "x\n".repeat(MAX_BYTES));
        let cut = StepLogRec::from_capture(dep(), Step::Build, &big, false, false, 1);
        assert!(cut.truncated && cut.text.len() <= MAX_BYTES + 128 && cut.text.ends_with("END\n"));
        // a withheld capture carries no text at all
        let w = StepLogRec::from_capture(dep(), Step::Build, "raw secret", true, false, 1);
        assert!(w.withheld && w.text.is_empty());
        let body = encode(&w);
        let f: Vec<&str> = body.split_once('\t').unwrap().1.split('\t').collect();
        assert_eq!(decode(&f).unwrap(), w);
        let mut s = LogState::default();
        s.apply(&r).unwrap();
        assert!(s.apply(&r).is_err());
        s.apply(&StepLogRec {
            step: Step::Source,
            ..r.clone()
        })
        .unwrap();
        assert_eq!(s.of(&dep()).len(), 2);
        for bad in [
            vec!["deplog", "1", "x"],
            vec!["deplog", "2", "d", "build", "0", "0", "1", "-"],
        ] {
            assert!(decode(&bad).is_err());
        }
    }
}
