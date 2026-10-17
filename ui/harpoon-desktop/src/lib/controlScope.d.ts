export type LifecycleState = "running" | "stopped" | "warning" | "error";
export function containerLifecycleState(container: any): LifecycleState;
export function eligibleMembers(command: string, containers: any[]): any[];
export function actionSummary(command: string, containers: any[]): { eligible: any[]; eligibleCount: number; skippedCount: number; total: number };
