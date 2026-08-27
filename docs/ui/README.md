# Harpoon UI Reference

These images are visual design references generated during the Harpoon
UI/UX design pass.

They are NOT specifications for backend behavior, data models, paths,
commands, metrics, or capabilities.

## Authority

The existing Harpoon implementation remains the source of truth for:
- functionality
- runtime behavior
- Docker/Harpoon APIs
- filesystem paths
- configuration
- available actions
- displayed data

The reference images are authoritative only for:
- visual language
- layout direction
- spacing and density
- typography hierarchy
- borders and panel treatment
- navigation treatment
- status presentation
- interaction styling

Do not implement functionality merely because it appears in a reference
image.

Do not copy example values, filesystem paths, configuration keys, or
capabilities from the images without verifying them against the existing
Harpoon implementation.

## References

01-overview.png
02-containers.png
03-images.png
04-volumes.png
05-networks.png
06-resources.png
07-diagnostics.png

## Known exceptions

Some generated references contain artifacts or hallucinated details.

- Resources: CPU Allocation and Memory Limit headings have incorrect
  low-contrast text. Do not reproduce this.
- Resources: generated configuration paths/fields may not represent
  Harpoon and must be verified against source.
- Diagnostics: generated binary paths must be verified against source.
- Diagnostics: shell/header differs slightly from the canonical shell.
  Follow the shared Harpoon shell rather than reproducing the discrepancy.
