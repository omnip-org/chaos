# CLAUDE.md

This file is read by Claude Code. It mirrors `AGENTS.md`, this repository's
authoritative agent-instructions file — keep the two in sync; if they ever
diverge, `AGENTS.md` wins.

- Communicate with the user in their preferred language, but write all repository artifacts in English.
- Use English for source comments, documentation, ADRs, API descriptions, logs, examples, commit messages, pull request text, and review comments.
- Use English Conventional Commits: `type(scope): concise imperative subject` (see `CONTRIBUTING.md` for examples). This applies to every commit, including large multi-file changes — do not fall back to a plain imperative subject without a `type(scope):` prefix.
- Preserve the DDD dependency direction documented in `docs/adr/0001-ddd-workspace-boundaries.md`.
- Follow `docs/database-conventions.md` for every schema, migration, query, constraint, and index.
- Run formatting, Clippy, tests, and `scripts/check-language.sh` before completing a change.
