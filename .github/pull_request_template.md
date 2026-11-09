## What

## Why

## How it was tested

Gate results with their counts, and which check you broke once to see its test fail.

## Checklist

- [ ] Every behavior changed or added has a test, its edge cases and errors included.
- [ ] A user-visible change has a line under `## [Unreleased]` in `CHANGELOG.md`, in the right subsection.
- [ ] Help text follows tenet 16: one sentence in sentence case for the short help, no final period, no Markdown backticks, examples at the end of every page.
- [ ] `docs/commands.md` is regenerated when any help text changed: `scripts/docs-reference.sh target/debug/hipc > docs/commands.md`.
- [ ] A change to a command's surface (a command, a flag, an effect, a confirmation, a guard, an error code) updates `README.md`, `docs/getting-started.md`, or `docs/agents.md` where they show it, and the `llms.txt` and CLI pages of docs.hippius.com when an agent's first steps change.
- [ ] A new command has a row in `scripts/release-check.sh`.
- [ ] A changed script passes `shellcheck` and `shfmt -d -i 2`; a changed workflow passes `actionlint` and `zizmor`.
- [ ] No private repository name, internal file or symbol name, person's name, or token anywhere in the diff.
