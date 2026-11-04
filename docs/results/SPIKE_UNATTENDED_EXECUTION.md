# Gate U: unattended-execution feasibility

**Result: NOT PASSED. Unattended operation is unproven.**

By owner decision recorded with Milestone D, Gate U was **not investigated** and no experiment was run: no test of whether
a non-interactive process can read the Keychain (deploy credentials, secret versions) with the screen locked, after
sleep, after logout or before login; no test of what must already be running for a deployment to proceed; no background
agent exists or was built.

Consequences that hold until a separate, evidence-backed investigation passes:

* Every automatic behaviour is **attended only**: polling, webhook handling and the queue run inside a developer-started
  Harpoon process (`watch`), and the CLI, the documentation and the UI must say "automatic while Harpoon is running".
* A push made while no Harpoon is running is detected on the next run and reported as late; it is not deployed by anything
  else in the meantime.
* A deployment that cannot read a credential or a secret fails before any side effect (Milestones C and D); there is no
  plaintext fallback.
* No wording anywhere may claim unattended, background or always-on deployment. A background agent, GitHub/GitLab Apps and a
  relay for real provider deliveries remain deferred.

The questions the investigation must answer are unchanged and are listed under Gate U in
`docs/deployment-control-plane-requirements.md`.
