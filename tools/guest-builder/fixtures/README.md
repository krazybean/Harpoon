# Fixtures

`udhcpc-default.script` and `udhcpc.conf` are the DHCP client files of the running Harpoon guest (Alpine 3.22.0, package
`busybox-1.37.0-r18`, `/usr/share/udhcpc/default.script` and `/etc/udhcpc/udhcpc.conf`), copied byte for byte with
`harpoon exec -- cat <path>` (read-only). `regression-guest-dns.sh` executes the real script from here, so the test checks what
the guest actually runs, and refuses to run if a fixture's SHA-256 is not the one below.

| File | SHA-256 |
|---|---|
| `udhcpc-default.script` | `1cf94e3918ce389c722f4408490ae7334321a5ca2aec2c945b0c47662aa274a6` |
| `udhcpc.conf` | `5b372209e01cda07c87b8afa6d1ab3d7e8daf18a1f2332d744203a6dc289eb1a` |

If the guest's Alpine release changes these files, copy the new ones, update the hashes, and re-run the test: it will show whether
the renewal behavior it relies on (`NO_DNS` honored by `resolvconf()`, `renew()` always calling it) still holds.
