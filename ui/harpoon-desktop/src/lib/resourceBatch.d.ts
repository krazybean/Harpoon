export function resourceId(resource: any): string;
export function selectAll(resources: any[]): Set<string>;
export function clearSelection(): Set<string>;
export function toggleSelection(selected: Set<string>, id: string): Set<string>;
export function pruneSelection(selected: Set<string>, resources: any[]): Set<string>;
export function runSequential(ids: string[], operation: (id: string) => Promise<unknown>): Promise<{ succeeded: string[]; failed: { id: string; error: string }[] }>;
export function deletePrompt(kind: string, names: string[]): { title: string; names: string[] };
