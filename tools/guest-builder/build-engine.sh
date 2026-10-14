#!/bin/bash

# Resolve one Linux container engine for guest artifact construction.
# The engine is build-time only; Harpoon's own context is never selected implicitly.
harpoon_detect_build_engine() {
  local requested="${HARPOON_BUILD_DOCKER_CONTEXT:-}"
  local caller_context="${DOCKER_CONTEXT:-}"

  HARPOON_BUILD_ENGINE_KIND=""
  HARPOON_BUILD_ENGINE_DESCRIPTION=""

  if command -v docker >/dev/null 2>&1; then
    if [ -n "$requested" ]; then
      docker --context "$requested" info >/dev/null 2>&1 && {
        HARPOON_BUILD_ENGINE_KIND=docker-context
        HARPOON_BUILD_ENGINE_DESCRIPTION="Docker context $requested"
        return 0
      }
    elif [ -n "${DOCKER_HOST:-}" ]; then
      docker info >/dev/null 2>&1 && {
        HARPOON_BUILD_ENGINE_KIND=docker
        HARPOON_BUILD_ENGINE_DESCRIPTION="Docker DOCKER_HOST=$DOCKER_HOST"
        return 0
      }
    elif [ -n "$caller_context" ]; then
      docker --context "$caller_context" info >/dev/null 2>&1 && {
        HARPOON_BUILD_ENGINE_KIND=docker-context
        HARPOON_BUILD_ENGINE_DESCRIPTION="Docker context $caller_context"
        return 0
      }
    elif docker info >/dev/null 2>&1; then
      HARPOON_BUILD_ENGINE_KIND=docker
      HARPOON_BUILD_ENGINE_DESCRIPTION="Docker default endpoint"
      return 0
    fi
  fi

  if command -v podman >/dev/null 2>&1 && podman info >/dev/null 2>&1; then
    HARPOON_BUILD_ENGINE_KIND=podman
    HARPOON_BUILD_ENGINE_DESCRIPTION="Podman default endpoint"
    return 0
  fi

  return 1
}

harpoon_build_engine() {
  case "${HARPOON_BUILD_ENGINE_KIND:-}" in
    docker) docker "$@" ;;
    docker-context) docker --context "${HARPOON_BUILD_DOCKER_CONTEXT:-${DOCKER_CONTEXT:-}}" "$@" ;;
    podman) podman "$@" ;;
    *) echo "[guest-builder] FAIL: build engine was not detected" >&2; return 1 ;;
  esac
}
