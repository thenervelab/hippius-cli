# hipc

`hipc` is the Hippius command line, for people and coding agents. It drives everything the console at https://console.hippius.com manages: accounts, projects, and teams, billing and plans, S3 buckets and tokens, the Hub registry, confidential VMs and App Engine apps, managed Kubernetes and PostgreSQL, GitHub Actions runners, CDN and WAF, the private network, and support. It does not move data: S3 objects go through an S3 client such as the AWS CLI or rclone, Drive files through the Drive apps, and Hub blobs through `hippius-hub`.

At a terminal it talks to a person. On a pipe it talks to a program: output is JSON, an error is one JSON object, and nothing prompts or pages. [TENETS.md](TENETS.md) is the contract.

## Install

macOS (Apple silicon and Intel) and Linux (x86_64 and aarch64).

```sh
brew install thenervelab/tap/hipc
# or the install script: into ~/.local/bin, no root, checks the release's checksum, and its
# signature when cosign is installed (HIPC_INSTALL_REQUIRE_SIGNATURE=1 makes that required)
curl -q --proto '=https' --tlsv1.2 -fsSL https://github.com/thenervelab/hippius-cli/releases/latest/download/install.sh | sh
```

On PyPI the package is `hippius` and the command it installs is `hipc`, with wheels for macOS and Linux: `pip install hippius`, `uv tool install hippius`, or `pipx install hippius` installs the same `hipc` command. The PyPI project `hipc` is unrelated to Hippius. Then check it:

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
- Every `--help` ends with examples. [docs/commands.md](docs/commands.md) is every help page on one page.

## For agents

```sh
hipc schema                                 # every command, its effect, flags, guards, and error codes; JSON, no network
hipc schema compute app-engine              # only the commands under a path, same fields
hipc compute vms list --json                # a pipe, CI=1, or AI_AGENT=1 already means JSON
hipc s3 buckets create photos --dry-run     # the requests a real run would send; needs no token
hipc s3 buckets create photos --yes         # --yes confirms a write; CI and AI_AGENT never do
hipc s3 tokens create --kind master --reveal --yes   # --reveal allows an answer that holds a secret
hipc billing transactions --all             # every page as one array
# exit 0: done; 1: the API or the runtime failed; 2: change the invocation, nothing was sent
# a failure is one JSON object on stderr: {"error":{"code":...,"message":...,"hint":...,"retryable":...}}
```

[docs/agents.md](docs/agents.md) is the playbook, written to paste into an agent's context. `hipc api GET /accounts/` reaches any console endpoint with the same token, rules, and errors.

## Read more

- [docs/getting-started.md](docs/getting-started.md): install options, login, text and JSON, accounts and projects, profiles, completions.
- [docs/agents.md](docs/agents.md): the agent playbook.
- [docs/commands.md](docs/commands.md): every `--help` page, generated from the binary.
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
