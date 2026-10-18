import { memo, useSyncExternalStore } from "react";
import { formatMetric, latestNumber, resourceMetricDescription, sparklinePoints } from "../lib/resourceMetrics.js";
import { getResourceSnapshot, subscribeResourceSnapshot } from "../lib/resourceSamplerStore";

function Meter({ label, scope, keyName, unit, note }: { label: string; scope: "HOST" | "IMAGE"; keyName: "cpuPercent" | "memoryUsedBytes" | "diskAllocatedBytes"; unit: "%" | "bytes"; note: string }) {
  const snapshot = useSyncExternalStore(subscribeResourceSnapshot, getResourceSnapshot, () => null);
  const samples = snapshot?.samples ?? [];
  const value = formatMetric(latestNumber(samples, keyName), unit);
  const points = sparklinePoints(samples, keyName, 44, 14);
  const description = resourceMetricDescription(scope, label, value, note);
  return <div className="resource-meter" title={description} aria-label={description}>
    <span className="resource-meter__scope">{scope}</span>
    <span className="resource-meter__label">{label}</span>
    {points ? <svg className="resource-meter__spark" viewBox="0 0 44 14" role="img" aria-label={`${label} recent trend`}><polyline points={points} fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg> : <span className="resource-meter__empty" aria-hidden="true">—</span>}
    <strong>{value}</strong>
  </div>;
}

export const ResourceMeters = memo(function ResourceMeters() {
  return <div className="resource-meters" aria-label="Host and image resource meters">
    <Meter label="CPU" scope="HOST" keyName="cpuPercent" unit="%" note="macOS host utilization" />
    <Meter label="RAM" scope="HOST" keyName="memoryUsedBytes" unit="bytes" note="macOS physical memory used: total minus free" />
    <Meter label="DISK" scope="IMAGE" keyName="diskAllocatedBytes" unit="bytes" note="Harpoon disk-image allocation, not disk I/O" />
  </div>;
});
