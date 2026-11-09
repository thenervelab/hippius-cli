# Security policy

## Reporting a vulnerability

Report a vulnerability in `hipc` privately. Never open a public issue, discussion, or pull
request for it.

- Preferred: GitHub private vulnerability reporting. Open
  https://github.com/thenervelab/hippius-cli/security/advisories/new, or use the "Report a
  vulnerability" button on the repository's Security tab.
- Or email `support@hippius.com`, the contact in
  https://hippius.com/.well-known/security.txt, with "hipc security" in the subject.

Say which version you ran (`hipc version`), on which OS, the command, and what happened.
`--debug` output helps. `hipc` redacts tokens and secrets in it, but read it before you send
it.

A vulnerability in the Hippius console or API itself, rather than in this client, goes to
the same email address.

## Supported versions

Fixes land in the next release. Older versions get no backports.

## If a secret leaked

If an API token, an S3 secret, a database password, or any other secret appeared anywhere
it should not (a report, an issue, a log, a chat, a shell history), rotate it before
anything else:

- the API token: create a new one in the console settings at
  https://console.hippius.com/dashboard/settings, delete the old one, then run
  `hipc auth login --token-file <path>`;
- an S3 token: `hipc s3 tokens rotate --kind <kind> <token_id> --reveal --yes`;
- a Hub CI key: `hipc hub keys rotate <key_id> --reveal --yes`;
- the Hub docker robot secret: `hipc hub namespace robot rotate --reveal --yes`.

Deleting the text afterwards does not undo the leak.

## What `hipc` promises

A report can show any of these broken:

- a token or secret printed without `--reveal`, printed on stderr, or left in a `--debug`
  line;
- a token sent to an origin it was not stored for, `HIPPIUS_TOKEN` sent to an origin other
  than `https://api.hippius.com` or the one `HIPPIUS_TOKEN_ORIGIN` names, a token sent over
  plain `http` to anything but loopback, or along a redirect;
- `hipc api` sending a request to any origin but the configured API origin, or to the
  account token route `/auth/token/`;
- a write or a spend sent without the confirmation, `--reveal`, `--public`,
  `--remove-others`, `--replace`, or `--change-isolation` the contract requires;
- the config or AWS files read when they are a symlink or open to other users, or written
  with a wider mode;
- server text that reaches a terminal unescaped.

`hipc api` prints an answer as the server sent it, so it is not held to the curated
commands' removal of secret-like fields and `docker login` commands; it still escapes
server text for a terminal. `hipc api POST /registry/robot/rotate/ --reveal --yes`
prints any `docker login` command the answer carries, `docker_login_cmd` included, and such
a command may hold the new secret on its command line, where a shell history can keep it.
`hipc hub namespace robot rotate --reveal --yes` drops those fields, prints a
`docker login --password-stdin` command of its own, and refuses an answer with any other
`docker login` command; rotate the robot secret with it.

[TENETS.md](TENETS.md) states the full contract.
