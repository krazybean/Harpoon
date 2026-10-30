//! Structured health-check evidence (review gap #5).
//!
//! A check never returns a bare "ok". It returns *what was observed*: the target, each
//! attempt's outcome, how many consecutive successes were required and reached, and the
//! time spent. The engine refuses to record a passing step whose evidence does not say
//! `Healthy`, and every piece of evidence carries the scope note: this is evidence about a
//! local container, not a guarantee about any other environment.

use crate::time::Clock;

pub const SCOPE_NOTE: &str = "local container only — not a guarantee of behaviour on Vercel or any other environment";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusClass {
    Success2xx,
    NotServerError, // < 500
}

impl StatusClass {
    pub fn accepts(self, status: u16) -> bool {
        match self {
            StatusClass::Success2xx => (200..300).contains(&status),
            StatusClass::NotServerError => status < 500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthSpec {
    pub path: String,
    pub expect: StatusClass,
    pub interval_ms: u64,
    pub consecutive_ok: u8,
    pub startup_deadline_ms: u64,
}

impl HealthSpec {
    /// First-test defaults: 60 s startup deadline, 500 ms interval, 2 consecutive successes.
    pub fn first_test(path: &str) -> HealthSpec {
        HealthSpec {
            path: path.to_string(),
            expect: StatusClass::Success2xx,
            interval_ms: 500,
            consecutive_ok: 2,
            startup_deadline_ms: 60_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    Status(u16),
    ConnectFailed(String),
    /// The container stopped; it cannot be healthy whatever the port says.
    ContainerExited(Option<i32>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub at_ms: u64,
    pub observation: Observation,
    /// Whether the container was still running when this attempt finished.
    pub container_running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Healthy,
    /// The deadline passed without enough consecutive successes.
    NotHealthy,
    /// The container exited during the check.
    Exited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckEvidence {
    pub target: String,
    pub expected: String,
    pub attempts: Vec<Attempt>,
    pub consecutive_required: u8,
    pub consecutive_reached: u8,
    pub elapsed_ms: u64,
    pub verdict: Verdict,
    pub scope_note: &'static str,
}

impl CheckEvidence {
    pub fn is_healthy(&self) -> bool {
        self.verdict == Verdict::Healthy
    }

    pub fn last_observation(&self) -> Option<&Observation> {
        self.attempts.last().map(|a| &a.observation)
    }

    /// A one-line statement of what was actually seen, for failure messages.
    pub fn summary(&self) -> String {
        let last = match self.last_observation() {
            Some(Observation::Status(s)) => format!("last response HTTP {s}"),
            Some(Observation::ConnectFailed(e)) => format!("last attempt: {e}"),
            Some(Observation::ContainerExited(c)) => format!(
                "container exited{}",
                c.map(|c| format!(" with code {c}")).unwrap_or_default()
            ),
            None => "no attempt completed".to_string(),
        };
        format!(
            "{} attempts, {}/{} consecutive successes over {} ms; {last}",
            self.attempts.len(),
            self.consecutive_reached,
            self.consecutive_required,
            self.elapsed_ms
        )
    }

    /// Evidence for a check that was not run at all (a fake that does not probe).
    pub fn not_probed(target: &str) -> CheckEvidence {
        CheckEvidence {
            target: target.to_string(),
            expected: "no probe configured".to_string(),
            attempts: Vec::new(),
            consecutive_required: 0,
            consecutive_reached: 0,
            elapsed_ms: 0,
            verdict: Verdict::Healthy,
            scope_note: SCOPE_NOTE,
        }
    }
}

/// The prober the real backend will provide: one HTTP/TCP attempt against the host port.
pub trait Probe {
    fn attempt(&mut self, at_ms: u64) -> Attempt;
}

/// Lets a test advance a manual clock instead of sleeping.
pub trait Waiter {
    fn wait_ms(&self, ms: u64);
}

impl Waiter for crate::time::ManualClock {
    fn wait_ms(&self, ms: u64) {
        self.advance(ms);
    }
}

impl<T: Waiter + ?Sized> Waiter for std::rc::Rc<T> {
    fn wait_ms(&self, ms: u64) {
        (**self).wait_ms(ms)
    }
}

/// Poll until `consecutive_ok` successes in a row, the container exits, or the deadline.
/// Pure given the probe, clock and waiter.
pub fn run_health(
    spec: &HealthSpec,
    target: &str,
    probe: &mut dyn Probe,
    clock: &dyn Clock,
    waiter: &dyn Waiter,
) -> CheckEvidence {
    let start = clock.now_ms();
    let mut attempts = Vec::new();
    let mut run = 0u8;
    let mut best = 0u8;
    let verdict = loop {
        let a = probe.attempt(clock.now_ms());
        let ok = matches!(&a.observation, Observation::Status(s) if spec.expect.accepts(*s)) && a.container_running;
        let exited = matches!(a.observation, Observation::ContainerExited(_)) || !a.container_running;
        attempts.push(a);
        if exited {
            break Verdict::Exited;
        }
        run = if ok { run.saturating_add(1) } else { 0 };
        best = best.max(run);
        if run >= spec.consecutive_ok {
            break Verdict::Healthy;
        }
        if clock.now_ms().saturating_sub(start) + spec.interval_ms > spec.startup_deadline_ms {
            break Verdict::NotHealthy;
        }
        waiter.wait_ms(spec.interval_ms);
    };
    CheckEvidence {
        target: target.to_string(),
        expected: format!("HTTP {:?}, {} consecutive", spec.expect, spec.consecutive_ok),
        attempts,
        consecutive_required: spec.consecutive_ok,
        consecutive_reached: if verdict == Verdict::Healthy { run } else { best },
        elapsed_ms: clock.now_ms().saturating_sub(start),
        verdict,
        scope_note: SCOPE_NOTE,
    }
}
