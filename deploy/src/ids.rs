//! Stable, typed, sortable identifiers: `<prefix>_<26 Crockford-base32 chars>`.
//!
//! The 26 characters encode 128 bits: a 48-bit millisecond timestamp followed by 80 bits
//! of entropy (the ULID layout), so identifiers sort by creation time and are safe to
//! use as Docker label values. Identifiers are **never reused and never derived from a
//! name**, so renames cannot break the link between records and Docker objects.
//!
//! A release has no identifier of its own beyond its deployment: per the spec (D10),
//! "release identity equals the deployment ID of the successful build", so
//! [`ReleaseId::from_deployment`] re-prefixes the same 26 characters.

use crate::time::{Clock, Entropy};
use std::fmt;
use std::io;
use std::str::FromStr;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const BODY_LEN: usize = 26;
const TS_MASK: u64 = (1u64 << 48) - 1;
const RAND_MASK: u128 = (1u128 << 80) - 1;

#[derive(Debug)]
pub enum IdError {
    Entropy(io::Error),
    Parse(ParseIdError),
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::Entropy(e) => write!(f, "entropy source failed: {e}"),
            IdError::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for IdError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseIdError {
    WrongPrefix { expected: &'static str },
    BadLength,
    BadCharacter(char),
    Overflow,
}

impl fmt::Display for ParseIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseIdError::WrongPrefix { expected } => write!(f, "identifier must start with `{expected}_`"),
            ParseIdError::BadLength => write!(f, "identifier body must be {BODY_LEN} characters"),
            ParseIdError::BadCharacter(c) => write!(f, "invalid identifier character {c:?}"),
            ParseIdError::Overflow => write!(f, "identifier value exceeds 128 bits"),
        }
    }
}

impl std::error::Error for ParseIdError {}

fn encode(value: u128) -> String {
    let mut out = [0u8; BODY_LEN];
    let mut v = value;
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[(v & 31) as usize];
        v >>= 5;
    }
    String::from_utf8(out.to_vec()).expect("alphabet is ASCII")
}

fn decode(body: &str) -> Result<u128, ParseIdError> {
    if body.len() != BODY_LEN {
        return Err(ParseIdError::BadLength);
    }
    let mut v: u128 = 0;
    for (i, c) in body.chars().enumerate() {
        let d = ALPHABET
            .iter()
            .position(|a| *a as char == c)
            .ok_or(ParseIdError::BadCharacter(c))? as u128;
        if i == 0 && d > 7 {
            return Err(ParseIdError::Overflow); // 26 * 5 = 130 bits; the top two must be zero
        }
        v = (v << 5) | d;
    }
    Ok(v)
}

macro_rules! define_id {
    ($(#[$m:meta])* $name:ident, $prefix:literal) => {
        $(#[$m])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            fn from_raw(raw: u128) -> $name {
                $name(format!("{}_{}", $prefix, encode(raw)))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Creation time encoded in the identifier, in milliseconds since the epoch.
            pub fn timestamp_ms(&self) -> u64 {
                let raw = decode(&self.0[$prefix.len() + 1..]).expect("validated at construction");
                (raw >> 80) as u64
            }

            #[allow(dead_code)]
            fn raw(&self) -> u128 {
                decode(&self.0[$prefix.len() + 1..]).expect("validated at construction")
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let rest = s
                    .strip_prefix($prefix)
                    .and_then(|r| r.strip_prefix('_'))
                    .ok_or(ParseIdError::WrongPrefix { expected: $prefix })?;
                decode(rest)?;
                Ok($name(s.to_string()))
            }
        }
    };
}

define_id!(
    /// A group of applications.
    ProjectId, "prj"
);
define_id!(
    /// One deployable unit. Written to the `io.harpoon.app` Docker label.
    ApplicationId, "app"
);
define_id!(
    /// One deployment attempt. Written to the `io.harpoon.deployment` Docker label.
    DeploymentId, "dep"
);
define_id!(
    /// The immutable result of a successful build; shares its characters with the deployment.
    ReleaseId, "rel"
);

impl ReleaseId {
    pub fn from_deployment(d: &DeploymentId) -> ReleaseId {
        ReleaseId::from_raw(d.raw())
    }
}

/// Generates identifiers. Monotonic: two identifiers from one generator always sort in
/// creation order, even within one millisecond or if the clock steps backwards.
pub struct IdGenerator<C: Clock, E: Entropy> {
    clock: C,
    entropy: E,
    last_ms: u64,
    last_rand: u128,
    started: bool,
}

impl<C: Clock, E: Entropy> IdGenerator<C, E> {
    pub fn new(clock: C, entropy: E) -> Self {
        IdGenerator {
            clock,
            entropy,
            last_ms: 0,
            last_rand: 0,
            started: false,
        }
    }

    fn next_raw(&mut self) -> Result<u128, IdError> {
        let now = self.clock.now_ms() & TS_MASK;
        let mut ms = if self.started { now.max(self.last_ms) } else { now };
        let rand = if self.started && ms == self.last_ms && self.last_rand < RAND_MASK {
            self.last_rand + 1
        } else {
            if self.started && ms == self.last_ms {
                ms = (ms + 1) & TS_MASK; // 80-bit counter exhausted within one ms: step time forward
            }
            let mut b = [0u8; 10];
            self.entropy.fill(&mut b).map_err(IdError::Entropy)?;
            b.iter().fold(0u128, |a, x| (a << 8) | *x as u128)
        };
        self.started = true;
        self.last_ms = ms;
        self.last_rand = rand;
        Ok(((ms as u128) << 80) | rand)
    }

    pub fn project(&mut self) -> Result<ProjectId, IdError> {
        Ok(ProjectId::from_raw(self.next_raw()?))
    }
    pub fn application(&mut self) -> Result<ApplicationId, IdError> {
        Ok(ApplicationId::from_raw(self.next_raw()?))
    }
    pub fn deployment(&mut self) -> Result<DeploymentId, IdError> {
        Ok(DeploymentId::from_raw(self.next_raw()?))
    }
}

/// Docker label keys the engine stamps on every object it creates (spec D10).
/// Only these three; no state is stored in labels.
pub mod labels {
    pub const MANAGED: &str = "io.harpoon.managed";
    pub const APP: &str = "io.harpoon.app";
    pub const DEPLOYMENT: &str = "io.harpoon.deployment";

    /// The exact label set for one deployment of one application.
    pub fn for_deployment(app: &super::ApplicationId, dep: &super::DeploymentId) -> [(&'static str, String); 3] {
        [
            (MANAGED, "true".to_string()),
            (APP, app.to_string()),
            (DEPLOYMENT, dep.to_string()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{ManualClock, SeqEntropy};

    fn gen(
        ms: u64,
    ) -> (
        IdGenerator<std::rc::Rc<ManualClock>, SeqEntropy>,
        std::rc::Rc<ManualClock>,
    ) {
        let c = ManualClock::new(ms);
        (IdGenerator::new(c.clone(), SeqEntropy::new(1)), c)
    }

    #[test]
    fn format_prefix_and_length() {
        let (mut g, _) = gen(1_700_000_000_000);
        let id = g.deployment().unwrap();
        assert!(id.as_str().starts_with("dep_"));
        assert_eq!(id.as_str().len(), 4 + 26);
        assert_eq!(id.timestamp_ms(), 1_700_000_000_000);
    }

    #[test]
    fn roundtrips_through_parse() {
        let (mut g, _) = gen(42);
        let id = g.application().unwrap();
        let parsed: ApplicationId = id.to_string().parse().unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn parse_rejects_wrong_kind_length_and_characters() {
        let (mut g, _) = gen(42);
        let dep = g.deployment().unwrap().to_string();
        assert!(matches!(
            dep.parse::<ApplicationId>(),
            Err(ParseIdError::WrongPrefix { .. })
        ));
        assert_eq!("dep_ABC".parse::<DeploymentId>(), Err(ParseIdError::BadLength));
        let bad = format!("dep_{}", "I".repeat(26)); // `I` is not in the alphabet
        assert_eq!(bad.parse::<DeploymentId>(), Err(ParseIdError::BadCharacter('I')));
        let big = format!("dep_8{}", "0".repeat(25));
        assert_eq!(big.parse::<DeploymentId>(), Err(ParseIdError::Overflow));
        assert!("".parse::<DeploymentId>().is_err());
        assert!("dep".parse::<DeploymentId>().is_err());
    }

    #[test]
    fn monotonic_within_one_millisecond() {
        let (mut g, _) = gen(1000);
        let ids: Vec<_> = (0..50).map(|_| g.deployment().unwrap()).collect();
        for w in ids.windows(2) {
            assert!(w[0] < w[1], "{:?} !< {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn monotonic_when_the_clock_steps_backwards() {
        let (mut g, c) = gen(5000);
        let a = g.deployment().unwrap();
        c.set(1000);
        let b = g.deployment().unwrap();
        assert!(a < b);
        assert_eq!(b.timestamp_ms(), 5000, "timestamp must not move backwards");
    }

    #[test]
    fn later_time_sorts_later() {
        let (mut g, c) = gen(1000);
        let a = g.project().unwrap();
        c.advance(1);
        let b = g.project().unwrap();
        assert!(a < b);
    }

    #[test]
    fn counter_exhaustion_steps_time_forward_and_stays_sorted() {
        let (mut g, _) = gen(1000);
        let a = g.deployment().unwrap();
        g.last_rand = RAND_MASK; // force the 80-bit counter to its limit
        let b = g.deployment().unwrap();
        assert!(a < b);
        assert_eq!(b.timestamp_ms(), 1001);
    }

    #[test]
    fn release_id_shares_characters_with_its_deployment() {
        let (mut g, _) = gen(77);
        let d = g.deployment().unwrap();
        let r = ReleaseId::from_deployment(&d);
        assert_eq!(&r.as_str()[4..], &d.as_str()[4..]);
        assert!(r.as_str().starts_with("rel_"));
        assert_eq!(r.timestamp_ms(), d.timestamp_ms());
    }

    #[test]
    fn distinct_generators_with_os_entropy_do_not_collide() {
        use crate::time::{OsEntropy, SystemClock};
        let mut a = IdGenerator::new(SystemClock, OsEntropy::new());
        let mut b = IdGenerator::new(SystemClock, OsEntropy::new());
        let set: std::collections::HashSet<_> = (0..200)
            .flat_map(|_| [a.deployment().unwrap(), b.deployment().unwrap()])
            .collect();
        assert_eq!(set.len(), 400);
    }

    #[test]
    fn labels_are_exactly_the_three_documented_keys() {
        let (mut g, _) = gen(9);
        let (app, dep) = (g.application().unwrap(), g.deployment().unwrap());
        let l = labels::for_deployment(&app, &dep);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0], ("io.harpoon.managed", "true".to_string()));
        assert_eq!(l[1].1, app.to_string());
        assert_eq!(l[2].1, dep.to_string());
    }

    #[test]
    fn entropy_failure_is_reported_not_swallowed() {
        struct Broken;
        impl Entropy for Broken {
            fn fill(&mut self, _: &mut [u8]) -> io::Result<()> {
                Err(io::Error::other("no entropy"))
            }
        }
        let mut g = IdGenerator::new(ManualClock::new(1), Broken);
        assert!(matches!(g.deployment(), Err(IdError::Entropy(_))));
    }
}
