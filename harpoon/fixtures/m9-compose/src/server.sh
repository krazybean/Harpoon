#!/bin/sh
set -eu
echo "m9 app listening 3000 env=$APP_ENV secret=$SECRET_FROM_ENV extra=$EXTRA_ENV"
# generate index for bind test
echo "m9 ok env=$APP_ENV secret=$SECRET_FROM_ENV extra=$EXTRA_ENV ts=$(date +%s)" > /app/src/index.html
while true; do
  # serve one request: read request line, then respond
  # busybox nc: nc -l -p 3000 -w 1
  # use printf for HTTP response
  body="m9 ok env=$APP_ENV secret=$SECRET_FROM_ENV extra=$EXTRA_ENV ts=$(date +%s)"
  # simple: handle health as json
  # we need to read request to determine path, but for now same body for all
  printf "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: %s\r\n\r\n%s" "${#body}" "$body" | nc -l -p 3000 -w 2 2>/dev/null || true
  sleep 0.05
done
