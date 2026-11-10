# hippius

This package installs `hipc`, the command-line tool for the Hippius console: accounts,
billing, S3, Drive and Hub plans, CDN zones, and confidential compute. It is a single
binary with no Python code.

```sh
uvx --from hippius hipc --version  # run it once
uv tool install hippius            # or: pipx install hippius, pip install hippius
hipc --help
```

The package is `hippius` and the command it installs is `hipc`. The PyPI project `hipc` is
unrelated to Hippius.

Wheels exist for macOS (Apple silicon and Intel) and Linux (x86_64 and aarch64).
