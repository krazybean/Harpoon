# Deployment baseline harness

Disposable, repeatable measurements for the Harpoon 2.1 deployment control plane
(see `docs/results/DEPLOY_BASELINE.md`). It targets an **already running** Harpoon
runtime, never starts/stops/reconfigures the VM, never prunes, and removes only Docker
objects labelled `harpoon.baseline=1`.

```sh
python3 bench.py env | build --n 5 | transport --n 30 | concurrent | publish --n 30 | idle --n 20 | cleanup
```

Environment overrides: `HARPOON_BIN` (default: the repo's `harpoon/build/harpoon`),
`DOCKER_BIN`. Fixtures are staged under `/private/tmp/harpoon-baseline-fx` (visible to the
guest as `/mnt/harpoon-host/tmp/...`) and deleted afterwards. Output goes to
`docs/results/DEPLOY_BASELINE/raw/`.

Known limits: synthetic fixtures; the 1 Hz sampler adds `harpoon exec` overhead; the
guest Docker CLI has no buildx, so there is no in-guest BuildKit comparison; the harness
deliberately does not push the VM toward its memory limit.
