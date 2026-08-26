# 0002. Rust vs Swift bridge for Virtualization.framework

- **Status:** proposed (Spike 1 decides)
- **Date:** 2026-08-24

## Context

Daemon is Rust, but Virtualization.framework is ObjC/Swift with block callbacks.

## Decision

Prefer Rust `objc2`/`block2`/`objc2-foundation` direct bindings if concise; otherwise allow minimal Swift bridge (one Swift file compiled with `swiftc -framework Virtualization`, called from Rust via FFI or subprocess). Do not contort Rust for language purity.

## Alternatives Considered

- Pure Swift daemon: rejected — Rust owns policy/state.
- Third-party crate `arcbox-vz`: rejected — no stable provenance for MVP.

## Consequences

- Build needs Xcode CLT + Swift toolchain + entitlement signing step.
- Spike 1 will produce both candidates and record smaller/cleaner in ADR.

