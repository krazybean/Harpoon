# Application recovery after a Harpoon restart

What happens to a Harpoon-managed application when Harpoon, or its virtual machine, stops and starts again.

## The model

- Harpoon creates every container with Docker restart policy **`no`**. A candidate (the preliminary check container and the
  new release before its final check) can therefore never come back by itself, including after a failed or interrupted
  deployment.
- Once a release is **confirmed serving** (the final health check passed and the success is durable in the journal), Harpoon
  sets that one container to **`unless-stopped`** through Docker's update API. It is the last step of a deployment, after
  the journal write. If it fails, the deployment is still a success and carries a warning; `restart-policy --apply` retries it.
- After the guest restarts, Docker brings back the `unless-stopped` containers that were running. Harpoon starts nothing,
  rebuilds nothing and redeploys nothing.

## Why `unless-stopped`, not `always`

Measured on an isolated Harpoon VM (graceful stop and start, and a hard kill of the engine):

| Container | Policy | Before the restart | After |
|---|---|---|---|
| serving release | `unless-stopped` | running | running again |
| unconfirmed candidate | `no` | running | stays stopped |
| release stopped on purpose (a replaced release, or `docker stop`) | `unless-stopped` | stopped | stays stopped |
| release stopped on purpose | `always` | stopped | **started again** |

`always` would revive every replaced or deliberately stopped release, over the release that should be serving.
`unless-stopped` honours the stop Harpoon itself performs when it replaces a release, so no new "desired state" record is
needed: Docker's own stopped flag is the record.

## Releases that predate this change

Containers created before this change have policy `no` and do not come back after a restart. `restart-policy` brings
them in line without redeploying anything:

```
harpoon-deploy restart-policy --runtime-dir DIR --docker-socket PATH --resources-dir DIR            # dry run, changes nothing
harpoon-deploy restart-policy --runtime-dir DIR --docker-socket PATH --resources-dir DIR --apply --authorize-live-docker-changes=…
```

For each application it takes the release the journal records as serving and that container's recorded id, re-checks the id,
name and Harpoon labels in Docker, and only then sets `unless-stopped`. It never starts, stops, recreates or adopts a container;
a container that is stopped now is reported and left stopped; a failed or superseded release, an unfinished deployment, a
container the journal does not vouch for, and another application's container are not touched. It writes nothing to the journal.

## What a restart policy does not give you

A running container is not a healthy application. Docker restarts the container; it does not run Harpoon's health check or wait
for the application to answer. After a restart, check the application (the Overview health and the deployment's final check
describe what "healthy" means for it). A container that exits repeatedly will be restarted by Docker with backoff until
stopped; stop it with `docker stop` to keep it down across restarts.

## Where the state lives

- **Deployment journal:** `~/Library/Application Support/com.harpoon.desktop/deploy/deploy-journal/deployments.sqlite3`, on the
  host, next to `runtime-kind`. It is not under `~/Library/Application Support/Harpoon/` (that holds the engine's log, runtime
  files and the root disk). A stopped or restarted engine does not touch it. Each isolated test namespace has its own.
- **Containers, images and volumes:** in the guest's persistent root disk. A restart keeps them.
