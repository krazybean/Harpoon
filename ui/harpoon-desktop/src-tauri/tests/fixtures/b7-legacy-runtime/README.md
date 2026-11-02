# B7 legacy runtime fixture

The journal of the successful B7 live-acceptance session of 2026-11-02 (deployment
`dep_01M6DPQ543ASWBHW7PW17ZAJ3C`, application `app_01M6DPQ543ASWBHW7PW17ZAJ3B`), copied byte for byte from the preserved
evidence (`~/harpoon-b7-evidence-PRESERVED-20261102/runtime`). It was written by the experimental B7 Phase 2B build
(revision `236ccec`), which predates the SQLite journal, so it is the legacy `journal.wal`.

Nothing was removed or edited: it holds only ids, timestamps and statuses (no paths, host names or secrets; checked), and
editing a record would invalidate its checksum. Tests copy this folder to a temporary directory; never write to it.
