export const HISTORY_LIMIT: number;
export function boundedSamples(samples: unknown[], limit?: number): unknown[];
export function numericSeries(samples: unknown[], key: string): Array<number | null>;
export function latestNumber(samples: unknown[], key: string): number | null;
export function sparklinePoints(samples: unknown[], key: string, width?: number, height?: number): string;
export function formatMetric(value: number | null | undefined, unit: string): string;
export function resourceMetricDescription(scope: string, label: string, value: string, note: string): string;
