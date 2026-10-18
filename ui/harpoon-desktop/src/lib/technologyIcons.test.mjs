import assert from "node:assert/strict";
import test from "node:test";
import { TECHNOLOGY_ICONS } from "./technologyIcons.js";
import { TECHNOLOGY_DISPLAY_COLORS } from "./technologyIconColors.js";

const keys = ["postgres", "redis", "nginx", "node", "mysql", "mariadb", "mongo", "rabbitmq", "traefik", "caddy", "python", "alpine"];

function luminance(hex) {
  const rgb = [0, 2, 4].map((offset) => parseInt(hex.slice(offset + 1, offset + 3), 16) / 255);
  const linear = rgb.map((value) => value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4);
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}

test("bundles exactly the curated technology marks with recognizable paths", () => {
  assert.deepEqual(Object.keys(TECHNOLOGY_ICONS), keys);
  for (const key of keys) {
    assert.match(TECHNOLOGY_ICONS[key].path, /\S/);
    assert.match(TECHNOLOGY_ICONS[key].color, /^#[0-9A-F]{6}$/);
  }
});

test("dark brand colors have accessible graphite display colors", () => {
  const graphite = luminance("#171B1F");
  for (const [key, color] of Object.entries(TECHNOLOGY_DISPLAY_COLORS)) {
    const ratio = (Math.max(graphite, luminance(color)) + 0.05) / (Math.min(graphite, luminance(color)) + 0.05);
    assert.ok(ratio >= 3, `${key} display color contrast ${ratio.toFixed(2)} is below 3:1`);
  }
});
