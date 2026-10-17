# R1 Startup Diagnostics — Code-Only Instrumentation

Status: IMPLEMENTED / LIVE REPRODUCTION NOT AUTHORIZED

The native startup failure callback in `harpoon/Sources/main.swift` now emits
bounded `HARPOON_VZ_START_*` records before the existing cleanup and exit path.
The change does not retry, delay, reorder, or otherwise alter VM lifecycle
behavior.

## Collected data

- VM state at callback time and validated configuration component counts.
- Kernel, initramfs, and root-disk path, existence, regular-file status, byte
  size, and file-resource identity metadata. Guest contents, credentials, and
  environment values are not read.
- NSError domain, code, localized description, bounded userInfo keys and safe
  scalar values, plus recursively captured `NSUnderlyingError` values.
- Sensitive-looking userInfo values are redacted; recursion, entry count, and
  text length are bounded to keep logs safe and finite.

## Reproduction and collection plan

After separate authorization for live startup tests, run the existing paired
R1 minimal/full method and preserve one complete startup log per attempt. Do
not add retries or modify VM/guest assets. Compare the new
`HARPOON_VZ_START_ERROR`, `HARPOON_VZ_START_DIAGNOSTICS`, and
`HARPOON_VZ_START_ASSET` records between healthy and failing events, especially
the nested error chain, state, component counts, and asset identities.

Until that authorization is granted, this change is validated only offline;
it does not claim the host/framework failure is fixed or that a new live result
exists.
