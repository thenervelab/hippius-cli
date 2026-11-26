# hipc

`hipc` is the Hippius command line, for people and coding agents. It drives everything the console at https://console.hippius.com manages: accounts, projects, and teams, billing and plans, S3 buckets and tokens, the Hub registry, confidential VMs and App Engine apps, managed Kubernetes and PostgreSQL, GitHub Actions runners, CDN and WAF, the private network, and support. It does not move data: S3 objects go through an S3 client such as the AWS CLI or rclone, Drive files through the Drive apps, and Hub blobs through `hippius-hub`.

At a terminal it talks to a person. On a pipe it talks to a program: output is JSON, an error is one JSON object, and nothing prompts or pages. [TENETS.md](TENETS.md) is the contract.

## Install

macOS (Apple silicon and Intel) and Linux (x86_64 and aarch64).

If Homebrew has the IPFS-era `hipc` 0.0.x (`brew list --versions hipc` shows it), run `brew uninstall hipc` first, so the old one does not come first on `PATH`, or move it onto this one with Homebrew as below.

```sh
# into ~/.local/bin, no root, checks the release's checksum, and its signature when cosign
# is installed (HIPC_INSTALL_REQUIRE_SIGNATURE=1 makes that required)
curl -q --proto '=https' --tlsv1.2 -fsSL https://github.com/thenervelab/hippius-cli/releases/latest/download/install.sh | sh
```

Or with Homebrew:

```sh
brew trust thenervelab/tap
brew install thenervelab/tap/hipc
```

- Homebrew: version 6.0.0 and later load a formula from a third-party tap only once the tap or the formula is trusted. Installing by the full name trusts that formula, but a command that names it by its short name, such as `brew upgrade hipc`, is refused until then, so the `brew trust` line trusts the whole tap. Homebrew before 6.0.0 does not need it, and one before 5.1.15 answers it with `Unknown command: brew trust`, which is harmless: the install line still runs. Each release updates the formula, and `brew upgrade hipc` installs it. The tap's `hipc` was the IPFS-era tool of that name, version 0.0.x. On a machine that has it, `brew trust thenervelab/tap` and then `brew upgrade hipc` move it onto this one.
- PyPI: the wheels are published as the project `hippius` and install the command `hipc` (`pip install hippius`, `uv tool install hippius`, or `pipx install hippius`) once the release's `PYPI_PUBLISH` switch is on; until then each of those installs the old Hippius Python SDK 0.2.77, not this command. Afterwards, to keep the old SDK, pin `hippius==0.2.77` in its own environment.
- The PyPI project `hipc` is unrelated to Hippius.

Then check it:

```sh
hipc version
```

## Log in

Sign in at https://console.hippius.com with Google, GitHub, or Apple and create an API token in [Settings](https://console.hippius.com/dashboard/settings). If you sign in with a 12-word access key, get your token from the access-key sign-in described at https://docs.hippius.com/use/wallet-login. `hipc` signs in with that token only; it never asks for your 12 words. Then:

```sh
hipc auth login                     # paste the token at the prompt; it is not shown
hipc auth status
```

The token never goes on the command line. For a single run, set `HIPPIUS_TOKEN` instead.

## First commands

```sh
hipc account get
hipc billing balance
hipc s3 buckets list
hipc compute vms list --json
hipc s3 buckets create photos --dry-run
```

- `--dry-run` shows the requests and sends nothing.
- A command that spends money, deletes, revokes, rotates, or cancels asks first, and needs `--yes` outside a terminal.
- A command that prints a secret needs `--reveal`.
- `--help` works at every level, from `hipc --help` down to one command, and every page ends with examples.

## For agents

Point the agent at [docs/agents.md](docs/agents.md), about 2,000 tokens. From there it finds a command with `hipc --help` and `hipc schema s3 buckets`, previews a write with `--dry-run`, and asks the user before it adds `--yes`. Output is JSON on a pipe, and a failure is one JSON object on stderr with a `code` and a `hint`.

## Read more

- [docs/getting-started.md](docs/getting-started.md): install options, login, text and JSON, accounts and projects, profiles, completions.
- [docs/agents.md](docs/agents.md): how an agent finds and runs a command for its user.
- [docs/user-journeys.md](docs/user-journeys.md): twelve scenarios with their commands in order.
- [TENETS.md](TENETS.md): the contract, tenet by tenet.

`hipc completions bash` (or `zsh`, `fish`, `elvish`, `powershell`) prints a completion script.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), and [ARCHITECTURE.md](ARCHITECTURE.md) for how the code is laid out. Report a security problem privately, as [SECURITY.md](SECURITY.md) says. Changes are listed in [CHANGELOG.md](CHANGELOG.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or https://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or https://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
