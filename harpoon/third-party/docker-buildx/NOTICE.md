# Docker Buildx

Harpoon.app bundles the unmodified, upstream `docker-buildx` executable as
`Contents/Resources/harpoon/bin/docker-buildx`.

- Project: https://github.com/docker/buildx
- Version: v0.38.1 (pinned in `harpoon/fetch-buildx.sh`)
- License: Apache License 2.0 (`LICENSE` beside this file; copied into the app at
  `Contents/Resources/harpoon/share/licenses/docker-buildx/`)
- Copyright: The Docker Buildx authors (upstream publishes no separate NOTICE file)

Harpoon does not modify the binary. It is re-signed with Harpoon's own Developer ID for
distribution, which replaces Docker Inc.'s signature but not the code. The license text is
upstream's `LICENSE` at tag `v0.38.1`.
