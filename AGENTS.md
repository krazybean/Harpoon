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

Worker-sandbox QMD repair/fallback:

- QMD 2.8.3 opens every database with `PRAGMA journal_mode = WAL`; a read-only index is not a supported read-only mode. `SQLITE_CANTOPEN` during WAL setup with user-owned, traversable paths means the worker cannot create SQLite sidecars; it does not by itself indicate a corrupt index.
- Do not point workers at the global QMD index. When the canonical `.qmd/index.sqlite` is inaccessible, copy the project-local config and index into a directory that the worker is allowed to write, then run QMD from that staging directory so the repo-local config is not rediscovered:

  ```sh
  HARPOON_ROOT="/Users/juancastro/Documents/Github/Harpoon"
  QMD_WORK="/path/to/worker-writable/qmd-harpoon"
  mkdir -p "$QMD_WORK/config"
  cp "$HARPOON_ROOT/.qmd/index.yml" "$QMD_WORK/config/index.yml"
  cp "$HARPOON_ROOT/.qmd/index.sqlite" "$QMD_WORK/index.sqlite"
  (cd "$QMD_WORK" && \
    QMD_CONFIG_DIR="$QMD_WORK/config" \
    INDEX_PATH="$QMD_WORK/index.sqlite" \
    qmd search "historical Harpoon decision" -n 10 --no-rerank)
  ```

- Retrieve results with the same `QMD_CONFIG_DIR` and `INDEX_PATH`, using `qmd get` after discovery. The staging directory must be worker-writable; `/tmp` is not guaranteed to be writable under every worker sandbox.
- If the indexed sources changed, refresh only the staged copy with `qmd update` from that same staging directory. If the source index is absent or unusable, omit the `cp` of `index.sqlite` and let `qmd update` initialize the staged database. Never delete or recreate the canonical index as a repair step.
- If no worker-writable directory can create SQLite WAL/SHM files, classify QMD as sandbox-blocked, report the exact error and paths, and do not silently fall back to another index or invent project history.
