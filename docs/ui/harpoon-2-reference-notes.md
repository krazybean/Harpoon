# Harpoon 2.0 — Visual Reference Index

This index preserves the **design intent** established in the October 2026 design discussion. The original user-uploaded reference images and generated concept board were attached to that conversation; they are **not yet present in this repository**. Do not claim binary provenance or create imagined copies. Once original bytes are made available through a binary-capable Git workflow, place the approved assets in `docs/ui/assets/harpoon-2/`, add file-specific source/licensing notes, and update this index.

## Inspiration images contributed in discussion

1. **Dark card-customization interface** — dense-but-clear icon and type treatment, calm negative space, color contrast; inspiration for compact controls and shell.
2. **Dark analytics illustration** — restrained KPI cards and purposeful line/donut charts; inspiration for Resources and miniature sparklines.
3. **Vaulto dark finance dashboard** — atmospheric lighting and surface depth; gradient use was judged too assertive. Borrow only subtle depth.
4. **Blue smart-home dashboard** — thin borders establish affordances; activated items gain fill; likely strongest pattern for Harpoon group/tile interactions.

These inspirations are third-party visual references of uncertain redistribution rights. **Do not commit/re-host them without confirming provenance and permission**; use descriptive notes or stable credited source links where appropriate.

## Generated Harpoon concept

A single wide multi-screen concept board was created in the discussion. This is a **new illustrative study**, not a screenshot of working Harpoon. Accepted: dark graphite base with restrained yellow accents, consistent iconography, compact typography, linked metrics, coherent sidebar, contextual resources, and recognizable multiple workspaces. Rejected/modified: assumes an ultrawide window, over-emphasizes solid yellow sidebar selection, and allocates large container-page space to graphs better represented as compact status-bar sparklines linking to Resources.

**Pending:** commit the actual generated concept image via binary-capable Git transport after verifying its original source and file; no image has been uploaded to GitHub as part of this documentation-only pass.

## Additional product references considered

- Krust: compact inspector and native operational layout, but visually stuffed.
- OrbStack: native simplicity, but too reminiscent of Docker Desktop for Harpoon's desired identity.
- TablePlus: efficient table presentation, but feels dated to the design reviewers.
- HyperDX: strongest reference for purposeful charts, clean left navigation, and operational data presentation.

These are inspiration sources, **not dependency, feature, or implementation requirements**.

## Governing decisions

See [Container workspace specification](harpoon-2-container-workspace-spec.md). Existing [UI reference guidance](README.md) applies: code and supported engine capabilities are authoritative for behavior; reference art only informs visual language.
