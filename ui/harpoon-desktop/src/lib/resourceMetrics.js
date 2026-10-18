export const HISTORY_LIMIT = 180;

export function boundedSamples(samples, limit = HISTORY_LIMIT) {
  return Array.isArray(samples) ? samples.slice(-limit) : [];
}

export function numericSeries(samples, key) {
  return boundedSamples(samples).map((sample) => typeof sample?.[key] === "number" && Number.isFinite(sample[key]) ? sample[key] : null);
}

export function latestNumber(samples, key) {
  const values = numericSeries(samples, key).filter((value) => value !== null);
  return values.length ? values[values.length - 1] : null;
}

export function sparklinePoints(samples, key, width = 120, height = 32) {
  const values = numericSeries(samples, key);
  const present = values.filter((value) => value !== null);
  if (present.length < 2) return "";
  const min = Math.min(...present);
  const max = Math.max(...present);
  const range = max - min || 1;
  const step = width / Math.max(values.length - 1, 1);
  return values.map((value, index) => value === null ? null : `${(index * step).toFixed(1)},${(height - ((value - min) / range) * (height - 4) - 2).toFixed(1)}`).filter(Boolean).join(" ");
}

export function formatMetric(value, unit) {
  if (value === null || value === undefined || !Number.isFinite(value)) return "N/A";
  if (unit === "%") return `${value.toFixed(0)}%`;
  const gib = value / (1024 ** 3);
  return `${gib >= 10 ? gib.toFixed(0) : gib.toFixed(1)} GiB`;
}

export function resourceMetricDescription(scope, label, value, note) {
  return `${scope} ${label}: ${value}. ${note}`;
}
