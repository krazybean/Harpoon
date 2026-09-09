import { useEffect, useState } from "react";
import { clearSelection, pruneSelection, resourceId, selectAll, toggleSelection } from "../lib/resourceBatch";

export function useResourceSelection(resources: any[]) {
  const [selected, setSelected] = useState<Set<string>>(()=>new Set());
  const resourceKey = resources.map(resourceId).join("\0");
  useEffect(()=>{ setSelected((current)=>pruneSelection(current, resources)); }, [resourceKey]);
  return {
    selected,
    toggle: (id: string)=>setSelected((current)=>toggleSelection(current, id)),
    selectAll: ()=>setSelected(selectAll(resources)),
    clear: ()=>setSelected(clearSelection()),
    remove: (ids: string[])=>setSelected((current)=>new Set([...current].filter((id)=>!ids.includes(id)))),
  };
}
