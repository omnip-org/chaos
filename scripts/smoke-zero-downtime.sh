#!/bin/sh
set -u

endpoint="${1:-https://127.0.0.1:${HTTP_PORT:-443}/health/live}"
requests="${2:-100}"
failures=0
completed=0

while [ "$completed" -lt "$requests" ]; do
    if ! curl --fail --silent --show-error --insecure --output /dev/null "$endpoint"; then
        failures=$((failures + 1))
    fi
    completed=$((completed + 1))
    sleep 0.1
done

printf 'requests=%s failures=%s\n' "$requests" "$failures"
test "$failures" -eq 0
