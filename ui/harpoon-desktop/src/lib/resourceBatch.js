export const resourceId = (resource) => String(resource.ID || resource.Id || "");

export const selectAll = (resources) => new Set(resources.map(resourceId).filter(Boolean));
export const clearSelection = () => new Set();
export const toggleSelection = (selected, id) => {
  const next = new Set(selected);
  next.has(id) ? next.delete(id) : next.add(id);
  return next;
};
export const pruneSelection = (selected, resources) => {
  const available = selectAll(resources);
  return new Set([...selected].filter((id) => available.has(id)));
};

export async function runSequential(ids, operation) {
  const succeeded = [];
  const failed = [];
  for (const id of ids) {
    try { await operation(id); succeeded.push(id); }
    catch (error) { failed.push({ id, error: String(error) }); }
  }
  return { succeeded, failed };
}

export function deletePrompt(kind, names) {
  return { title: `Delete ${names.length} ${kind}?`, names: names.length <= 6 ? names : [] };
}
