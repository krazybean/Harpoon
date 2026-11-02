// Docker's published ports (a mapping inside Harpoon's VM) and the listeners actually bound on this Mac are two different
// statements. They are shown side by side, never one in place of the other, and a failed host check is "not verified".
import assert from "node:assert/strict";
import test from "node:test";
import { compactExposure, describeExposure, exposureOf, parseDockerPorts } from "./portExposure.ts";

const B7_PORTS = "0.0.0.0:38000->8080/tcp, [::]:38000->8080/tcp";
const L = (address, scope, port = 38000, pid = 67874, command = "harpoon") => ({ port, address, scope, pid, command });
const verified = (...listeners) => ({ verified: true, error: null, checkedMs: 1, listeners });

test("docker ps port text is parsed exactly as reported", () => {
  assert.deepEqual(parseDockerPorts(B7_PORTS), [
    { hostIp: "0.0.0.0", hostPort: 38000, containerPort: 8080, proto: "tcp" },
    { hostIp: "[::]", hostPort: 38000, containerPort: 8080, proto: "tcp" },
  ]);
  assert.deepEqual(parseDockerPorts("127.0.0.1:5432->5432/tcp"), [{ hostIp: "127.0.0.1", hostPort: 5432, containerPort: 5432, proto: "tcp" }]);
  assert.deepEqual(parseDockerPorts("8080/tcp, 9000/udp"), [], "an exposed but unpublished port is not a mapping");
  assert.deepEqual(parseDockerPorts(""), []);
  assert.deepEqual(parseDockerPorts(undefined), []);
});

test("Docker's wildcard bindings are reported as Docker's, beside a loopback-only host listener, and neither is rewritten", () => {
  const e = exposureOf(parseDockerPorts(B7_PORTS), verified(L("127.0.0.1:38000", "loopback")));
  assert.deepEqual(e.docker.map((m) => m.hostIp), ["0.0.0.0", "[::]"], "the wildcard bindings stay exactly as Docker reported them");
  assert.deepEqual(e.host.ports.map((p) => [p.port, p.reach, p.listeners.map((l) => l.address)]), [[38000, "loopback", ["127.0.0.1:38000"]]]);
  const d = describeExposure(e);
  assert.match(d.docker, /Docker reports 0\.0\.0\.0:38000→8080\/tcp, \[::\]:38000→8080\/tcp \(a mapping inside Harpoon's VM\)/);
  assert.deepEqual(d.host, ["Host listener 127.0.0.1:38000 · loopback only, reachable from this Mac only · harpoon (pid 67874)"]);
  assert.equal(d.hostTone, "ok");
  const c = compactExposure(e);
  assert.equal(c.docker, "Docker: 0.0.0.0:38000→8080 · [::]:38000→8080");
  assert.equal(c.host, "Mac listener: 127.0.0.1:38000 · loopback only");
});

test("a listener on every interface is reported as reachable from the network, whatever Docker said", () => {
  const e = exposureOf(parseDockerPorts("127.0.0.1:38000->8080/tcp"), verified(L("*:38000", "wildcard")));
  assert.equal(e.host.ports[0].reach, "network");
  const d = describeExposure(e);
  assert.match(d.host[0], /\*:38000 · reachable from other machines on the network/);
  assert.equal(d.hostTone, "warn");
  const mixed = exposureOf(parseDockerPorts(B7_PORTS), verified(L("127.0.0.1:38000", "loopback"), L("*:38000", "wildcard")));
  assert.equal(mixed.host.ports[0].reach, "mixed");
  assert.match(describeExposure(mixed).host[0], /some addresses are reachable from the network/);
});

test("no listener found is said, and a check that did not run claims nothing at all", () => {
  const none = exposureOf(parseDockerPorts(B7_PORTS), verified(L("127.0.0.1:9999", "loopback", 9999)));
  assert.equal(none.host.ports[0].reach, "none");
  assert.match(describeExposure(none).host[0], /No listener found on this Mac for port 38000/);
  for (const hl of [null, { verified: false, error: "cannot run /usr/sbin/lsof", checkedMs: 1, listeners: [] }]) {
    const e = exposureOf(parseDockerPorts(B7_PORTS), hl);
    assert.equal(e.host.state, "unverified");
    assert.deepEqual(e.host.ports, [], "no per-port conclusions");
    const d = describeExposure(e);
    assert.match(d.host[0], /^Host listener not verified/);
    assert.doesNotMatch(d.host.join(" "), /loopback only|reachable/, "verification is unavailable, so no exposure is claimed");
    assert.equal(compactExposure(e).host, "Mac listener: not verified");
    assert.equal(d.hostTone, "neutral");
  }
  assert.match(describeExposure(exposureOf(parseDockerPorts(B7_PORTS), { verified: false, error: "lsof did not finish in 5 s", checkedMs: 1, listeners: [] })).host[0], /lsof did not finish in 5 s/);
});

test("a container with no published ports has no exposure to describe", () => {
  const e = exposureOf([], verified());
  assert.equal(compactExposure(e).docker, "No published ports");
  assert.equal(compactExposure(e).host, "");
  assert.deepEqual(describeExposure(e).host, []);
});
