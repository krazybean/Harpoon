//! Injectable time and entropy, so identifiers and durations are deterministic in tests.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, Read};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock milliseconds since the Unix epoch.
pub trait Clock {
    fn now_ms(&self) -> u64;
}

impl<T: Clock + ?Sized> Clock for Rc<T> {
    fn now_ms(&self) -> u64 {
        (**self).now_ms()
    }
}

impl<T: Clock + ?Sized> Clock for &T {
    fn now_ms(&self) -> u64 {
        (**self).now_ms()
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A clock tests advance by hand. Shared through `Rc` so a fake backend can move it.
#[derive(Debug)]
pub struct ManualClock(Cell<u64>);

impl ManualClock {
    pub fn new(start_ms: u64) -> Rc<ManualClock> {
        Rc::new(ManualClock(Cell::new(start_ms)))
    }
    pub fn advance(&self, ms: u64) {
        self.0.set(self.0.get() + ms);
    }
    pub fn set(&self, ms: u64) {
        self.0.set(ms);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}

/// A source of random bytes.
pub trait Entropy {
    fn fill(&mut self, buf: &mut [u8]) -> io::Result<()>;
}

/// Operating-system entropy (`/dev/urandom`). Unix only, which is all Harpoon targets.
pub struct OsEntropy(Option<File>);

impl OsEntropy {
    pub fn new() -> OsEntropy {
        OsEntropy(None)
    }
}

impl Default for OsEntropy {
    fn default() -> Self {
        Self::new()
    }
}

impl Entropy for OsEntropy {
    fn fill(&mut self, buf: &mut [u8]) -> io::Result<()> {
        if self.0.is_none() {
            self.0 = Some(File::open("/dev/urandom")?);
        }
        self.0.as_mut().expect("opened above").read_exact(buf)
    }
}

/// Deterministic entropy for tests: a repeating byte sequence.
pub struct SeqEntropy {
    next: u8,
}

impl SeqEntropy {
    pub fn new(start: u8) -> SeqEntropy {
        SeqEntropy { next: start }
    }
}

impl Entropy for SeqEntropy {
    fn fill(&mut self, buf: &mut [u8]) -> io::Result<()> {
        for b in buf {
            *b = self.next;
            self.next = self.next.wrapping_add(1);
        }
        Ok(())
    }
}
