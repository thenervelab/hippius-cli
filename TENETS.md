# Tenets

The contract of the `hipc` command, for the people and the programs that call it. Each tenet is a rule `hipc` keeps; tests hold it to most of them. A change that breaks one is a breaking change.

`hipc` has two callers: a person at a terminal, and a program, such as a coding agent, that parses stdout. When they want different things, the program wins. A person can read JSON; a program cannot recover from a spinner, a pager, or a secret printed once into a terminal animation.

`hipc schema` carries the machine-readable half of this contract: every command, its effect, its flags, its confirmation and guards, and every error code. Read it from the binary rather than from this file.

## 1. One program

One binary, `hipc`: one synopsis, one way to authenticate, one way to select an account. Version, help, and schema are part of it. There is no component manager and no alpha or beta tree.

The Homebrew formula and the command name `hipc` were taken over from the deprecated IPFS-era tool of that name; this program implements none of its commands (Docker registry, IPFS storage), and it does not replace `hippius-hub`, which moves Hub blobs.

## 2. Resource, then verb

The grammar is `hipc <service> <resource> <verb>`. A few commands sit at the top because they are not resources: `auth`, `api`, `schema`, `version`, `status`, `completions`.

Verbs come from a closed shelf: `list`, `get`, `create`, `update`, `delete`, plus the product's own words (`subscribe`, `change`, `cancel`, `start`, `stop`, `reboot`, `resize`, `restart`, `restore`, `resume`, `revoke`, `rotate`, `scale`, `replace`, `verify`, `connect`, `test`, `rollback`, `presign`, `dns-setup`, `domain-connect`, `use`, `login`, `logout`, `accept`, `reply`, `configure`, `card-setup`, `top-up`, `estimate`, `preview`, `transfer`, `decline`, `validate`, `fix`). `get` reads one resource, with an id or without one (`hipc account get`). The exception is `hipc network get`: its first call creates the account's private network, so every call is confirmed, and `hipc network create` is the way to set the network up on purpose. There is no `show` or `describe`, and no synonym for a verb that exists: a Hub namespace is created with `hipc hub namespace create`, though the server calls it provisioning. Every other leaf is a noun, and a test holds each leaf to the shelf or to a reviewed list of nouns, so a new verb joins the shelf here first.

A group exists because it holds several commands. Two kinds of group may hold one: a fixed part of the grammar (`hipc support`, which holds the `tickets` group), and, for a short while, a resource whose list must be `<noun> list` before its other commands arrive (`hipc s3 buckets objects` held `list` alone until `presign` joined it; `hipc network groups`, `hipc network policies`, `hipc network vpcs resources`, `hipc network vpcs routers`, and `hipc compute kubernetes backups` hold `list` alone until their writes arrive, and `hipc hub events` until the API has another events route; `hipc support tickets messages` holds `list` alone while an attachment upload stays on `hipc api`). A leaf that is a noun reads the thing it names and changes nothing (`hipc compute prices`, `hipc cdn stats <zone_id>`); the one exception among the resources is `hipc compute kubernetes clusters kubeconfig <cluster_id>`, which reads the kubeconfig and writes it to the file `--output-file` names, never to stdout (tenet 9). `hipc api` is a noun too, a top-level command that is not a resource, and it sends whatever method and route it is given, so it can write; its writes are confirmed as the commands' are (tenet 5). A read about a verb's operation sits beside it as `<verb>-options` or `<verb>-status` (`vms resize-options`, `vms resize-status`). A group's noun is plural when its parent holds several of the thing and singular when it holds at most one (`exposures`, `public-ip`). The one exception is `hipc hub subscription`, singular though an account may hold several, so that it reads as `hipc drive subscription` and `hipc s3 subscription` do (tenet 11).

Global flags work anywhere after `hipc`: `--profile`, `--api-url`, `--account`, `--project`, `--billing`, `--output`, `--json`, `--quiet`, `--debug`, `--yes`, `--no-input`, `--dry-run`, and `--request-timeout`. `-V` / `--version` prints the version. `--quiet` drops text-mode notes and warnings, never an error or its hint. There is no `--verbose`, no pager flag, and no color flag.

Help, version, and schema never touch the network.

## 3. Output is a document, not a conversation

`--output` is `auto`, `text`, or `json`; `--json` is `--output json`, and passing both is `usage`. `auto` prints JSON when stdout is not a terminal, when `CI` is set to any value, or when `AI_AGENT` is set to anything but empty, `0`, `false`, `no`, or `off` (trimmed, in any case). Otherwise it prints text. A flag always wins over detection.

`--help`, `--version`, and the `hipc completions` script are text in every mode. `hipc schema` is JSON in every mode. `hipc version` is the JSON form of `-V`.

A successful JSON document on stdout is the resource itself, with no `{"ok": true, "data": ...}` wrapper and no `ok` field: a success is exit 0, a failure the error object. Three exceptions add `next_step`, the `hipc` command that goes on with something a person completes in a browser: a Drive or S3 card subscribe (HTTP 202) names the command that reads its progress, `hipc billing auto-reload card-setup` names `hipc billing auto-reload get`, which shows the card it saves, and a CDN hostname's Domain Connect link (`hipc cdn hostnames domain-connect`) names `hipc cdn hostnames list`, which shows the hostname's status. Only `hipc` sets `next_step`: one the server sends, at any depth, is dropped from every document before it is printed. `hipc api` prints the server's body as sent, so a `next_step` there is the server's.

A list is `{"items":[...],"next_page_token":null}`, with any other fields of the server's page (such as `count`) beside `items`. `--all` prints one JSON array and follows pages to the end, at most 1000 pages or 64 MiB, and past either exits 1 `internal` rather than return a list cut short. An answer body is read up to 32 MiB; a larger 2xx answer exits 1 `internal`.

stdout holds the result and nothing else; stderr holds diagnostics. A failed command leaves stdout empty, except where it has a record of what was done to keep, so an id or a one-time secret is not lost: a failed command that printed a document on stdout printed what was done; keep it. That is `partial_failure`, a VM or app launch whose wait ended before `running` or whose answer `hipc` cannot use, a managed database whose credentials could not all be written to `--credentials-dir`, a one-time secret that could not be written to `--secret-file`, and a database create or restore whose answer is not JSON (`unparsed_answer`). An empty stdout does not mean nothing was done: `side_effects` says whether a write may have been.

No pager, ever. No spinner, no color, and no update check.

## 4. Errors are one JSON object, and the exit code is small

Exit codes:

| Code | Meaning |
| --- | --- |
| 0 | The command did what it said, including `--dry-run`. |
| 1 | The API or the runtime failed. Authentication failure is 1. |
| 2 | The invocation must change before it can work: a flag or argument, the environment (`HOME` unset, a malformed `HIPPIUS_*` variable), a local file it reads (a config or AWS file that is a symlink, too large, owned by another user, open to others, or malformed), or a missing confirmation flag. Nothing that changes state was sent. |

A missing token, or a stored token or `HIPPIUS_TOKEN` for another origin, is `auth` and exit 1: logging in, or setting `HIPPIUS_TOKEN_ORIGIN` for the variable, fixes it. `hipc` installs no signal handler, so SIGINT and SIGTERM end it the default way (a shell reports 130 or 143).

When the output mode is JSON, a failure prints exactly one JSON object on stderr, apart from any `--debug` lines. That holds for argv errors too: the mode is read before the arguments are parsed.

```json
{
  "error": {
    "code": "not_found",
    "message": "cannot get the VM `vm_123`: the API says `Not found.`",
    "request_id": null,
    "hint": "run `hipc compute vms list` to see what exists",
    "retryable": false,
    "side_effects": "none",
    "http_status": 404,
    "retry_after_ms": null,
    "details": {"detail": "Not found."}
  }
}
```

Every field is always present; one that does not apply is `null`, `details` included.

`code` is the contract; branch on it, never on `message`. The closed set, with its exit code:

| Code | Exit | Meaning |
| --- | --- | --- |
| `usage` | 2 | the invocation is wrong: a flag, an argument, a variable, or a local file it reads |
| `confirmation_required` | 2 | the command needs a confirmation, and `--yes` was not passed |
| `reveal_required` | 2 | the call returns a secret, and `--reveal` was not passed |
| `replace_required` | 2 | a paid Hub plan is active and `--replace` was not passed, or a Hub subscribe was sent in the last 10 minutes and no paid plan is listed that it did not cancel |
| `remove_others_required` | 2 | the new list leaves out entries the list has now, and `--remove-others` was not passed |
| `public_required` | 2 | the write may publish a private bucket or open the Hub namespace to anyone, and `--public` was not passed |
| `change_isolation_required` | 2 | the call changes which peers of the network can reach each other, and `--change-isolation` was not passed |
| `auth` | 1 | 401, or no token is configured for the API origin |
| `permission` | 1 | 403 with no account code |
| `not_found` | 1 | 404 |
| `conflict` | 1 | 409, or 412: another client changed the resource first; or a state `hipc` read that rules the write out, sent nothing, with `http_status` null |
| `payment_required` | 1 | 402 |
| `validation` | 1 | 400, 422, or any other status this table does not name |
| `rate_limited` | 1 | 429 |
| `timeout` | 1 | the request or the wait ran out of time |
| `unavailable` | 1 | the connection failed, or 425, 502, 503, or 504 |
| `partial_failure` | 1 | part of a multi-call command failed, and stdout holds what was done |
| `internal` | 1 | 500, 501, a 5xx not listed under `unavailable`, a redirect (never followed), an answer `hipc` cannot read, or output `hipc` could not write |
| `not_a_member` | 1 | the caller is not a member of the selected account |
| `account_scope_unsupported` | 2 | the route cannot be used in another account, so `--account` must select the token's own |
| `project_scope_unsupported` | 2 | the route does not take a project, so `--project` must select the Default one |
| `billing_scope_unsupported` | 2 | the route does not take a billing account, so `--billing` must select the Default one |
| `owner_only` | 1 | only the account owner can do this |
| `insufficient_grant` | 1 | the member's grant on the product is too low |
| `team_management_required` | 1 | the member needs the Manage the team permission |
| `billing_required` | 1 | the member needs the Billing permission |

`hipc schema` prints this table in its `errors` array. The last eight are account, selector, and grant failures; a 403 that carries one passes the server's code through unchanged. A new code may be added in a minor version; an existing code is never renamed or reused.

Each exit-2 code other than `usage` names exactly one flag, and its hint adds it. The one exception is `replace_required` inside the Hub double-subscribe window (tenet 11): when the list shows no paid plan the last subscribe did not already cancel, `--replace` is refused too, and the hint says to wait instead, or, for a marker dated in the future, names the profile field to remove. A second exception is a server's `account_scope_unsupported`, `project_scope_unsupported`, or `billing_scope_unsupported` on a request that carried no such header: no flag value would change the answer, so the hint names none and says to report it. A `usage` hint names what to change: a flag, a variable, or a `chmod`.

`side_effects` is `none` or `partial`. A failed read, or a write the server rejected with a 4xx, is `none`. A timeout, a 5xx, or a redirect after a write was sent is `partial` and not retryable, because a retry could do the write twice. One exception: a 503 with `Retry-After` to the Drive or S3 subscribe is `none` and retryable after that delay.

`hipc` does not retry a request; `retryable` and `retry_after_ms` tell the caller when to. Three loops read again, each through up to three retryable read failures in a row, and after a failure that carries `Retry-After` they wait that long when it is longer than their interval: the VM launch wait and the App Engine launch wait, every 2 seconds, and the follow of a hAlpha top-up's purchase, every 3 seconds for at most 15 minutes, which ends with the purchase as it stands and exit 0. A `Retry-After` that runs past a loop's deadline ends the loop at the deadline, without another read.

Text mode prints `error:` and `hint:` lines on stderr instead, with the same exit code and an empty stdout.

## 5. Confirmation is opt-in, and `--force` is not `--yes`

A command that spends money, deletes, revokes, rotates, or cancels does not run until it is confirmed, and neither does a write that can damage or redirect what the account does: stopping a VM, adding items to a WAF list, storing an SSH key, which is an access grant to every VM launched with it, a login that replaces a different token stored in the profile, a `hipc s3 configure` that overwrites an AWS profile, a billing profile update that changes the billing email, and a notification update that turns an alert off. Opening a support ticket is confirmed too, because a person reads it, and so is closing one, which ends the support team's work on it; a reply is not, but a reply or a new ticket whose text holds the API token or an access key is refused with `usage`, through `hipc api` too; the check catches an accident, such as a pasted config file, and not a secret encoded on purpose. A spend cap (a CDN zone's cap, the runner limit and settings, auto-reload) is confirmed for a change that may raise spend, and not for a cut. `confirmation` and `confirmation_when` in `hipc schema` say which commands ask, and when.

The command prompts only when the output mode is text, stdin, stdout, and stderr are terminals, no CI or agent variable is set, and neither `--yes` nor `--no-input` was passed. The default answer is no. In every other case it exits 2 `confirmation_required`, with a hint that names `--yes`, and sends nothing.

Some writes need a flag that names their risk, besides `--yes`. Each is its own exit-2 code, `--yes` does not imply it, and it does not imply `--yes`:

- `--reveal`: the answer holds a secret (tenet 9).
- `--remove-others`: a write that replaces a whole list (firewall rules, CDN cache rules, WAF rules and rate limits, a WAF list's items, a member's grants) would drop an entry the list has now. Without it, the command reads the list and refuses a removal.
- `--public`: a CDN zone create, or an update that moves its origin, may publish a private bucket. A bucket ACL update needs both `--public` and `--remove-others`. A Hub namespace update that makes it public (`--visibility public`) lets anyone pull from it; through `hipc api`, a write of the publicity route needs `--public` unless the body sets `public` to `false`.
- `--replace`: a Hub subscribe while a paid plan is active (tenet 11).

`hipc api` applies the same flags to the same routes, and one of its own: a network isolation change needs `--change-isolation`, refused without it as `change_isolation_required`. Each flag and each refusal holds for every write method of its route, not only the one the API documents (a `DELETE` of the deletion request, which withdraws it, is sent), and for the route in any letter case. An account deletion request, the read of a Kubernetes cluster's sealed kubeconfig (a `GET` or `HEAD` of its secrets), a CDN hostname's DNS setup, and a Hub subscribe are never sent; the console, `hipc compute kubernetes clusters kubeconfig`, `hipc cdn hostnames dns-setup`, and `hipc hub subscription subscribe`, which reads the current plans first and refuses a second paid one, are the paths to them. `guards` and `refused_when` on `api` in `hipc schema` list them.

The checks run in a fixed order, all before any request that changes state: argv, then `--reveal` and `--public`, then `hipc api`'s route flags, then `--dry-run` and the local account, project, and billing refusals, then the token, then the confirmation. A read that only names the target in the question (the VM a restore overwrites or a delete removes, the account an invite joins, the list a replace would change, the Kubernetes cluster a delete removes, the operation a cancel or resume acts on, the node pool a scale or a node replacement grows, the transfer an accept, decline, or cancel acts on, the project and accounts an offer names, and the support ticket a close ends) may run before the question, and so may the read of the setting in force that decides whether a conditional confirmation asks (a CDN zone's cap, the runner limit and settings, auto-reload, the billing profile's email), and the `POST validate/` an App Engine launch sends with the compose alone, to check it and the secrets it declares; none of them changes anything.

There is no `--force`. `--yes` is the one word for every confirmation.

`--dry-run` prints the requests a real run would send, sends nothing (not even a read), and exits 0. It needs no token and none of the flags above. Its document is `dry_run`, then `requests`: the requests a real run sends, in order, reads included, one request alone included, and no entry repeats `dry_run`, with a placeholder such as `<the VM's name>` for a value only a read can give. It leaves out the polls of a VM launch's wait and of a hAlpha top-up's follow, and the later pages of `--all`, which follow from an answer. After `requests`, `confirmation` and `confirmation_when` say what `hipc schema` says of the command: whether the real run asks for `--yes`, and when, if that depends on what it reads or is given. Each request names its `method`, `url`, `account_header`, `project_header`, and `billing_header` (each `null` when the request goes out without it), and `body`; `if_match` is present only on a request that sends a precondition. A command that only changes a local file sends no request: it names its `action` and the profile it would change, and a login says in `replaces_token` whether it would replace a different stored token, or, when the token is not known (no `--token-file`), whether one is stored. `reads_first` (a list replace) and `may_cancel_first` (Hub `--replace`) stay beside `requests`. The one placeholder not written `<...>` is `{subscription_id}` in the cancel path of `may_cancel_first`, `/registry/subscriptions/{subscription_id}/`, kept because changing a field's value is a breaking change (tenet 15). A route `hipc api` never sends is refused even in a dry run. `--help`, `hipc version`, `hipc schema`, and `hipc completions` send nothing anyway and ignore `--dry-run`: they print their usual output.

## 6. One page, unless asked

A list returns one page, 100 items by default. `--limit` sets the page size, `--page-token` asks for a later page, and `--all` follows to the end (tenet 3). `--all` means nothing else anywhere: a purge of a whole CDN zone is `--everything`. Only a list the server pages takes these flags, and a list paged by a server cursor at a fixed page size (`hipc compute runners jobs`) has no `--limit`. One list is read whole on purpose: `hipc s3 tokens list` prints the master and the sub tokens, two routes, as one list, so it takes none of these flags, follows every `next` page of each route within the caps `--all` has (tenet 3), and prints `next_page_token` null. `pagination` and `flags` in `hipc schema` say which lists take which.

`next_page_token` is the next page number as a decimal string, or the server's cursor where a route pages by cursor, and null on the last page. A page is never printed as the whole list without saying more exist.

## 7. The command line is discoverable without a human

`hipc schema` prints `schema_version`, every command with its effect, confirmation, guards, pagination, arguments and flags, the global flags once, and every error code with its exit code, meaning, and flag. Given a command path, such as `hipc schema compute app-engine`, it lists only the commands under it and every other field as without one, so the filtered document is the same shape under the same `schema_version`. The arguments and flags are read from the parser, so they cannot drift from it. A unit goes in the value name, never in the flag name: `--storage-cap <BYTES>`, `--expires-in <DAYS>`, `--wait-timeout <SECONDS>`, `--traffic <REGION=GB>`. Read the value name on each command: `hipc hub keys create --expires-in` takes `<DAYS>`, `hipc s3 buckets objects presign --expires-in` takes `<SECONDS>`. No flag is named a unit or ends in one.

The effect says what a repeat does to the server:

- `read_only`: nothing changes.
- `idempotent`: a repeat leaves the server as one call did, though it may answer 404 or 409. Deletes, revokes, cancels, VM start and stop, and local file writes.
- `non_idempotent`: a repeat does more work or spends again. Creates, subscribes, plan changes, rotates, reboot, and resize. `hipc api` reports this, the effect of its most dangerous method.

`hipc api <METHOD> <PATH>` calls any console endpoint with the same token, account rules, guards, and error object. It sends only to the configured API origin: a full URL on another is `usage`. On a pipe in JSON mode a 2xx body is printed byte for byte, so a large number is not rounded; whenever a person may read it, control and invisible characters are escaped. A non-2xx is exit 1 with the parsed body in `error.details`. `--all` follows `next` links only on the first page's origin and route.

There is no `--jq` and no `--fields`: pipe the JSON to `jq`. `hipc completions <shell>` is for people; programs read `hipc schema`.

## 8. A profile is a token plus an account

`hipc auth login` stores a token the caller already holds, read from `--token-file <path>` (`-` for stdin) and never from argv, where `ps` shows it. Without `--token-file`, a person at a terminal is asked for it with hidden input, under the prompt rules of tenet 5; anyone else exits 2 `usage`, with a hint that names `--token-file`. Create a token in the console settings at https://console.hippius.com/dashboard/settings after a Google, GitHub, or Apple sign-in; an account that signs in with a 12-word access key gets its token from the access-key sign-in described at https://docs.hippius.com/use/wallet-login. A token is the only way in: `hipc` never signs with an account's own key, so the 12 words are never an input to `hipc`, and hAlpha buys an account signs itself are made in the console. A login while `HIPPIUS_TOKEN` is set stores the token and warns that later commands still send the variable.

A stored token belongs to the API origin it was stored for and is sent only there. When `--api-url` or `HIPPIUS_API_URL` points elsewhere, the token is withheld and a command that needs it exits 1 `auth`. A login for any origin but `https://api.hippius.com` asks first, or needs `--yes`. `HIPPIUS_TOKEN` is the token for one run and is never written to the file. It is bound the same way: it goes only to `https://api.hippius.com`, or to the one origin `HIPPIUS_TOKEN_ORIGIN` names (normalized as `--api-url` is), so `--api-url` alone cannot send it elsewhere. A run that targets another origin withholds it, uses no stored token instead, and a command that needs a token exits 1 `auth`.

One token can act in several accounts. `hipc account list` shows them; `--account <id>` picks one for a run and `hipc account use <account_id>` for the profile; `self` goes back to the token's own. A project and a billing account are picked the same way, with `--project`, `--billing`, `HIPPIUS_PROJECT`, `HIPPIUS_BILLING`, or `hipc account projects use` and `hipc account billing-accounts use`; `default` goes back to the account itself. Switching accounts clears a stored project and billing account.

Precedence is flag, then environment, then the profile. The variables are `HIPPIUS_TOKEN`, `HIPPIUS_TOKEN_ORIGIN`, `HIPPIUS_ACCOUNT`, `HIPPIUS_PROJECT`, `HIPPIUS_BILLING`, `HIPPIUS_API_URL`, `HIPPIUS_PROFILE`, and `HIPPIUS_CONFIG`.

The config file is `~/.hippius/config.toml`, or the path in `HIPPIUS_CONFIG`. It holds tokens, so `hipc` guards it as OpenSSH guards a private key:

- The directory is created with mode `0700` and a new file with mode `0600`; an existing file keeps its owner bits. Writes go through an exclusive temporary file, a rename, and an fsync, under a lock.
- `hipc` refuses a config that is a symlink, is not a regular file, is larger than 1 MiB, belongs to another user, or has any permission bit for the group or others, and a directory that is a symlink or that others can write. The checks run on the open descriptor, not the path.
- `hipc s3 configure` holds the AWS files to the same rules.
- `hipc` never changes a mode for you; the error names the `chmod` that fixes it. Access-control lists are not checked.
- A change rewrites the whole file, so comments are not kept. The rename splits a hard link: the other name keeps the old contents, tokens included. The change that splits it warns, once; later changes see a file with one link and do not.
- `hipc` clears its own copies of the token from memory when it is done with them, best effort, sets its core dump limit to zero at start, and on Linux marks itself not dumpable, so a crash dump or another process of the user does not get the token from it.

The origin must be `https`, except the loopback addresses `127.0.0.1` and `[::1]`. Redirects are never followed.

## 9. A secret is printed once, and only when asked

A command whose answer holds a secret needs `--reveal`, or a file flag that keeps the secret off stdout (below), or it exits 2 `reveal_required` and sends nothing: creating or rotating an S3 token or a Hub CI key, creating a Hub namespace (`hipc hub namespace create`), rotating the Hub docker robot secret, creating or restoring a managed database, reading a Kubernetes cluster's backups, whose download links work without a token, and presigning an S3 object's URL (`hipc s3 buckets objects presign`), which reads the object without one. `hipc api` applies the rule to every route whose answer holds a secret, reads included, such as the account's master token. `reveal` and `reveal_when` in `hipc schema` say which, and `reveal_when` names the file flag that serves in its place.

With `--reveal`, the secret goes on stdout in the resource; with a file flag, into new mode-0600 files the caller named; and nowhere else: `--secret-file <path>` on the S3 token and Hub key creates and rotates, the Hub namespace create, and the Hub robot rotate writes the secret alone to one file, and `--credentials-dir <dir>` on `hipc compute databases create` and `restore` writes every credential to files in that directory. The document then names the files in place of the secrets. Naming the file asks for the secret as plainly as `--reveal` does, so either flag lets the command send; `--reveal` beside the file flag changes nothing. The file flags keep the secret out of the stdout a log or an agent's transcript holds, so they are the form to use; only a file that cannot be written after the secret was issued puts the whole answer on stdout instead (tenet 3), so the secret is not lost. stderr never holds one. A later list or get shows a preview and `secret: null`; the recovery is a rotate.

A Kubernetes cluster's kubeconfig is opened on the caller's machine with the key file the console saved when it created the cluster (`--cluster-key-file <path>`), and goes only to a new mode-0600 file `--output-file` names, never to stdout, so it needs no `--reveal`. The key file is held to the config file's checks (tenet 8): a regular file of the current user with no bits for the group or others, not a symlink, at most 1 MiB, in a directory no one else can write. Its private keys are never sent, printed, or stored; `hipc` uses them to open the kubeconfig and to sign each node a scale or a replacement adds. Before it opens the kubeconfig, `hipc` checks its signature with the cluster's guest key, which the API serves: that proves the drop is whole and is this cluster's, not that it is authentic against a compromised API, which could serve both the key and the drop.

`--debug` redacts `Authorization`, cookies, tokens, passwords, and secrets. Every URL a message, hint, note, or `--debug` line quotes shows its query names and no values. A JSON-mode dry run on a pipe is the one exception: its `url` is the exact request.

An invite token is read from `--token-file` only and appears in no output. A DNS provider credential for `hipc cdn hostnames dns-setup` is read from `--credentials-file` only, a JSON file held to the config file's checks; it goes in that one request's body and appears in no output, `--debug` line, or error. `hipc api` never sends that route, since its body and answer hold the credential. Its answer prints only what `hipc` can check (the provider it sent, a `mode` the API documents, a boolean `replace`), and its errors say only the HTTP status, without the server's words or `details`: the server could repeat the credential in a form `hipc` cannot recognise. An error answer that repeats the API token has it replaced by `<redacted>`.

Never paste a Drive unlock password, a recovery seed, a 12-word access key, or a one-time database password into a chat, a shell history, or a flag. They are not inputs to `hipc`. A Bittensor hotkey's words are one only in the file `--hotkey-file` names (or stdin with `-`), for `hipc billing gamma hotkeys create`, and a command line that holds 12 words of the BIP-39 list in a row is refused without being repeated.

## 10. This is the control plane

`hipc` talks to `https://api.hippius.com`. It does not move bytes. S3 objects stay with the AWS CLI, boto3, or rclone against `https://s3.hippius.com` (`hipc s3 configure` writes an AWS profile for it); Drive files stay with the Drive apps and SDK; Hub blobs stay with `hippius-hub`, docker, or oras against `registry.hippius.com`.

## 11. One subscription grammar, not one HTTP path

Drive, S3, and Hub are sold as plans; compute and CDN draw down the balance by usage and have no subscription commands. Where plans exist, the grammar is the same:

```text
hipc <service> plans
hipc <service> subscription get
hipc <service> subscription subscribe --plan <PLAN> [--period monthly|annual] [--pay-from credits|card]
hipc <service> subscription change --plan <PLAN> [--period monthly|annual] | --pay-from credits|card | --storage-cap <BYTES>
hipc <service> subscription cancel
```

The plan is always the `--plan` flag, never a positional argument, and how it is paid is always `--pay-from`, on a subscribe and on a change alike (a change sets how later renewals are paid). `--period` is Drive's only. Hub takes no `--pay-from`: its plans are paid from the balance only. A one-off payment that is not a plan, `hipc billing top-up`, keeps `--method`.

Hub has no `change`. Hub lets an account hold a plan twice, so `hipc hub subscription subscribe` exits 2 `replace_required` while a paid Hub plan is active; `--replace` cancels it first. A new subscription shows in the list only minutes later, so the profile remembers a subscribe for 10 minutes (the `hub_subscribe` field) and refuses a second one in that window with `replace_required`. Inside the window, `--replace` is accepted only when the list shows a paid plan the last subscribe did not already cancel; it cancels that plan and subscribes. Otherwise the hint says to wait and check `hipc hub subscription get`. A marker dated more than 10 minutes in the future, which a clock that stepped back leaves, is not waited out: its hint names the `hub_subscribe` field to remove from the profile. The marker of every profile in the config file that holds the same origin and token counts, so a second profile with that token is refused too. A subscribe made from another machine, or with another `HIPPIUS_CONFIG`, in that window is not seen.

A Drive or S3 card checkout prints its URL and does not wait; nothing is bought until it is paid. `hipc <drive|s3> subscription checkout <intent_id>` reads its progress.

`hipc` hardcodes no price. Prices come from the catalogs and `hipc compute prices` (the account's, in base units) or `hipc compute public-prices` (in credits, with no token).

## 12. The account header is part of the request, or it is a bug

Each route is in one class, decided from its path:

- **Account-scoped** (compute, S3, the object store, CDN, the balance, and the rest of the console's account routes): with a membership selected, `hipc` sends `X-Hippius-Account`.
- **Personal** (`hipc account list`, SSH keys, support tickets, project transfers, referral links, the signed-in user's settings): acts as the caller in every account; the header is never sent.
- **Everything else** (Drive, Hub, billing transactions): with a membership selected, `hipc` refuses locally with `account_scope_unsupported` and sends nothing, rather than quietly act on the caller's own account.

The header is left out when the selected account is the caller's own.

A project (`X-Hippius-Project`) goes on compute, databases, Kubernetes, runners, S3 buckets and sub-token keys, CDN, and the S3 plan reads (`GET` only), but not on the S3 master keys: `hipc s3 tokens create --kind master --project <id>` is `project_scope_unsupported`. A billing account (`X-Hippius-Billing`) goes on the balance, invoices, the billing profile, card and Bitcoin top-ups, auto-reload, and the billing reads. The hAlpha quote, buy, and purchases act on the signed-in user's own hAlpha and credit the user's own balance: like the routes of the last class above, they refuse a membership, and a project or a billing account from a flag or a variable, and a stored selection follows the rules below. No route takes both, so a project and a billing account both given by flag or variable refuse every route but the personal ones. A route that cannot take one refuses a selection from a flag or a variable with `project_scope_unsupported` or `billing_scope_unsupported`. A selection stored in the profile is skipped on reads of such a route. On writes a stored project is refused, since without the header the write would act in the Default project, and a stored billing account is skipped: such a write goes out without the header, so one that spends is paid by the Default billing account, and its confirmation names the billing account it goes out without. A stored project and a stored billing account are both skipped on writes to the four routes that name one project id in their path (the project itself, its budget, quota, and transfer) and to the route that names one billing account id in its path (its rename), since the path, not a header, says which one the write acts on, and none of those routes takes either header; `hipc account projects delete` of the stored project clears the selection (a `hipc api DELETE` of it does not).

Plan purchases and changes are owner-only. A member tops up only a selected billing account they administer. `owner_only`, `not_a_member`, `insufficient_grant`, `team_management_required`, and `billing_required` come from the server unchanged.

## 13. Ship a binary, not a package

The source is https://github.com/thenervelab/hippius-cli. Each release is a GitHub release there: tarballs for macOS on Apple silicon and Intel and static (musl) Linux on x86_64 and aarch64, `SHA256SUMS`, and `SHA256SUMS.sigstore.json`, a cosign keyless signature of `SHA256SUMS` made by the release workflow. The macOS binaries are not notarized; after a browser download, run `xattr -d com.apple.quarantine ./hipc`.

The install script checks the hash, and the signature when `cosign` is on `PATH`; `HIPC_INSTALL_REQUIRE_SIGNATURE=1` makes a missing `cosign` a failure. Homebrew installs the formula `hipc` from `thenervelab/tap`, a name taken over from the deprecated IPFS-era tool (tenet 1). PyPI ships binary-only wheels under the name `hippius` (`pip install hippius`), which install the command `hipc`, once the release workflow's `PYPI_PUBLISH` variable is turned on for the trusted publisher. `hipc` is not published to crates.io.

## 14. What we refuse

- A pager, a component manager, or alpha and beta trees.
- A generated command for each API operation; `hipc api` covers the rest.
- Uploading or downloading S3 objects, Drive files, or Hub blobs.
- A TUI on the default path.
- An `--idempotency-key` flag the server would ignore.
- A one-time secret printed twice, or without `--reveal`.
- A token in argv.
- A hardcoded price.
- A second Hub subscribe that does not warn.
- A `--dry-run` that calls the server or fails.
- An update check that can fail the command.
- Rounding a server number.
- Reading `CI` or an agent variable as a reason to skip confirmation; either one requires `--yes`.

## 15. One voice: how `hipc` speaks

### Streams

| Stream | Holds | Modes |
| --- | --- | --- |
| stdout | The result: the JSON document, the text table, the `hipc api` body, `--help`, `--version`, the completion script. Nothing else. | all |
| stderr | Everything about the run: `error:`, `hint:`, `note:`, `warning:`, and `debug:` lines, and the confirmation prompt. In JSON mode, the one error object of tenet 4 and no notes. | all |

A closed stdout pipe is exit 0 with nothing more written. Any other stdout write failure is `internal`.

### Line kinds

Every stderr line in text mode starts with one of these labels:

| Label | Means | Written when |
| --- | --- | --- |
| `error:` | What failed, in one line. | Once per failure. |
| `hint:` | One change likely to fix it. | Only when `hipc` knows a fix; every exit-2 failure has one. |
| `note:` | A fact about a result that succeeded. | Text mode; `--quiet` drops it. |
| `warning:` | It worked, but may not do what the caller expects. | Text mode; `--quiet` drops it. Never changes the exit code. JSON mode puts the config file's hard-link warning in a top-level `warnings` array of texts, on the document or beside `error`. |
| `debug:` | A request or status line, redacted (tenet 9). | Only with `--debug`, in every mode. |

The prompts are the only unlabeled lines. The confirmation prompt is `this will <what>; continue? [y/N] `, and `hipc auth login` asks for the token with ``paste the token from `https://console.hippius.com/dashboard/settings` (it is not shown): ``. Its refusal reuses it: ``this would <what>, and `--yes` was not passed``.

### Phrasing

The JSON `message` is the exact text after `error: ` in text mode.

- Lowercase, no final period; a second clause joins with `;` or `:`.
- State the problem, not the person. No "please", "sorry", or exclamation marks.
- A bad value names where it came from: the flag, the variable, the file, or the profile.
- Backticks around every literal: a flag, a command, a variable, an id, a path, a URL, a value.
- A hint is one imperative that names the exact action: ``pass `--yes` to confirm``, ``run `hipc auth login --token-file <path>` ``.
- The server's words are quoted, not adopted: `` cannot delete VM `vm_123`: the API says `VM is still starting` ``. The body stays in `error.details`. A backtick in server text is written `\u{60}`, and a control character as its escape (`\n`, `\u{1b}`), in the message itself.
- An `internal` error's hint names `--debug` and https://github.com/thenervelab/hippius-cli/issues, unless its side effects are `partial`: then it says to check whether the change was made before running the command again.

### Machine output is a contract

- A JSON document only gains fields. Renaming or removing a field, or changing its type or meaning, needs a new major version and a new `schema_version`. Before 1.0.0 a minor version may remove or rename a field, always with a new `schema_version`. Ignore fields you do not know.
- Nothing in a JSON document depends on the terminal's width, the locale, or color. Whether a person may read stdout (text mode, or a terminal) changes what is shown there: a `hipc api` body's control and invisible characters are escaped (tenet 7), and a dry run's `url` shows its query names and no values (tenet 9); only JSON mode on a pipe gets the exact bytes. Whether stdout is a terminal also decides whether `hipc compute vms create` waits for `running` (below), and so which VM document it prints.
- Output is never cut short without saying so.

### Terminals and agents

- `CI` and `AI_AGENT` pick JSON for `--output auto`, refuse prompts, and make `hipc compute vms create` return without waiting for `running` (`--wait` overrides). A variable only makes `hipc` stricter: it never confirms and never reveals.
- What reaches a terminal is escaped in every mode: a `hipc api` body's control characters, and a dry run's query values. Only a pipe gets the raw bytes.
- Nothing waits for input that may not come.

## 16. Documentation is part of the command

A command is done when a person and a program can learn it without asking anyone. The help is the prose half of this contract, `hipc schema` the machine half, and the generated reference is the help, so none of them can drift from the binary.

- Every command has a short help that says what it does in one sentence, with the flag it needs at the end (`; needs --yes`), and a long help that says what it sends, what it reads first, and what it refuses. Every help page, the top level, each group, and each command, ends with examples, and every example parses. The global options are listed in `hipc --help` and in `hipc schema`; every other page ends, after its examples, with one line that points there (`Global options: see hipc --help`), so a command's own flags are not lost among them. Tests hold the shape: a short help that is one sentence in sentence case with no final period, no backticks, and examples that parse. Review holds the content: what the command sends, reads first, and refuses, and the flag it names.
- Help is plain text, shown unchanged in a terminal, in `hipc schema`, and on the web: the short help is one sentence in sentence case with no final period, and no help text holds Markdown backticks or `<https://...>` link syntax. A literal is written as typed: `--yes`, `hipc compute vms list`, `https://console.hippius.com`.
- `docs/commands.md` is rendered from `hipc --help` and the `--help` of every group and command by `scripts/docs-reference.sh`. A test fails while the committed page differs from the built binary, so a help change ships with its page.
- A change to a command's surface, a new command, a rename, a flag, an effect, a confirmation, a guard, or an error code, is already a change to `hipc schema`; the same pull request changes `CHANGELOG.md`, the README and the docs pages where they show it, and the `llms.txt` and CLI pages of docs.hippius.com when an agent's first steps change.
- The docs speak as `hipc` speaks (tenet 15): plain sentences, literals in backticks in Markdown, no exclamation marks, no "please", and nothing private. Each layer is short and names the next: a person reads the README, then `docs/getting-started.md`; a program reads `hipc schema`, then `docs/agents.md`.
