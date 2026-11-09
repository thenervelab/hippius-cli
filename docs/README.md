# hipc documentation

Each layer is short and points to the next. A person starts at the README; a program starts at `hipc schema`.

| Read | When | Minutes |
| --- | --- | --- |
| [README](../README.md) | What `hipc` is, how to install it, how to log in, the first commands | 2 |
| [getting-started.md](getting-started.md) | Install options, verifying a download, login, text and JSON, accounts and projects, profiles, completions | 5 |
| [agents.md](agents.md) | The agent playbook: discovery, the output and error contract, confirmations, dry runs, pagination, selectors, secrets, App Engine, a worked loop; written to be pasted into an agent's context | 10 |
| [commands.md](commands.md) | Every `--help` page, rendered from the binary by `scripts/docs-reference.sh`; a test keeps it current | reference |
| [user-journeys.md](user-journeys.md) | Twelve scenarios with their commands in order, which `scripts/manual_test.sh` runs against the live API | reference |
| [TENETS.md](../TENETS.md) | The contract: output, errors, exit codes, confirmation, secrets, naming, documentation | reference |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | How to build, test, and change `hipc`, and what a pull request carries | contributors |
| [SECURITY.md](../SECURITY.md) | How to report a security problem, and what `hipc` promises about the token | reference |
| [CHANGELOG.md](../CHANGELOG.md) | What changed in each version | reference |

The same material for the whole Hippius platform, with the console, the apps, and the products, is at https://docs.hippius.com, which has no CLI section for `hipc` yet; this directory is the CLI's documentation until it does. https://docs.hippius.com/llms.txt is the index written for agents.
