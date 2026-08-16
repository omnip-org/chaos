#!/bin/sh
set -eu

cjk_pattern='[\x{4e00}-\x{9fff}]'

if rg --line-number "$cjk_pattern" \
    AGENTS.md CONTRIBUTING.md README.md Dockerfile .env.example \
    docker-compose.yaml docs crates migrations packages scripts .githooks \
    --glob '*.md' \
    --glob '*.rs' \
    --glob '*.sql' \
    --glob '*.sh' \
    --glob '*.js' \
    --glob '*.json' \
    --glob '*.yaml' \
    --glob 'Dockerfile' \
    --glob '.env.example'; then
    printf 'Non-English CJK text found in repository artifacts.\n' >&2
    exit 1
fi

printf 'Language convention check passed.\n'
