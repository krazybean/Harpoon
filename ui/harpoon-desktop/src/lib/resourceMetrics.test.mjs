import test from "node:test";
import assert from "node:assert/strict";
import { boundedSamples, formatMetric, latestNumber, numericSeries, resourceMetricDescription, sparklinePoints } from "./resourceMetrics.js";
import { subscribeResourceSampler } from "./resourceSamplerLifecycle.js";

test("resource history stays bounded and preserves unavailable gaps", () => {
  const samples = Array.from({ length: 181 }, (_, index) => ({ cpuPercent: index === 90 ? null : index }));
  assert.equal(boundedSamples(samples).length, 180);
  assert.equal(numericSeries(samples, "cpuPercent").filter((value) => value === null).length, 1);
  assert.equal(latestNumber(samples, "cpuPercent"), 180);
  assert.equal(sparklinePoints(samples, "missing"), "");
});

test("metric formatting keeps units explicit", () => {
  assert.equal(formatMetric(41.2, "%"), "41%");
  assert.equal(formatMetric(2 * 1024 ** 3, "bytes"), "2.0 GiB");
  assert.equal(formatMetric(null, "%"), "N/A");
});

test("compact meters keep host and image sources distinct", () => {
  assert.match(resourceMetricDescription("HOST", "RAM", "18.0 GiB", "macOS physical memory used"), /^HOST RAM:/);
  assert.match(resourceMetricDescription("IMAGE", "DISK", "0.9 GiB", "Harpoon disk-image allocation, not disk I\/O"), /IMAGE DISK:.*not disk I\/O/);
});

test("sampler listener cleans up even when listen resolves after unmount", async () => {
  let resolveListener;
  const events = [];
  const cleanup = subscribeResourceSampler({
    listen: () => new Promise((resolve) => { resolveListener = resolve; }),
    start: () => events.push("start"),
    stop: () => events.push("stop"),
    onSnapshot: () => events.push("snapshot"),
  });
  cleanup();
  let listenerCleaned = false;
  resolveListener(() => { listenerCleaned = true; });
  await Promise.resolve();
  assert.deepEqual(events, ["start", "stop"]);
  assert.equal(listenerCleaned, true);
});
