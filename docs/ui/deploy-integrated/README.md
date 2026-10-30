# Harpoon Deployments — integrated desktop UI (SIMULATED)

Status: **frontend integration of the approved prototype; mock-backed; no deployment engine.**
Reference: `docs/ui/deploy-prototype/index.html` and `docs/ui/deploy-ux.md` (PR #104).

Everything under Deployments is simulated. Nothing runs a deployment, writes a secret to the
Keychain, touches Docker, polls Git, or talks to a backend. Existing Harpoon screens are unchanged.

## Where things live
| Path | Role |
|---|---|
| `ui/harpoon-desktop/src/lib/deployTypes.ts` | **The interface boundary** for a future engine: types and the `DeployAction` vocabulary |
| `src/lib/deployStore.ts` | Pure reducer + selectors (the mock "service"); status is *derived*, never stored |
| `src/lib/deployMock.ts` | Seed data (all invented) |
| `src/lib/deployPresentation.ts` | Wording rules (recovery labels, evidence note, rollback warning, redaction) |
| `src/lib/deployPreview.ts` | Dev-only, whitelisted `#deploy=…` deep links for review |
| `src/hooks/useDeploySimulation.ts` | React wrapper (owns state, auto-advance timer, runtime override) |
| `src/views/deploy/*` | `DeployView`, `Screens`, `Dialogs`, `parts`, `deploy.css` (ported from the prototype) |
| `src/App.tsx` | +1 nav item (lazy view), runtime derivation, nav badge. No other view touched |

### Replacing the mock
A real engine implements the same read model and consumes the same `DeployAction`s; the reducer in
`deployStore.ts` is the only thing replaced. Views already take `Ctx` (state + `dispatch`), never
import the mock directly, and never call Tauri. The Rust side (`src-tauri`) is untouched; **no
command stubs exist**, so nothing implies functionality that is not there.

## What is real and what is simulated
- **Real:** Harpoon's runtime status (`status.state`, `dockerReady`). The Deployments views believe it
  unless a simulation override is set. A stopped Harpoon shows `UNKNOWN · NOT LIVE`, disables
  actions, hides resource figures, and shows no attention badge.
- **Simulated:** projects, applications, deployments, logs, metrics, containers, variables, timings,
  Keychain state, "Start Harpoon" in an *simulated* outage. Every hero carries a `SIMULATED` tag, the
  view carries a standing notice, and simulated container names are never links into the real
  Containers view.

## Deviations from the approved prototype
| Deviation | Why |
|---|---|
| Shell, rail, header and meters are the app's own (the prototype drew its own) | Native integration; header shows the real runtime badge |
| No icon-rail collapse of the global nav at compact widths | The shipping shell has no compact rail; changing it is out of scope. Deployments adapts to its own pane width (container queries) |
| Simulation bar is inside the view, labelled "not part of the product", with a **Next deployment** outcome control | Lets every state be reached by real actions, not just jumps |
| Phosphor icons instead of the prototype's hand-drawn strokes | App convention |
| No "Queued" step row; log lines are numbered, not timestamped | Six spec steps; mock has no clock |
| No Deployment chip in the real Containers view | There are no real managed containers to mark |
| Settings fields read-only; "Compare with Preview" disabled | Not part of the mock |
| `web` and `site` use the generic application screen | Prototype opened the same screen |
| `SIMULATED` tags and notices added | Required distinction from real runtime data |

Side-by-side crops (prototype left, integrated right): `comparison/`.

## Verification (run from `ui/harpoon-desktop`, Node 22)
- `npm test` — 82/82 (54 existing + 19 reducer/state-machine + 9 integration/source-level tests)
- `npx tsc --noEmit` and `npm run build` — pass
- `node docs/ui/deploy-integrated/click-through.mjs` — 19/19 headless-Chrome checks on the live UI
  (deploy plan, failed cutover → restored → paused, resume, rollback, failed restore, Keychain
  locked, simulated outage, existing navigation)
- `sh docs/ui/deploy-integrated/shoot.sh` — regenerates the 22 screenshots in `shots/`

## Unresolved limitations
- Screenshots and click-through ran in a browser with the Vite dev server, not inside the Tauri
  WebView; WKWebView behavior is **not verified**. There is no Tauri-run check in this change.
- No screen-reader pass; no usability testing (hypotheses H1–H8 in `deploy-ux.md` remain open).
- Compact behavior was checked at 1100/860/700 px window widths in Chrome only.
- Scenarios `building` and `recovering` hold still until stepped; user-started deployments run by
  themselves (~0.9 s per phase). Durations and metrics are illustrative.
- The global rail does not collapse; the existing header meters show `N/A` outside Tauri.
