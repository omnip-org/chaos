#!/bin/sh
set -u

endpoint="${1:-http://127.0.0.1:8080/health/live}"
requests="${2:-100}"
failures=0
completed=0

while [ "$completed" -lt "$requests" ]; do
    if ! curl --fail --silent --show-error --output /dev/null "$endpoint"; then
        failures=$((failures + 1))
    fi
    completed=$((completed + 1))
    sleep 0.1
done

printf 'requests=%s failures=%s\n' "$requests" "$failures"
test "$failures" -eq 0
