//! Guest-side free space, fail closed.
//!
//! The Docker Engine API reports usage (`/system/df`) but not free space, and Harpoon has no guest
//! disk report of its own, so there is nothing in this crate that can *measure* the guest. What
//! this module fixes is the rule: a build proceeds only if a probe positively reports at least
//! `required` free bytes. An error, a zero, a missing probe, or too little are all refusals.
//! B7's measuring mechanism (a short-lived, labelled, ownership-verified container running
//! `df -Pk /`) is specified in `docs/results/DEPLOY_B7_LIVE_ACCEPTANCE.md`; it is not implemented here.

use crate::preflight::parse_df;

pub trait GuestDiskProbe {
    /// Free bytes on the guest filesystem that holds Docker's data.
    fn free_bytes(&self) -> Result<u64, String>;
}

/// Always refuses, naming why. What a live deployment gets until a real probe exists.
pub struct Unavailable(pub String);

impl GuestDiskProbe for Unavailable {
    fn free_bytes(&self) -> Result<u64, String> {
        Err(self.0.clone())
    }
}

/// Free bytes from the output of `df -Pk <path>` run in the guest.
pub fn free_from_df(output: &str) -> Result<u64, String> {
    parse_df(output)
}

pub fn check_guest_free(probe: &dyn GuestDiskProbe, required: u64) -> Result<u64, String> {
    let free = probe
        .free_bytes()
        .map_err(|e| format!("guest free space could not be verified: {e}"))?;
    if free == 0 {
        return Err("the guest reports no free space (or a value that cannot be trusted)".into());
    }
    if free < required {
        return Err(format!(
            "only {} MB free in the guest; {} MB are required (2 GiB reserve plus the build context). Harpoon never frees space by pruning.",
            free / 1_000_000,
            required / 1_000_000
        ));
    }
    Ok(free)
}
