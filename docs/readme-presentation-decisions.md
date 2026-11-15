# Harpoon README: presentation decisions

**Decision date:** 2026-11-15. **Scope:** the public-facing README, screenshots, onboarding, and installer-facing cross-links for the published **v2.0.0-rc.4 testing prerelease**. This is a communications decision, not a change to runtime behavior or an acceptance-gate waiver.

## Who the first screen must serve

A person arriving from GitHub should be able to answer, without reading an engineering milestone log: **What does Harpoon do? Will it run on my Mac? Which version should I download? Is it safe to try alongside existing Docker tooling? How do I get one container or app running? What data will it preserve? What is untested?** The README answers those first and links to deeper evidence.

## Adopt

| Direction | Decision and rationale |
|---|---|
| Lead with source to app | **Adopt.** The desired differentiator is moving a repository into a running **local** application. Docker compatibility and the VM underpin that experience; they still appear in the first paragraph. |
| Tagline: “From repository to running app. All on your Mac.” | **Adopt.** Clearer to a new visitor than “Containers, under control.” The latter remains part of the project's identity/history but is not the primary hero statement. |
| Audience and capabilities | **Adopt, with constraints.** Speak to developers who want Docker-compatible workloads and a low-friction local deployment workflow. Never imply Vercel's hosting, remote environments or production orchestration parity. |
| Stable and testing separation | **Adopt.** Put rc.4 download and its warnings above the first screenshot. v0.1.1 stays labeled latest stable. Direct asset link and verified checksum are more useful than a release badge that can point to the stable release instead. |
| Installation before source compilation | **Adopt.** The README explains the DMG path first; source build steps live in docs/building.md. Separate requirements for running the app from requirements for contributing. |
| Plain-language feature/limitations sections | **Adopt.** Separate containers/Compose from attended deployments; explain missing features plainly. Remove historical 'v0.1 scope' headings from the user-facing README. |
| Screenshot captions and provenance | **Adopt.** The gallery states these came from a development build, not accepted screenshots of the shipping desktop. Do not imply disabled options are available. |
| Data safety / troubleshooting | **Adopt.** Name both support directories, Keychain, runtime disk and Docker context; explain intentional operations, backup needs, and unresolved startup faults. |

## Modify rather than adopt literally

| Suggestion | Decision and rationale |
|---|---|
| Performance headline | **Do not use as a marketing claim.** Prior measurements have different methods, Macs and workload mixes; they belong in methodology and evidence documents, not an unqualified Docker Desktop comparison. Revisit only with controlled repeatable same-host benchmarks. |
| “Everything is current” screenshots | **Keep real captures but explicitly label source and testing boundary.** Recapture from the shipping DMG after desktop install/first-launch acceptance; until then, do not present development captures as proof of shipped UI acceptance. |
| Empty/zero cleanup counts and unavailable cards | **Do not manufacture better-looking state.** Explain empty or unavailable views truthfully, and recapture with representative real disposable state later. |
| Teaser media | **Keep one link to the existing rc.1 teaser**, clearly described as an earlier preview. Do not imply that video captures rc.4 behavior. An autoplaying GIF is not necessary for first-run clarity. |
| Landing-page density | **Shorten the README** and link architecture, technical measurements, acceptance evidence and roadmap rather than burying the install path under implementation history. |

## Defer or reject

| Direction | Decision and re-evaluation condition |
|---|---|
| Cloud deployment, auto deploy-on-push, Kubernetes or feature parity with Vercel | **Reject as present-day product claims.** Harpoon deploys to one Mac with attended decisions. Future provider integration is not a promise and requires a separate product decision. |
| Intel as a supported platform | **Defer.** Universal slices exist, but boot of the Intel guest is unverified. Change the claim only after measured Intel hardware acceptance. |
| Rewrite text inside screenshot images | **Defer.** Do not alter genuine captures to make disabled or absent functionality look real. Improve the product UI first, then capture it accurately. |
| Treat release CI as desktop install acceptance | **Reject.** Runtime/CI smoke tests, notarization and signed-bundle validation are not the same as first-run desktop acceptance. Explicitly report the gap until tested. |
| Rewrite all historical results to current tense | **Reject.** Historical evidence is immutable context. Update only misleading current-status claims and add a dated notice where a document is intentionally a historical snapshot. |

## Remaining work that documentation cannot accomplish

- Install and launch the shipped rc.4 desktop on a clean Mac; capture the first-run, Docker-context and source-deployment flow using disposable state.
- Re-test Intel guest boot on Intel hardware before calling it supported, and complete the separately documented reliability and upgrade/rollback gates.
- **Completed 2026-11-15:** GitHub repository description changed from “Cause damn the whales” to **“Native macOS Docker-compatible runtime with local source-to-app deployments.”** Topics were set to `macos`, `docker`, `containers`, `virtualization`, `developer-tools`, `tauri`. This is repository metadata, not a README code change.
- Re-check screenshots and caption text after product/UI changes; change presentation evidence, not the provenance of old results.

Public-facing truth source: [rc.4 release notes](releases/v2.0.0-rc.4.md), [installation](installation.md), [compatibility](compatibility.md), and [security policy](../SECURITY.md).
