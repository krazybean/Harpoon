# http-marker — the disposable B7 test application

`Dockerfile` + `srv.sh`: an HTTP responder on port 8080 for busybox `nc -lk -e` in `alpine:3.22` (the
base image Harpoon's own gates use; the CLI's `doctor` confirms it is present locally, and Harpoon never
pulls). The only input is `HARPOON_DEPLOYMENT_MARKER`, which Harpoon sets to a fresh random value for
each container it creates.

* `GET /health` → `200` with `X-Harpoon-Deployment-Marker: <marker>` (also in the body).
* any other path → `404`, never the marker. No marker configured → `503`, never the marker.
* No volumes, no `USER`/privilege changes, no packages, no network use at build time, no restart policy
  (that is a container setting, and Harpoon sets none).

Tested offline in `deploy-docker/tests/b7_fixture.rs`: the script is run by `/bin/sh` behind a loopback
TCP listener owned by the test and probed with Harpoon's own HTTP client; the Dockerfile is checked with
Harpoon's own preflight. **It has not been built or run in Docker.**
