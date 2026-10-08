## Harpoon 2.0 — Committed Design References

These assets establish the visual and interaction direction for Harpoon 2.0. They are **design references, not implemented application screenshots or authoritative runtime specifications**.

### 1. Harpoon 2.0 — Visual Design Concept

![Harpoon 2.0 Design Concept](assets/harpoon-2/harpoon-v2-concept.png)

**File:** [`assets/harpoon-2/harpoon-v2-concept.png`](assets/harpoon-2/harpoon-v2-concept.png)

Primary reference for:

- Dark graphite palette with restrained yellow accents
- Typography, iconography, spacing, borders, and elevation
- Consistent navigation and workspace layouts
- Compact operational metrics and data visualizations
- Responsive container management and contextual inspection

**Known limitation:** The main composition assumes an unusually wide window. Implementation must adapt to ordinary laptop dimensions. Large container-page graphs should instead become compact status-bar metrics linking to Resources.

### 2. Harpoon 2.0 — Network Topology

![Harpoon Network Topology](assets/harpoon-2/harpoon-network-topology.svg)

**File:** [`assets/harpoon-2/harpoon-network-topology.svg`](assets/harpoon-2/harpoon-network-topology.svg)

Primary reference for:

- Visualizing networks and their attached containers
- Interactive relationships between infrastructure objects
- Consistent node styling and connection rendering
- Contextual navigation to container and network details

**Design constraint:** Connections must represent actual engine-reported network attachments, not inferred traffic or fabricated relationships.

### Reference Authority

Visual references guide aesthetics, layout, and interaction design. The existing Harpoon implementation, supported engine capabilities, and canonical roadmap remain authoritative for runtime functionality.

See [Container Workspace Specification](harpoon-2-container-workspace-spec.md) for agreed behavior and acceptance criteria.