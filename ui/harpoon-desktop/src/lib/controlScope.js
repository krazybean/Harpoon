export function containerLifecycleState(container) {
  const value = String(container?.State || container?.Status || "").toLowerCase();
  if (value.includes("restart")) return "warning";
  if (value.includes("dead") || value.includes("error")) return "error";
  if (value.includes("running") || value.includes("up")) return "running";
  return "stopped";
}

export function eligibleMembers(command, containers) {
  if (command === "start_container") return containers.filter((container) => containerLifecycleState(container) !== "running");
  if (command === "stop_container" || command === "restart_container") return containers.filter((container) => containerLifecycleState(container) === "running");
  return [];
}

export function actionSummary(command, containers) {
  const eligible = eligibleMembers(command, containers);
  return { eligible, eligibleCount: eligible.length, skippedCount: containers.length - eligible.length, total: containers.length };
}
