# Harpoon historical knowledge retrieval

For previous architectural decisions, implementation rationale, historical constraints, design changes, or earlier investigations, consult Harpoon's project-local QMD index before making assumptions.

- Use `qmd search` to discover relevant documents, then `qmd get` to retrieve the original source with line references.
- Cite the original source when reporting findings.
- Distinguish historical decisions, proposals, current implementation, and superseded documentation.
- Current source code and authoritative documentation take precedence over outdated historical descriptions.
- Always use Harpoon's local `.qmd/index.sqlite`; never fall back to the global QMD index.
- Do not invoke QMD for routine work, and never invent project history when retrieval fails.

QMD execution rules:

- Resolve the Harpoon repository root before invoking QMD, then run retrieval from that root so the local `.qmd/index.sqlite` is selected.
- Use `qmd search` for discovery and `qmd get` for the original source with line references.
- A read-only investigation must not modify tracked project files or authoritative documentation. QMD may create SQLite WAL/SHM housekeeping files inside the ignored `.qmd/` directory; that housekeeping is not a source change.
- Respect the execution sandbox and filesystem permission boundaries. If SQLite cannot open the local index because of sandbox restrictions, report that restriction explicitly; do not bypass it or request broader permissions without user approval.
- Do not generate embeddings, invoke local models, or run `qmd update` unless indexed documents changed.
