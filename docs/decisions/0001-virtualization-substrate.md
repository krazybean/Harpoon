# 0001. Virtualization substrate — Apple Virtualization.framework

- **Status:** accepted (Spike 1 validates)
- **Date:** 2026-08-24

## Context

Linux containers need a Linux kernel. On macOS, VM is mandatory. Options: Virtualization.framework, QEMU, hyperkit, etc.

## Decision

We will use Apple `Virtualization.framework` (`VZVirtualMachine`, `VZLinuxBootLoader`, `VZGenericPlatformConfiguration`) as sole hypervisor for v0.1 on Apple Silicon.

## Alternatives Considered

- QEMU/hyperkit: more config options but extra dependency, slower, less integrated with macOS memory/compression.
- Multiple hypervisor abstraction: rejected per ponytail — one seam first.

## Consequences

- Requires `com.apple.security.virtualization` entitlement and signing.
- Ties MVP to macOS 12+ and Apple toolchain; limits portability, acceptable for MVP.

## Validation

Spike 1 boots ARM64 kernel via `VZLinuxBootLoader` and proves host-visible boot; `isSupported==true` check on host; `validate()` error with missing entitlement documented.

