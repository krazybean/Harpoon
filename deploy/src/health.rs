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
    /// The status class the observations are judged against; carried so the evidence can be
    /// re-validated without trusting `verdict`.
    pub expect: StatusClass,
    /// True when the observations were scripted by a fake, not made against a container.
    pub simulated: bool,
    pub attempts: Vec<Attempt>,
    pub consecutive_required: u8,
    pub consecutive_reached: u8,
    pub elapsed_ms: u64,
    pub verdict: Verdict,
    pub scope_note: &'static str,
}

impl CheckEvidence {
    /// Healthy only if the verdict says so *and* the observations back it up. A verdict alone
    /// establishes nothing.
    pub fn is_healthy(&self) -> bool {
        self.validate().is_ok()
    }

    /// Re-derive the verdict from the observations. Requires a positive threshold, at least
    /// `consecutive_required` trailing accepted observations made while the container was
    /// running, and `consecutive_reached` equal to that trailing run.
    pub fn validate(&self) -> Result<(), String> {
        if self.verdict != Verdict::Healthy {
            return Err(format!("verdict is {:?}", self.verdict));
        }
        if self.consecutive_required == 0 {
            return Err("no positive consecutive-success threshold configured".into());
        }
        if self.attempts.is_empty() {
            return Err("no health observation was made".into());
        }
        let run = self
            .attempts
            .iter()
            .rev()
            .take_while(|a| {
                a.container_running && matches!(&a.observation, Observation::Status(s) if self.expect.accepts(*s))
            })
            .count();
        if run < usize::from(self.consecutive_required) {
            return Err(format!(
                "observations show {run} trailing successes while running, {} required",
                self.consecutive_required
            ));
        }
        if usize::from(self.consecutive_reached) != run.min(usize::from(u8::MAX)) {
            return Err(format!(
                "claims {} consecutive successes but observations show {run}",
                self.consecutive_reached
            ));
        }
        Ok(())
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

    /// Evidence for a check that was not run at all. It can never establish health.
    pub fn not_probed(target: &str) -> CheckEvidence {
        CheckEvidence {
            target: target.to_string(),
            expected: "no probe configured".to_string(),
            expect: StatusClass::Success2xx,
            simulated: false,
            attempts: Vec::new(),
            consecutive_required: 0,
            consecutive_reached: 0,
            elapsed_ms: 0,
            verdict: Verdict::NotHealthy,
            scope_note: SCOPE_NOTE,
        }
    }

    /// Scripted passing evidence for fakes: `spec.consecutive_ok` explicit HTTP 200
    /// observations against a running container, marked as simulated. No networking.
    pub fn simulated_passing(target: &str, spec: &HealthSpec) -> CheckEvidence {
        let n = spec.consecutive_ok.max(1);
        let attempts = (0..u64::from(n))
            .map(|i| Attempt {
                at_ms: i * spec.interval_ms,
                observation: Observation::Status(200),
                container_running: true,
            })
            .collect();
        CheckEvidence {
            target: target.to_string(),
            expected: format!("HTTP {:?}, {} consecutive (simulated)", spec.expect, n),
            expect: spec.expect,
            simulated: true,
            attempts,
            consecutive_required: n,
            consecutive_reached: n,
            elapsed_ms: u64::from(n - 1) * spec.interval_ms,
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
        if spec.consecutive_ok > 0 && run >= spec.consecutive_ok {
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
        expect: spec.expect,
        simulated: false,
        attempts,
        consecutive_required: spec.consecutive_ok,
        consecutive_reached: if verdict == Verdict::Healthy { run } else { best },
        elapsed_ms: clock.now_ms().saturating_sub(start),
        verdict,
        scope_note: SCOPE_NOTE,
    }
}
