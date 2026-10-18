import type { ContainerTechnology } from "../lib/containerTechnology.js";
import { ImageSquare } from "@phosphor-icons/react";
import { TECHNOLOGY_ICONS } from "../lib/technologyIcons.js";
import { TECHNOLOGY_DISPLAY_COLORS } from "../lib/technologyIconColors.js";

export function ContainerTechnologyIcon({ technology, size = 18 }: { technology: ContainerTechnology; size?: number }) {
  const mark = TECHNOLOGY_ICONS[technology.key];
  if (!mark) return <ImageSquare className="container-technology-icon" size={size} weight="regular" aria-hidden="true" />;
  return <svg className="container-technology-icon" width={size} height={size} viewBox="0 0 24 24" aria-hidden="true" focusable="false" fill={TECHNOLOGY_DISPLAY_COLORS[technology.key as keyof typeof TECHNOLOGY_DISPLAY_COLORS] ?? mark.color}>
    <path d={mark.path} />
  </svg>;
}
