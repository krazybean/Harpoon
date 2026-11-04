#!/bin/sh
# One HTTP request on stdin, one response on stdout (busybox `nc -lk -e` runs this per connection).
#
#   GET /health  -> 200, the deployment marker in the X-Harpoon-Deployment-Marker header and body
#   anything else -> 404 (and never the marker)
#
# If HARPOON_DEPLOYMENT_MARKER is empty or unset this answers 503 without any marker, so a
# misconfigured container can never look healthy.
CR="$(printf '\r')"
read -r method path _version || exit 0
# drain the request headers
while read -r header; do
  [ -z "$header" ] && break
  [ "$header" = "$CR" ] && break
done
marker="${HARPOON_DEPLOYMENT_MARKER:-}"
case "$marker" in
  *[!0-9a-f]*|"") marker="" ;;
esac
path="${path%%\?*}"
if [ -z "$marker" ]; then
  body="no deployment marker configured"
  printf 'HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: %s\r\nConnection: close\r\n\r\n%s\n' "$((${#body} + 1))" "$body"
elif [ "$method" = "GET" ] && [ "$path" = "/health" ]; then
  printf 'HTTP/1.1 200 OK\r\nX-Harpoon-Deployment-Marker: %s\r\nContent-Type: text/plain\r\nContent-Length: %s\r\nConnection: close\r\n\r\n%s\n' "$marker" "$((${#marker} + 1))" "$marker"
else
  printf 'HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 10\r\nConnection: close\r\n\r\nnot found\n'
fi
