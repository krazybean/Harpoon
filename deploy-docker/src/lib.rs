//! Docker Engine API backend for the Harpoon deploy engine — Milestone B3.
//!
//! This crate is where socket and process I/O lives, so the engine crate (`harpoon-deploy`)
//! stays offline. It talks to exactly one configured Unix socket, invokes only the bundled
//! Buildx, and has no code path that prunes, pulls, or reads the user's Docker configuration.

pub mod api;
pub mod backend;
pub mod buildx_run;
pub mod confine;
pub mod dockerfile;
pub mod forwarder;
pub mod guest;
pub mod http;
pub mod ignore;
pub mod json;
pub mod preflight;
pub mod probe;
pub mod snapshot;

pub use backend::{DockerBackend, DockerConfig};
