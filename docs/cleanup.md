# Clean Up

One operation removes what Harpoon can prove it no longer needs, and keeps everything it must.

```
harpoon-deploy cleanup --runtime-dir DIR --docker-socket PATH --resources-dir DIR --mgmt-socket PATH            # review (changes nothing)
harpoon-deploy cleanup … --keep-rollbacks N --apply --authorize-live-docker-changes=…                           # carry it out
```

The desktop offers the same as **Clean up** on the Resources page: *Review clean up*, then *Clean up now*. The older single-purpose
controls (old releases, build cache, one image) remain under "More control".

## What is removed
- Containers and images of releases that no longer serve (historical rollback releases, by default none are kept).
- Leftovers of failed attempts, and of **settled** interrupted deployments: the restoration finished and passed its health check,
  and a decision cleared the pause. The failed candidate's own image goes even when its restoration container is what serves.
- Unused build cache: records Docker reports as not in use, not shared with an image, and of the plain type.
- Journal records for objects Docker no longer has (reconciled, nothing removed).

Every object is re-verified immediately before removal (exact id, all three Harpoon labels, one tag, no container using it), the
attempt is journaled before Docker is asked and the result after, and an interrupted run is finished or reconciled by the next one.

## What is always kept
- Whatever serves now: the release, its container and its image, including a restoration container that is serving.
- **Anything a deployment may still need to recover.** A deployment whose cutover began and that is not settled (its restoration is
  unfinished, unverified or failed, or no decision cleared its pause) keeps the release it replaced. Retention below never governs this.
- Volumes and persistent data, configuration, secrets and journals.
- Anything Harpoon did not create, and Harpoon-labelled objects its journal does not know (they are reported, never removed).
- Everything, if any deployment is unfinished: Clean Up refuses to run then.

## The one setting
`--keep-rollbacks N` (default **0**, desktop: "Keep previous releases for rollback") keeps the N most recent previous releases that
are *proven* restorable (journal eligibility plus Docker holding exactly that image id with the deployment's labels). What cannot
be proven is kept, not guessed.

## Space: expected and measured
Removing an image removes a reference; space is freed only for layers no image that stays keeps. The review prints
`cleanup-estimated-bytes` (layers no remaining image keeps, plus unused unshared cache); shared layers are never counted. After an
apply, `measured-reclaimed-bytes` is the change in the guest's free space, with a `reclaim-check` comparing it to the estimate. An
estimate that cannot be shown is "not estimated" or a lower bound, never zero by assumption. Running Clean Up again finds nothing.
