# hippius-cli

This package installs `hipc`, the command-line tool for the Hippius console: accounts,
billing, S3, Drive and Hub plans, CDN zones, and confidential compute. It is a single
binary with no Python code.

```sh
uvx --from hippius-cli hipc --version  # run it once
uv tool install hippius-cli            # or: pipx install hippius-cli, pip install hippius-cli
hipc --help
```

The package is `hippius-cli` and the command it installs is `hipc`. The `hippius` package on
PyPI is the Hippius Python SDK, a separate project that this package does not replace.

Wheels exist for macOS (Apple silicon and Intel) and Linux (x86_64 and aarch64).
