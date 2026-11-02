//! Git sources and triggers for Harpoon deployments (Milestone D). Attended use only: everything here
//! runs inside a developer-started process; nothing is a daemon, and nothing claims unattended operation.

pub mod runner;
pub mod sha256;
pub mod webhook;

pub use runner::{
    branch_heads, checkout, leftovers, ls_remote, ssh_command, sweep, Checkout, Credential, GitError, RemoteRef,
    Selector, Settings, Workspace,
};
