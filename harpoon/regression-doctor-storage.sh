#!/bin/sh
set -eu

# Offline policy check: df capacity may be below raw ext4 geometry by <=2%.
classify() {
  logical=$1
  capacity=$2
  if [ "$logical" -gt "$capacity" ] && [ $((logical - capacity)) -gt $((logical / 50)) ]; then
    printf '%s\n' warning
  else
    printf '%s\n' consistent
  fi
}

[ "$(classify 34359738368 33741029376)" = consistent ]
[ "$(classify 34359738368 30000000000)" = warning ]
[ "$(classify 34359738368 33672543601)" = consistent ]
printf '%s\n' 'doctor storage regression: PASS'
