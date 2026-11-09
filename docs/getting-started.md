# Getting started

From nothing to a logged-in `hipc`. The [README](../README.md) is the short version; [agents.md](agents.md) is the version for programs.

## Install

macOS (Apple silicon and Intel) and Linux (x86_64 and aarch64):

```sh
brew install thenervelab/tap/hipc
# or
curl -q --proto '=https' --tlsv1.2 -fsSL https://github.com/thenervelab/hippius-cli/releases/latest/download/install.sh | sh
```

- The Homebrew formula replaces the old IPFS-era `hipc`, so `brew upgrade` moves a machine onto this one.
- The script installs to `~/.local/bin` (or `$HIPC_INSTALL_DIR`) with no root, checks `SHA256SUMS`, and checks the signature when `cosign` is on `PATH` (`HIPC_INSTALL_REQUIRE_SIGNATURE=1` makes that required). End the pipe with `sh -s -- 0.15.0` to pin a version.
- PyPI follows right after the first release: `pip install hippius-cli` will install the same `hipc` binary (the `hippius` package on PyPI is the Python SDK, a different project).
- From source: `cargo install --locked --git https://github.com/thenervelab/hippius-cli hippius-cli`.
- To uninstall, remove the binary; remove `~/.hippius` too to forget the token.

Then run `hipc version`.

### An older `hip` binary

If you tested a pre-release build, the command was `hip`. Remove `~/.local/bin/hip` and its completion files (`~/.local/share/bash-completion/completions/hip`, `~/.zfunc/_hip`, `~/.config/fish/completions/hip.fish`), then write new ones as [Shell completions](#shell-completions) shows. The config in `~/.hippius` and the `HIPPIUS_*` variables carry over.

## Verifying a download

Each release has a tarball per target, `hipc-<target>.tar.gz`, and a `SHA256SUMS` signed by the release workflow. The macOS binaries are not notarized: after a browser download, run `xattr -d com.apple.quarantine ./hipc`. Check a download with [cosign](https://docs.sigstore.dev/cosign/system_config/installation/) 3.0 or later (each release's notes print these lines filled in):

```sh
version=0.15.0                         # the release you downloaded, without the v
file=hipc-aarch64-apple-darwin.tar.gz  # the tarball you downloaded, or install.sh
cosign verify-blob SHA256SUMS --bundle SHA256SUMS.sigstore.json \
  --certificate-identity "https://github.com/thenervelab/hippius-cli/.github/workflows/release.yml@refs/tags/v$version" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
awk -v f="$file" '$2 == f' SHA256SUMS | shasum -a 256 -c -
```

Expect `Verified OK` and `: OK` (`sha256sum -c -` works on Linux too). Anything else: do not use the file.

## Log in

Sign in at https://console.hippius.com with Google, GitHub, or Apple and copy your API token from [Settings](https://console.hippius.com/dashboard/settings). If you sign in with a 12-word access key, Settings shows no token: run `hipc auth login --access-key` instead, and type the 12 words at its prompt, which does not show them; `hipc` stores only the token the sign-in answers. Then:

```sh
hipc auth login                     # paste the token at the prompt; it is not shown
hipc auth status
```

- The token comes from the prompt, stdin, or a file, never the command line. The prompt appears only at a terminal; a script or an agent passes `--token-file <path>`, or `--token-file -` for stdin. On macOS, `pbpaste | hipc auth login --token-file -` works too.
- It is stored in `~/.hippius/config.toml` (mode `0600`) and sent only to the API it was stored for. `hipc auth status` shows its last four characters, never the token.
- For one run or a pipeline, set `HIPPIUS_TOKEN` instead; it is stored nowhere, and it is sent only to `https://api.hippius.com` unless `HIPPIUS_TOKEN_ORIGIN` names another origin.
- Words the API has not seen open a new, empty account, and so does one mistyped word. So when the sign-in says the account is new, `hipc auth login --access-key` stores nothing and exits 2: check the words and run it again. By then the server has already made the empty account, and a login with the same words now finds it and logs in. To make a new account on purpose, add `--new-account`.
- For CI with an access-key account, sign in once on a machine you trust and store only the token as the runner's `HIPPIUS_TOKEN` secret. Never put the 12 words on a runner: they control the whole account.

## First commands

```sh
hipc account get
hipc billing balance
hipc s3 buckets list
hipc status
hipc s3 buckets create photos --dry-run
hipc s3 buckets create photos --yes
```

- Reads change nothing. `--dry-run` shows the requests and sends nothing.
- A command that spends money, deletes, revokes, rotates, or cancels asks first, and needs `--yes` outside a terminal. So do opening or closing a support ticket, `hipc network get` (the first call creates the private network), `hipc hub namespace create`, and `hipc auth logout`.
- A command that prints a secret needs `--reveal`, and prints it once.
- Every `--help` ends with examples. [commands.md](commands.md) has every command, App Engine and the private network included, and [user-journeys.md](user-journeys.md) has twelve scenarios in order.

## Text and JSON

At a terminal `hipc` prints text; on a pipe, or with `CI` or `AI_AGENT` set, JSON. `--json` and `--output text` force one. A list is one page, `{"items":[...],"next_page_token":null}`; `--limit` sets its size, `--page-token` fetches the next, and `--all` fetches every page:

```sh
hipc compute vms list --json | jq -r '.items[].vm_id'
hipc billing transactions --all
```

A failure is an `error:` line on stderr, with a `hint:` when `hipc` knows the fix, or one JSON object in JSON mode. Exit `1` means the API or the runtime failed; exit `2` means change the command, and nothing was sent.

## Accounts, projects, and billing accounts

One token can act in your own account and in any shared account you were added to. Pick one with `--account` for a command, or `hipc account use` for every later one; `self` goes back:

```sh
hipc account list
hipc account use acct_0123456789abcdef0123
hipc account use self
```

Projects and billing accounts work the same way, with `--project`, `--billing`, `hipc account projects use`, `hipc account billing-accounts use`, and `default` to go back. Switching accounts clears both. A command that cannot take one refuses it rather than act in the wrong place.

```sh
hipc account projects create --name web
hipc account projects use proj_0a1b2c3d4e5f60718293
hipc billing balance --billing bill_9f8e7d6c5b4a39281706
```

Creating or renaming a project or billing account is free; archiving, budgets, limits, and transfers need `--yes`.

## Profiles and the config file

The config is `~/.hippius/config.toml`, or `HIPPIUS_CONFIG`. `hipc` refuses it when others can read it, and the error names the `chmod` that fixes it. `--profile work` (or `HIPPIUS_PROFILE=work`) keeps a second token. Each setting comes from the flag, then the `HIPPIUS_*` variable, then the profile.

## Shell completions

```sh
# bash, with the bash-completion package
mkdir -p ~/.local/share/bash-completion/completions
hipc completions bash > ~/.local/share/bash-completion/completions/hipc

# zsh: put fpath=(~/.zfunc $fpath) before compinit in ~/.zshrc
mkdir -p ~/.zfunc
hipc completions zsh > ~/.zfunc/_hipc

# fish
mkdir -p ~/.config/fish/completions
hipc completions fish > ~/.config/fish/completions/hipc.fish
```

`elvish` and `powershell` work the same way: print the script with `hipc completions elvish` or `hipc completions powershell` and load it from your shell's profile.

## Moving data

`hipc` is the control plane and copies no bytes. S3 objects go through the AWS CLI, boto3, or rclone, with the profile `hipc s3 configure` writes. Drive files go through the Drive apps, and Hub blobs through `hippius-hub`, `docker`, or `oras`.

## Next

- [agents.md](agents.md): the output and error contract, and a worked loop.
- [user-journeys.md](user-journeys.md): twelve scenarios with their commands in order.
- [TENETS.md](../TENETS.md): the rules `hipc` keeps.
