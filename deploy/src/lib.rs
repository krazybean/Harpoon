//! Harpoon deployment engine — **offline foundation** (Milestone B, step 0).
//!
//! What is here: stable typed identifiers, the deployment state machine, explicit
//! interrupted-operation handling, a per-application queue, and a deterministic
//! lifecycle driver that runs against a [`backend::Backend`] trait with a scripted
//! [`backend::FakeBackend`].
//!
//! What is deliberately **not** here: Docker calls, VM interaction, network listeners,
//! background processes, SQLite, Keychain, Git, serialization formats, a UI data
//! contract, or any provider abstraction. The [`backend::Backend`] trait is the
//! execution seam for the *local* runtime only; it is not a provider framework.
//!
//! Spec: `docs/deployment-control-plane-requirements.md` (D4, D5, D10).

pub mod backend;
pub mod buildx;
pub mod engine;
pub mod health;
pub mod ids;
pub mod journal_format;
pub mod ledger;
pub mod logs;
pub mod ports;
pub mod queue;
pub mod release;
pub mod state;
pub mod time;
