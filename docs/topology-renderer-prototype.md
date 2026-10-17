# Topology renderer A/B prototype and production decision

Status: **React Flow + Dagre promoted to production Container Topology** (2026-10-16 America/Chicago)

## Scope

The harness compares two isolated renderers against the same normalized,
Docker-authoritative fixture model:

- **A — Native SVG + Dagre:** keeps Harpoon's existing SVG ownership and uses
  Dagre only for constrained placement.
- **B — React Flow + Dagre:** uses `@xyflow/react` custom Harpoon nodes and
  edges, with the same Dagre positions and data.

The production Container Topology diagram now uses the React Flow + Dagre
renderer. The accessible Details fallback remains. No Docker, VM, guest,
container, or network polling was performed.

## Empirical checks

| Area | Result |
|---|---|
| Fixture sizes | PASS: 2, 5, 10, and 25 containers; 3 shared networks |
| Pathological names | PASS: service-aware labels, ellipsis, native title, and accessible full identifier |
| Multi-network membership | PASS: one container can attach to multiple networks; every graph edge is labeled `attachment` |
| Empty/unknown states | PASS: empty stays empty; unknown service/image stays explicit |
| Explicit links | PASS: `HostConfig.Links` is shown as metadata and never rendered as a topology edge |
| Keyboard labels | PASS in browser review: SVG nodes are focusable; React Flow nodes expose full accessible labels |
| Fit-to-view | PASS in browser review: React Flow 25-node view fits and exposes zoom/fit controls |
| Graph visual | PASS in browser review at the available 840px viewport; graphite/yellow styling and bundled Simple Icons render |
| 760px / ~1000×700 | INCOMPLETE: browser review used the available in-app viewport, not a dedicated 760px or 1000×700 capture |
| Tauri WKWebView CPU/input latency | NOT MEASURED: no Tauri runtime or Docker workload was started |

The first SVG pass exposed a real compatibility issue: SVG `foreignObject`
labels were not visible in the browser review. It was corrected to native SVG
`text`/`path` elements, which restored visible labels and Simple Icons while
keeping containment deterministic.

## Cost and complexity

The prototype adds development-only MIT packages `@xyflow/react` 12.12.0,
`dagre` 0.8.5, and `@types/dagre` 0.7.54. The graph renderer is loaded through
a development-only dynamic import, so the production bundle excludes it.

Measured production build output after isolation:

- Main JavaScript: **738.61 kB raw / 195.52 kB gzip**.
- Before dynamic isolation, the same build was **830.26 kB raw / 227.94 kB gzip**.
- The existing Vite chunk-size warning remains.

These are build measurements, not Tauri runtime performance measurements.

## How to use it

Run the frontend dev server and open:

`http://127.0.0.1:1420/?topology-prototype`

The harness has switchable controls for renderer A/B, 2/5/10/25 nodes, and
populated/unknown/empty state. The route is guarded by `import.meta.env.DEV`.

## Production semantics

The selected container and inspect-reported network attachments are the only
graph nodes. Running-container attachment edges use slow CSS animation;
stopped edges are static muted lines and unknown states are neutral. The UI
labels this explicitly as “Animated = running container attached to network ·
Not measured traffic”. Reduced-motion disables the animation. Explicit Docker
links remain Details metadata and are never graph edges.
