# Contributing

## Language convention

Repository artifacts must use English. This includes:

- source-code comments and documentation comments;
- Markdown documentation and architecture decision records;
- API descriptions, examples, error messages, logs, and operational scripts;
- commit subjects and bodies;
- pull request titles, descriptions, and review comments.

Conversation outside repository artifacts may use any language. User-facing localization resources are exempt when they are intentionally introduced as product data.

Run the language check before committing:

```bash
./scripts/check-language.sh
```


Enable the repository-provided commit-message check once per clone:

```bash
git config core.hooksPath .githooks
```

The hook rejects commit messages containing CJK text. The convention still relies on review for other non-English languages.

## Commit convention

Use English [Conventional Commits](https://www.conventionalcommits.org/) with a concise imperative subject.

Examples:

```text
feat(merchant): add merchant account transaction context
fix(api): preserve requests during instance draining
docs(architecture): document blue-green deployment
```

## Required checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
npm test --prefix packages/js
./scripts/check-language.sh
```
