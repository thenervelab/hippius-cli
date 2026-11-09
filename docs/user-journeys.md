# User journeys

This catalogue lists what people and programs do with `hipc`, as scenarios with their commands in order. `scripts/manual_test.sh` runs the same scenarios against the live API and walks the person holding the token through each step. It is the manual end-to-end test before a release.

The command blocks below are the script's own `--list --tier write` output, and the `manual_test` test in `crates/hippius-cli/tests/e2e` fails when the two differ. Change a scenario in the script, then paste its new `--list` block here.

Every command `hipc schema` names is run by some tier, as itself or as a `--dry-run`, or is named in the script's `EXCLUDED` list with a reason (`--excluded` prints it). A second test fails when a command is neither, so a new command needs a step or a reason in the same change.

## Running it

```sh
HIPPIUS_TOKEN=<token> scripts/manual_test.sh                 # read tier, latest release
HIPPIUS_TOKEN=<token> scripts/manual_test.sh --tier write    # also create and delete free resources
HIPPIUS_TOKEN=<token> scripts/manual_test.sh --tier spend    # also buy what the run undoes; asks first
HIPPIUS_TOKEN=<token> scripts/manual_test.sh --hipc target/debug/hipc s3 hub
scripts/manual_test.sh --list --tier write                   # print the scenarios, run nothing
scripts/manual_test.sh --excluded                            # the commands no tier runs, and why
scripts/manual_test.sh --offline --hipc target/debug/hipc      # test the script, no network
```

Options: `--tier read|write|spend`, `--yes-spend` (answer y to every purchase of the spend tier), `--hipc <path>`, `--log <path>` (default `./manual_test.log`, overwritten), `--pause` (wait for Enter after each step), `--docker` (log Docker in with the new Hub key), `--list`, `--excluded`, `--offline`, and scenario names to run only those.

For each step the script prints the step's title, why a user runs it, and the command line. Then it prints stdout and stderr as `hipc` wrote them, the exit code, and PASS or FAIL against the expectation. Everything printed also goes to the log. At the end it prints the counts and the list of failures, and exits 1 if anything failed.

The script needs `jq`. Without `--hipc` it also needs `gh`, signed in, and `cosign` 3.0 or later, and refuses to start without either of them, or with a `cosign` whose `cosign version` reports a major version below 3. Steps that use `ssh-keygen`, `aws`, or `docker` are skipped when the program is missing.

## The promises

**A fresh start.** Without `--hipc`, the script reads the latest release's tag, downloads its tarball, `SHA256SUMS`, and `SHA256SUMS.sigstore.json` with `gh release download`, verifies the signature with `cosign verify-blob` against the release workflow's identity for that tag, checks the tarball against its line in `SHA256SUMS`, and unpacks it, as the README tells a user to. A release with no `SHA256SUMS.sigstore.json`, made before the workflow signed its checksums, stops the run with one line before any scenario: the hash is not checked instead, and `--hipc <path>` is the way to test such a release. Each scenario runs in a new, empty `HOME`, with `XDG_CONFIG_HOME`, `AWS_CONFIG_FILE`, `AWS_SHARED_CREDENTIALS_FILE`, and `DOCKER_CONFIG` inside it. It logs in at the start and logs out at the end, and both are tested steps. The script unsets every `HIPPIUS_*`, `AWS_*`, and `DOCKER_*` variable it inherits, and `CI` and `AI_AGENT`, so the user's own credentials and settings cannot leak in. It refuses to run under `bash -x` or `SHELLOPTS=xtrace`, which would print the token.

**The token never shows.** The script reads `HIPPIUS_TOKEN` once and unsets it, so no `hipc` process gets the token from the environment. `hipc auth login --token-file -` reads it from stdin, so it is never in argv, where `ps` would show it. Each scenario's `hipc` reads it from the profile that login stored. Every printed line goes through a sed mask that replaces the token, the `token_preview` that `hipc auth status` prints, and every secret an answer holds on stdout or stderr (the S3 secret, the Hub key secret). So are the secret fields of every command that takes `--reveal`, such as a database's password, passphrase, and client key, and a Kubernetes backup's sealed key. A URL whose query carries a signature, a token, or a password (a presigned S3 URL, a snapshot download link) and a checkout URL are masked the same way; a dry run's request URL is not. A `password`, `passphrase`, `pass`, `pwd`, `secret`, or `token` in any query string, in any case, is masked wherever it appears. The secrets are replaced longest first, so a short secret that is part of a longer one cannot break the longer one up. Each secret is masked both as raw bytes and in its JSON-escaped form, and a secret with a line break is masked line by line. The mask is read with `sed -f`, so the token is not in sed's argv either. After the run, the script searches the log for the token, for every secret it masked or that a step's raw output holds, and for the parts of each secret around any shorter secret inside it, and fails if it finds one. The temporary directory, which holds the stored token and the secrets the write tier saves, is removed on exit.

**The account is left as it was found** in the read and write tiers; the spend tier allows exactly its own differences (see [The spend tier](#the-spend-tier)).

- The read tier changes nothing on the server. Its requests are GETs, plus one POST that stores nothing: `cdn estimate`, which prices a month of traffic. Every write appears as a `--dry-run`, which sends nothing.
- The write tier also creates six free resources: an S3 bucket, an S3 sub token for it, an SSH key, a Hub CI key, a project, and a CDN WAF list. It changes each one the way a user would (a bucket policy, a rotated token and key, a renamed project with a budget and a quota, a list's items) and deletes each in the same scenario, as a tested step. The delete must exit 0, and a list read after it must not show the resource (a revoked S3 token or an archived project may stay listed as such). It also changes three things the account already has, and puts each back, reading the field after each write, so a live run shows whether the field `hipc` sends is the one the server keeps: the runner concurrency (one lower, skipped at 1, and when it is above `quota_runners` or the settings show no quota, since the server refuses a value above the quota and so could not take it back), an existing runner spend cap of a whole number of credits (one credit higher; put back by sending the server's own `monthly_cap` with `hipc api PUT`, so the put-back does not depend on the field names under test), and the name of the first billing account with an id (skipped with only the Default, or when `hipc` would not send the name back as it is). Each change is in the cleanup ledger, with the value to put back, before it is sent, so the exit trap puts it back after a failure too.
- Each resource goes into a cleanup ledger before its create is sent, so a create that times out after the server acted is covered too. When the create exits 0, the ledger records the id it answered. A scenario's delete runs only after that run's own create exited 0 and answered an id. A line leaves the ledger only when that delete exited 0 and a complete list no longer shows the name. A list counts as complete when it names no next page and its `count`, if it has one, equals its rows.
- The exit trap runs after the last scenario, after a failure, and after Ctrl-C. For each line left in the ledger, it deletes by the recorded id, or looks the resource up by its name, which is unique to the run. It then lists again. Unless a complete list no longer shows the name, it fails the run and names the resource to delete by hand.
- After a SIGKILL no trap runs. The run prints the ledger's path at the start (`$TMPDIR/hipc-manual.XXXXXX/ledger`, one `kind name id state` line per resource). That file is then the list of what to delete by hand. A `runner-max`, `runner-cap`, or `billing-name` line is a change, not a resource: put it back from the value in `restore/<kind>.<id>` beside the ledger (for `runner-cap`, the body to send with `hipc api PUT /runners/limit/ --body-file`), and never delete what it names. Remove the directory afterwards, since it also holds the stored token.
- Before the first scenario, the script takes a snapshot of the account with read-only commands (the setup block below). After the last scenario it takes it again and diffs the two.
  - Lists are reduced to the `id`, `vm_id`, `name`, `subscription_id`, `plan_id`, `active`, and `cancelled_at` of each live row, sorted. A row that is revoked, archived, or being deleted is not live.
  - Other documents are reduced to `active`, `status`, `plan`, `plan_id`, `plan_code`, `mode`, `period`, `subscription_id`, `cancelled_at`, and `cancel_at_period_end`. The Drive subscription is kept whole, so the teardown's check that the plan is cancelled reads the full answer, and the diff compares those fields plus `plan_name`.
  - A VM's public IP is reduced to its address.
  - Any difference fails the run and is printed in the log. A snapshot read that fails, or a list that is not complete, also fails the run, except `network peers list` and `compute kubernetes clusters list` answering `not_found` (an account with no network yet, or no Kubernetes access), `network peers list` answering 403 "You must call /api/network/provision/ first." (a network never set up, recorded as `network_unprovisioned`), and `drive subscription get` answering `not_found`, which the snapshot records as that state; setting the network up would be a write that cannot be undone, so no tier does it.
  - The balance is compared on its own line. A VM or a metered service that runs during the test can move it, so a FAIL there says to check it by hand.
- In the read and write tiers, anything that spends credits or touches a subscription never runs for real: subscribing, changing or cancelling a plan, a top-up, a VM launch, an App Engine app launch (its secrets file holds a stand-in value, which the dry run must show only as a placeholder), a public IP, a database, a CDN zone, an invite, and a support ticket. Each one is shown as a `--dry-run`, as are the network summary, whose first read creates the account's network, and `network create`, and a kubeconfig fetch, which needs a cluster's key file. So is every write no tier can undo within the run, or that reaches another person or a live site: a member change, an invite answer, a project transfer, a billing account create (there is no delete for one), a namespace claim, a firewall, rule, or WAF change on an existing VM or zone, a restore, a node or node pool change, the runner settings other than the concurrency, and the auto-reload and billing profile settings.

**Offline.** `--offline` runs every command against `http://127.0.0.1:9`, where nothing listens, with a fake token. A read then fails with `unavailable`, and the expectations say so. Local commands, dry runs, and refusals behave as they do online. Checks that need a live answer are skipped. That is how the test suite runs the script without a network.

## The spend tier

`--tier spend` is for the live run before a release, by the owner of the account. It is never the default and `--tier write` never implies it. It runs every step of the write tier, and in four scenarios it also buys something and undoes it in the same run:

| Scenario | What it buys | What it costs | How the run undoes it |
| --- | --- | --- | --- |
| Drive | The Starter plan (`solo`), monthly, from the balance. It is asked about only when the read shows a free state: a `plan` or `plan_code` of `free`, `{"active": false}` with nothing else but `free_plan` and null plan fields, or `not_found`. Any other answer is logged and nothing is bought, as when the subscription cannot be read | One month of Starter: a cancel does not refund the period | `drive subscription cancel` (retried while it answers 425 `still_confirming`), then a read that shows it cancelled (a cancelled status, a cancel at the period's end, or a free state). Only a subscribe that exited 0 is ever cancelled, and a cancel that exited 0 is never sent again |
| Compute | A VM of the smallest available flavor, launched with `--wait`; on it a power policy, an exposure of port 8080 (nothing listens there; never SSH), a daily backup policy, and a reboot; then a firewall with no rule, which closes every inbound port, and a public IP, attached only once the exposure's delete and the firewall change exited 0 | The flavor's and the IP's hourly prices for the minutes they run | The exposure and the policy are removed as steps, the IP is released, then the VM is deleted, and its firewall with it. A launch that fails but prints the VM is deleted by that id |
| Compute | The smallest database (`starter`, size `s`, PostgreSQL 17); the run waits up to ten minutes for it to run | Its usage price for the minutes it runs | `compute databases delete`. The server also makes a backup bucket (`hippius-db-<id>-backups`) and a writer token (`db-<id>-backup-writer`) for the database and keeps both after the delete; the run reports them as a failed check of their own and lists them in `<log>.ledger` under "left by the server". It deletes neither: the owner decides |
| CDN | A new, empty bucket and a zone that serves it, capped at 1 credit a month; its settings and WAF mode are changed and its cache purged | At most 1 credit; nothing is uploaded, so nothing is served | The zone is deleted, then the bucket |
| Support | A real ticket, whose subject says it is a CLI release test and asks the team to close it, and one reply | No credits; a person on the support team reads it | `support tickets update <id> --status closed`, then a read that shows it closed; with a binary that has no close, the subject asks the team to close it |

Before each purchase the script prints what it buys and what it costs, and waits for `y` at the terminal; any other answer skips the purchase and every step that needs it. With stdin not a terminal, `--tier spend` is refused unless `--yes-spend` answers `y` for every purchase in advance. Each purchase is in the cleanup ledger before its create is sent, and the exit trap undoes whatever is left: it cancels the Drive plan, closes the ticket, releases the IP, and deletes the VM, database, zone, and bucket. The snapshot diff allows exactly the spend tier's own differences and nothing else: the balance may move, but only after a purchase's create exited 0; the Drive subscription may change, but only to a cancelled shape (a cancelled status, a cancel at the period's end, back on Free, or none at all); the tickets list may gain the tickets this run opened; and, when the run launched a VM, the network peers read may go from a network never set up to no peers, since the launch sets the network up. The database's backup bucket and writer token are reported apart, not as a difference. A ticket's status and close date are in the snapshot, so a ticket this run did not open that changes still fails the run. When a cleanup fails, or the server kept something it made for a purchase, the ledger is kept beside the log as `<log>.ledger`, the list of what may be left.

A purchase declined at the terminal still leaves the reads of what the account already has: the first VM, database, and zone are read as in the read tier.

## How to read a block

Each line is a step, tagged with what it does to the account:

| Tag | Meaning |
| --- | --- |
| `setup` | Installs the binary under test: `gh`, `cosign`, `shasum`, and `tar`. |
| `read` | Changes nothing: a GET, or the `cdn estimate` POST, which stores nothing. |
| `local` | Changes a file on this machine only: the hipc config or the AWS profile. |
| `dry-run` | Prints the request a write would send, and sends nothing. |
| `write` | Creates or changes a free resource on the server (write and spend tiers). |
| `undo` | Deletes, cancels, or closes what a `write` or `spend` step made. |
| `spend` | Buys something the run then undoes (spend tier only). |
| `ask` | The spend tier's question before a purchase, with its cost (spend tier only). |
| `error` | A refusal on purpose: the error code and the exit code are checked. |
| `ext` | Another program: `ssh-keygen`, `aws`, `docker`. |
| `check` | An assertion the script makes on what the last step left behind. |

`<bucket>`, `<token-name>`, `<hub-key>`, `<ssh-key>`, `<zone-name>`, `<project-name>`, `<vm-name>`, `<database-name>`, and `<ticket-tag>` are one name per run, `hipc-manual-<date>-<pid>`; `<cdn-bucket>` is `hipc-manual-cdn-<date>-<pid>`, `<namespace>` is `hipc-manual-<pid>`, and `<waf-list>` is `hipc_manual_<date>_<pid>`, cut to 32 characters. An id in angle brackets comes from the answer of an earlier step. A read of one resource (the first VM, zone, database, cluster, repository, or VPC) is skipped when the account has none; the dry runs that name it then use an id that names nothing, since a dry run sends nothing. The human scenarios pass `--output text`, which is the default at a terminal, because the script captures stdout into a file and `hipc` would otherwise print JSON. A step that reads a value for a later step passes `--json`.

## Personas

- **A new user** installs `hipc`, logs in with a token copied from the console, and looks around.
- **An account owner** watches the balance, the plans, and the bills, and manages the team.
- **A developer** keeps files in S3 and images in Hub, runs VMs, and serves a site through the CDN.
- **A pipeline** pushes images with a CI key and reads S3 with a scoped token.
- **A coding agent** drives `hipc` through JSON, learns it from `hipc schema`, and branches on error codes.

## Setup

The binary under test, then a login and the account snapshot. The snapshot's reads run quietly: the log shows the snapshot itself, not each answer. `compute public-ip get` runs once for each VM the VM list names.

```text
scenario setup: install hipc, log in for the snapshot, snapshot the account
  [setup] gh release view --repo thenervelab/hippius-cli --json tagName --jq .tagName
  [setup] gh release download --repo thenervelab/hippius-cli <tag> --pattern hipc-<target>.tar.gz --pattern SHA256SUMS --pattern SHA256SUMS.sigstore.json
  [check] SHA256SUMS lists the tarball
  [setup] cosign verify-blob SHA256SUMS --bundle SHA256SUMS.sigstore.json --certificate-identity https://github.com/thenervelab/hippius-cli/.github/workflows/release.yml@refs/tags/<tag> --certificate-oidc-issuer https://token.actions.githubusercontent.com
  [setup] shasum -a 256 -c SHA256SUMS.target
  [setup] tar -xzf hipc-<target>.tar.gz
  [setup] hipc --output text version
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [read] hipc --json s3 buckets list
  [read] hipc --json s3 tokens list
  [read] hipc --json compute ssh-keys list --all
  [read] hipc --json hub keys list
  [read] hipc --json hub repositories list
  [read] hipc --json cdn zones list
  [read] hipc --json cdn lists list
  [read] hipc --json compute vms list
  [read] hipc --json compute databases list
  [read] hipc --json compute kubernetes clusters list
  [read] hipc --json network peers list
  [read] hipc --json account invites list
  [read] hipc --json support tickets list --all
  [read] hipc --json drive subscription get
  [read] hipc --json s3 subscription get
  [read] hipc --json hub subscription get
  [read] hipc --json account projects list
  [read] hipc --json account billing-accounts list
  [read] hipc --json billing balance
  [read] hipc --json compute public-ip get <vm> (for each VM)
```

With `--hipc <path>`, the five install steps and their check are replaced by that binary. `<tag>` is the latest release's tag, `vX.Y.Z`; the signing identity names it, so the signed `SHA256SUMS` of another release does not pass.

## Scenarios

### 1. First run and login

A new user checks the installed binary, sees that nothing is logged in, logs in, checks the login, and logs out.

```text
scenario first-run: install, read the version and help, log in, check, log out
  [local] hipc --output text version
  [local] hipc --output text --help
  [error] hipc --json auth status
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [read] hipc --output text auth status
  [read] hipc --output text account list
  [read] hipc --output text status
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
  [error] hipc --json auth status
```

- Reads: `auth status`, `account list`, `status`. Writes: none on the server. Login and logout change only the config file in the temporary home.
- The user sees the version and target, the help, an `auth` error with a hint that names `hipc auth login --token-file <path>` before the login, the token's account and preview after it, the state of each product, and the same `auth` error after the logout.

### 2. Account and billing

An account owner reads the account, its members, and where the money goes.

```text
scenario account-billing: read the account, its members, the balance and the bills
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [read] hipc --output text account get
  [read] hipc --output text account notifications get
  [dry-run] hipc --output text account notifications update --low-credit-alerts true --dry-run
  [read] hipc --output text account marketing-consent get
  [dry-run] hipc --output text account marketing-consent update --granted false --dry-run
  [read] hipc --output text account members list
  [read] hipc --json account referrals list
  [read] hipc --output text account referrals analytics
  [dry-run] hipc --output text account referrals create --name hipc-manual --dry-run
  [dry-run] hipc --output text account referrals update <referral> --name hipc-manual --dry-run
  [read] hipc --output text billing balance
  [read] hipc --output text billing stripe
  [read] hipc --output text billing subscriptions
  [read] hipc --output text billing transactions --limit 5
  [read] hipc --output text billing summary
  [read] hipc --output text billing usage
  [read] hipc --json billing invoices list --limit 5
  [read] hipc --output text billing invoices get <invoice> --output-file ~/invoice.pdf
  [read] hipc --output text billing profile get
  [dry-run] hipc --output text billing profile update --city Paris --dry-run
  [read] hipc --output text billing auto-reload get
  [read] hipc --output text billing auto-reload history --limit 5
  [dry-run] hipc --output text billing auto-reload update --enabled false --dry-run
  [dry-run] hipc --output text billing auto-reload card-setup --dry-run
  [dry-run] hipc --output text billing top-up --amount 10 --method card --dry-run
  [read] hipc --output text billing native-quote
  [dry-run] hipc --output text billing top-up --method halpha --alpha 1 --dry-run
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
```

- Reads: every step but the dry runs, including the marketing consent, the referral links and their analytics, the legacy Stripe plan, and the first invoice's PDF, saved in the temporary home (skipped when there is no invoice). Writes, all dry runs in every tier: a notification change and a consent withdrawal (each reaches the owner's email), a referral link create or rename (a link has no delete, and the first create mints the code for good), a billing profile change, auto-reload off, a card setup (a Stripe checkout), and a top-up, which spends money.
- The user sees the account, the notification settings and consent, the members, the balance and runway, the plans, the last five transactions and invoices, an invoice file, the name and address the invoices carry, the month's summary and usage, the auto-reload settings and history, and the request each change would send.

### 3. Drive

A user compares the Drive plans and checks the subscription.

```text
scenario drive: read the Drive plans and subscription; plan changes are dry runs
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [read] hipc --output text drive plans
  [read] hipc --output text drive subscription get
  [read] hipc --output text drive subscription history --limit 5
  [read] hipc --json drive subscription checkout 0f8fad5b-d9cb-469f-a165-70867728950e
  [dry-run] hipc --output text drive subscription subscribe --plan solo --dry-run
  [dry-run] hipc --output text drive subscription change --plan duo --dry-run
  [dry-run] hipc --output text drive subscription cancel --dry-run
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
```

- Reads: the plans, the subscription, its history, and a card checkout by an id that names none, which may answer `not_found`, `permission`, or `owner_only`. Writes: subscribe, change, and cancel are dry runs in the read and write tiers. They are paid, and a cancel does not refund the period. The spend tier buys Starter and cancels it (see [The spend tier](#the-spend-tier)).
- The user sees the plan codes and prices, the plan in use (or none), and the exact request each plan change would send.

### 4. S3, with an AWS profile

A developer lists buckets, the first objects of one, and tokens. In the write tier they create a bucket, a sub token scoped to it, and an AWS profile from that token, list the bucket with the AWS CLI, and delete everything again.

```text
scenario s3: buckets, tokens and an AWS profile; the write tier creates and deletes
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [read] hipc --output text s3 plans
  [read] hipc --output text s3 subscription get
  [read] hipc --output text s3 buckets list
  [read] hipc --json s3 buckets list
  [read] hipc --output text s3 buckets objects list <first-bucket> --limit 10
  [read] hipc --output text s3 buckets acl list
  [read] hipc --output text s3 tokens list
  [read] hipc --output text s3 subscription history --limit 5
  [read] hipc --json s3 subscription checkout 0f8fad5b-d9cb-469f-a165-70867728950e
  [dry-run] hipc --output text s3 subscription subscribe --plan pro --dry-run
  [dry-run] hipc --output text s3 subscription change --plan business --yes --dry-run
  [dry-run] hipc --output text s3 subscription cancel --yes --dry-run
  [write] hipc --output text s3 buckets create <bucket> --yes
  [read] hipc --json s3 buckets list
  [check] the list shows the new bucket
  [read] hipc --output text s3 buckets acl get <bucket>
  [write] hipc --output text s3 buckets acl update <bucket> --policy-file ~/policy.json --public --remove-others --yes
  [read] hipc --json s3 buckets acl get <bucket>
  [check] the policy is the deny statement
  [read] hipc --json s3 buckets objects presign <bucket> hipc-manual.txt --expires-in 60 --reveal
  [write] hipc --json s3 tokens create --kind sub --name <token-name> --bucket <bucket> --reveal --yes > ~/s3-token.json
  [read] hipc --json s3 tokens list
  [check] the list shows the new token
  [local] hipc --output text s3 configure --from-token-file ~/s3-token.json --aws-profile hipc-manual
  [ext] aws --profile hipc-manual s3 ls s3://<bucket>
  [write] hipc --json s3 tokens rotate --kind sub <token-id> --reveal --yes
  [undo] hipc --output text s3 tokens revoke --kind sub <token-id> --yes
  [read] hipc --json s3 tokens list
  [check] the list no longer shows the token as live
  [undo] hipc --output text s3 buckets delete <bucket> --yes
  [read] hipc --json s3 buckets list
  [check] the list no longer shows the bucket
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
```

- Reads: the plans, the subscription, the buckets, the first page of the first bucket's objects (skipped when the account has no bucket; the listing moves no bytes, and copying an object is `aws s3 cp` with the profile `hipc s3 configure` writes), the ACLs, the tokens, the subscription history, and a card checkout by an id that names none. Writes (write tier): the bucket, given a policy with no statement and a presigned URL for a key in it (masked), undone by `s3 buckets delete`, and the sub token, rotated once, undone by `s3 tokens revoke`. The token is revoked first, because it is scoped to the bucket. A plan subscribe, change, and cancel are dry runs. The AWS profile lives in the temporary home and is removed with it.
- `aws s3 ls` runs only when the AWS CLI is installed, and never offline.
- In the read tier, the steps from the bucket create to the last check are replaced by five dry runs: the bucket create, its policy, a presigned URL, the sub token create, and a rotation.
- The user sees the keys, sizes, and dates of a bucket's first objects, the new bucket and token in the lists, a private ACL, the profile `hipc s3 configure` wrote, an empty listing from the AWS CLI, and then neither resource in the lists. The token's secret goes to `~/s3-token.json`, which the script's `umask 077` makes mode 0600 inside the temporary home that is removed on exit, and is masked in the transcript. Outside the script, keep a secret off stdout with `--secret-file` (see [agents.md](agents.md#secrets)), or redirect under `umask 077` and delete the file after use.

### 5. Hub, with a CI key

A developer reads the registry. In the write tier a pipeline's CI key is created, used for `docker login` when asked, and deleted.

```text
scenario hub: read the registry; the write tier creates and deletes a CI key
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [read] hipc --output text hub plans
  [read] hipc --json hub subscription get
  [dry-run] hipc --output text hub subscription cancel <subscription> --yes --dry-run
  [read] hipc --output text hub namespace status
  [read] hipc --output text hub namespace availability <namespace>
  [dry-run] hipc --output text hub namespace create <namespace> --reveal --yes --dry-run
  [read] hipc --json hub namespace get
  [read] hipc --json hub namespace robot get
  [dry-run] hipc --output text hub namespace update --visibility private --yes --dry-run
  [dry-run] hipc --output text hub namespace robot rotate --reveal --yes --dry-run
  [read] hipc --json hub events list
  [read] hipc --json hub usage
  [read] hipc --json hub repositories list
  [read] hipc --output text hub repositories get <repository>
  [read] hipc --json hub artifacts list <repository>
  [read] hipc --output text hub artifacts get <repository> <reference>
  [dry-run] hipc --output text hub artifacts delete <repository> <reference> --yes --dry-run
  [dry-run] hipc --output text hub repositories delete <repository> --yes --dry-run
  [read] hipc --output text hub keys list
  [read] hipc --json hub models list --limit 5
  [read] hipc --output text hub models get <model> --limit 5
  [read] hipc --output text hub models formats
  [dry-run] hipc --output text hub subscription subscribe --plan Builder --dry-run
  [write] hipc --json hub keys create --name <hub-key> --role read --expires-in 1 --reveal --yes > ~/hub-key.json
  [read] hipc --json hub keys list
  [check] the list shows the new key
  [read] hipc --output text hub keys get <hub-key-id>
  [ext] docker login registry.hippius.com -u <login> --password-stdin < (the key's secret, on stdin)
  [ext] docker logout registry.hippius.com
  [write] hipc --json hub keys rotate <hub-key-id> --reveal --yes
  [undo] hipc --output text hub keys delete <hub-key-id> --yes
  [read] hipc --json hub keys list
  [check] the list no longer shows the key
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
```

- Reads: the plans, the subscription, the namespace, whether a new namespace name is free, the robot login, the usage, the repositories, the registry events, the first repository and its first artifact (skipped when there is none), the keys, and the model index, its formats, and the first model. The namespace, robot, events, and usage reads accept `not_found`, for an account with no namespace. Writes (write tier): the CI key, read-only and expiring after a day, read back, rotated once, undone by `hub keys delete`. Making the namespace public or private, rotating the robot secret, subscribing to a plan, cancelling it, claiming a namespace, and deleting an artifact or a repository are dry runs only.
- `docker login` runs only with `--docker`, when Docker is installed, and never offline. It reads the key's secret from stdin and stores the login in the temporary `DOCKER_CONFIG`, and `docker logout` removes it.
- In the read tier, the steps from the key create to the last check are replaced by two dry runs, the key create and a rotation.
- The user sees the new key in the list, its login, and then no key. The secret goes to `~/hub-key.json`, mode 0600 under the script's `umask 077`, and is masked in the transcript. Outside the script, `hipc hub keys create --secret-file` writes the secret alone to a new private file, which `docker login --password-stdin` reads.

### 6. Compute, with an SSH key

A developer checks prices and capacity before a launch, and manages SSH keys.

```text
scenario compute: prices and capacity, VMs, databases, runners and SSH keys; a launch is a dry run below spend
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [read] hipc --output text compute regions
  [read] hipc --json compute flavors
  [read] hipc --output text compute prices
  [read] hipc --output text compute public-prices
  [read] hipc --output text compute quota
  [read] hipc --output text compute charge-cap
  [read] hipc --output text compute images
  [read] hipc --output text compute apps
  [read] hipc --output text compute placement-groups
  [read] hipc --output text compute backups options
  [read] hipc --output text compute feasibility --flavor <flavor>
  [read] hipc --output text compute vms list
  [read] hipc --output text compute ssh-keys list --all
  [read] hipc --output text compute account-ssh-keys
  [read] hipc --output text compute public-ip availability
  [ext] ssh-keygen -q -t ed25519 -N '' -C hipc-manual -f ~/hipc-manual-key
  [dry-run] hipc --output text compute vms create --flavor <flavor> --name hipc-manual --ssh-public-key-file ~/hipc-manual-key.pub --dry-run
  [dry-run] hipc --output text compute public-ip create vm_example --dry-run
  [write] hipc --json compute ssh-keys create --name <ssh-key> --public-key-file ~/hipc-manual-key.pub --yes
  [read] hipc --json compute ssh-keys list --all
  [check] the list shows the new key
  [read] hipc --output text compute ssh-keys get <ssh-key-id>
  [undo] hipc --output text compute ssh-keys delete <ssh-key-id> --yes
  [read] hipc --json compute ssh-keys list --all
  [check] the list no longer shows the key
  [read] hipc --json compute vms list
  [read] hipc --output text compute vms get <vm>
  [read] hipc --json compute vms attestation <vm>
  [read] hipc --json compute vms failovers <vm>
  [read] hipc --output text compute vms resize-options <vm>
  [read] hipc --json compute vms resize-status <vm> 999999999
  [read] hipc --output text compute vms power-policy get <vm>
  [read] hipc --json compute public-ip usage <vm> --range 24h
  [read] hipc --output text compute firewall get <vm>
  [read] hipc --output text compute exposures list <vm>
  [read] hipc --json compute backups policy get <vm>
  [read] hipc --json compute backups list <vm>
  [read] hipc --json compute backups restore-status <vm>
  [dry-run] hipc --output text compute vms stop <vm> --yes --dry-run
  [dry-run] hipc --output text compute vms start <vm> --dry-run
  [dry-run] hipc --output text compute vms reboot <vm> --yes --dry-run
  [dry-run] hipc --output text compute vms resize <vm> --flavor <flavor> --yes --dry-run
  [dry-run] hipc --output text compute vms power-policy update <vm> --on-guest-poweroff stop --dry-run
  [dry-run] hipc --output text compute firewall update <vm> --rules-file ~/firewall.json --remove-others --yes --dry-run
  [dry-run] hipc --output text compute exposures create <vm> --port 8080 --mode http --yes --dry-run
  [dry-run] hipc --output text compute exposures delete <vm> 999999999 --yes --dry-run
  [dry-run] hipc --output text compute backups policy update <vm> --interval 86400 --yes --dry-run
  [dry-run] hipc --output text compute backups policy delete <vm> --yes --dry-run
  [dry-run] hipc --output text compute backups restore <vm> --run-id 0f8fad5b-d9cb-469f-a165-70867728950e --accept-rollback --yes --dry-run
  [dry-run] hipc --output text compute public-ip delete <vm> --yes --dry-run
  [dry-run] hipc --output text compute vms delete <vm> --yes --dry-run
  [read] hipc --json compute databases list
  [read] hipc --output text compute databases refusals
  [read] hipc --output text compute databases get <database>
  [read] hipc --output text compute databases events <database>
  [read] hipc --output text compute databases metrics <database>
  [dry-run] hipc --output text compute databases create --tier starter --size s --engine-version 17 --reveal --yes --dry-run
  [dry-run] hipc --output text compute databases restart <database> --yes --dry-run
  [dry-run] hipc --output text compute databases restore <database> --reveal --yes --dry-run
  [dry-run] hipc --output text compute databases exposure create <database> --yes --dry-run
  [dry-run] hipc --output text compute databases exposure delete <database> --yes --dry-run
  [dry-run] hipc --output text compute databases delete <database> --yes --dry-run
  [read] hipc --json compute runners settings get
  [write] hipc --json compute runners settings update --max-concurrent <max-concurrent-1> --yes
  [read] hipc --json compute runners settings get
  [check] max_concurrent is the lower value
  [undo] hipc --json compute runners settings update --max-concurrent <max-concurrent> --yes
  [read] hipc --json compute runners settings get
  [check] max_concurrent is back
  [read] hipc --json compute runners installations list
  [dry-run] hipc --output text compute runners installations delete <installation> --yes --dry-run
  [dry-run] hipc --output text compute runners connect --dry-run
  [read] hipc --output text compute runners jobs
  [read] hipc --output text compute runners usage
  [read] hipc --json compute runners limit get
  [write] hipc --json compute runners limit update --monthly-cap <cap+1> --yes
  [read] hipc --json compute runners limit get
  [check] the cap is the new one
  [undo] hipc --json api PUT /runners/limit/ --body-file ~/runner-limit.json --yes
  [read] hipc --json compute runners limit get
  [check] the cap is back
  [read] hipc --output text compute app-engine get
  [read] hipc --json compute app-engine apps list
  [read] hipc --output text compute app-engine apps get <app>
  [read] hipc --output text compute app-engine apps runtime <app>
  [read] hipc --output text compute app-engine guest-components get <app>
  [read] hipc --output text compute app-engine guest-components update-status <app>
  [read] hipc --json compute app-engine validate --compose-file ~/compose.yml
  [read] hipc --json compute app-engine fix --compose-file ~/compose.yml --output-file ~/compose-fixed.yml
  [dry-run] hipc --json compute app-engine apps create --flavor small --compose-file ~/compose.yml --secrets-file ~/app-secrets.json --name hipc-manual --dry-run
  [check] the dry run shows a placeholder for the secret
  [check] the dry run does not show the secret
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
```

- Reads: the regions, flavors, prices, quota, charge cap, images, apps, placement groups, backup options, a launch's feasibility for the smallest flavor, the VMs, the SSH keys, and public IP availability. Then the first VM's state, attestation, failovers, resize options, power policy, IP usage, firewall, exposures, backup policy, backups, and restore (skipped when there is no VM; the ones a VM may lack accept `not_found`), the databases and their refusals, the first database's state, events, and metrics (skipped when there is none), the runner settings, installations, jobs, usage, and limit (`not_found` is accepted: a server may not offer the limit), and App Engine: whether the account may use it, its apps, the first app's compose, runtime, and guest components (skipped when there is none), and a check and a fix of a one-service compose, which launch nothing (`permission` and `validation` are accepted). Writes (write tier): the SSH key, read back, then undone by `compute ssh-keys delete`. Every VM and database write, and every runner write but two, is a dry run below the spend tier: a launch, a public IP, and a database spend credits, and the rest would change a running VM or database, or the runner bill. The two are the write tier's runner concurrency, lowered by one and put back, and an existing runner spend cap, raised by one credit and put back, each read back after every write. The key pair is made by `ssh-keygen` in the temporary home.
- In the spend tier, a new VM and a new database replace the first ones for the reads, and are deleted at the end (see [The spend tier](#the-spend-tier)).
- In the read tier, the steps from the key create to the last check are replaced by `hipc --output text compute ssh-keys create --name <ssh-key> --public-key-file ~/hipc-manual-key.pub --dry-run`.
- The user sees the price and capacity tables, a feasibility verdict, the launch request with the public key in it, the key in the list and then gone, and the request each VM, database, and runner change would send.

### 7. CDN

A developer prices a site and reads the zones and their firewall.

```text
scenario cdn: prices, an estimate, zones, hostnames and WAF; a zone is created only in the spend tier
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [read] hipc --output text cdn prices
  [read] hipc --output text cdn regions
  [read] hipc --output text cdn estimate --traffic FR=100 --requests 1000000
  [read] hipc --json cdn zones list
  [read] hipc --json cdn lists list
  [read] hipc --output text cdn lists get <waf-list>
  [dry-run] hipc --output text cdn zones create --name <zone-name> --bucket <bucket> --dry-run
  [read] hipc --output text cdn zones get <zone>
  [read] hipc --output text cdn cap get <zone>
  [read] hipc --output text cdn hostnames list <zone>
  [read] hipc --output text cdn rules get <zone>
  [read] hipc --output text cdn stats <zone>
  [read] hipc --output text cdn waf get <zone>
  [read] hipc --output text cdn waf status <zone>
  [read] hipc --output text cdn waf analytics <zone>
  [read] hipc --output text cdn waf events <zone>
  [read] hipc --output text cdn waf rules get <zone>
  [read] hipc --output text cdn waf rate-limits get <zone>
  [read] hipc --json cdn waf test <zone> --request-file ~/waf-request.json
  [read] hipc --json cdn waf versions list <zone>
  [read] hipc --output text cdn waf versions get <zone> <version>
  [read] hipc --json cdn purges list <zone>
  [read] hipc --output text cdn purges get <zone> <purge>
  [read] hipc --json cdn hostnames domain-connect <zone> hipc-manual.example.com
  [dry-run] hipc --output text cdn zones update <zone> --redirect-https true --yes --dry-run
  [dry-run] hipc --output text cdn cap update <zone> --monthly-cap 1 --yes --dry-run
  [dry-run] hipc --output text cdn hostnames create <zone> hipc-manual.example.com --yes --dry-run
  [dry-run] hipc --output text cdn hostnames verify <zone> hipc-manual.example.com --dry-run
  [dry-run] hipc --output text cdn hostnames dns-setup <zone> hipc-manual.example.com --provider cloudflare --credentials-file ~/dns-credentials.json --yes --dry-run
  [dry-run] hipc --output text cdn hostnames delete <zone> hipc-manual.example.com --yes --dry-run
  [dry-run] hipc --output text cdn purges create <zone> --everything --dry-run
  [dry-run] hipc --output text cdn rules update <zone> --rules-file ~/rules.json --remove-others --yes --dry-run
  [dry-run] hipc --output text cdn waf update <zone> --mode log --yes --dry-run
  [dry-run] hipc --output text cdn waf rules update <zone> --rules-file ~/rules.json --remove-others --yes --dry-run
  [dry-run] hipc --output text cdn waf rate-limits update <zone> --rules-file ~/rules.json --remove-others --yes --dry-run
  [dry-run] hipc --output text cdn waf versions rollback <zone> 1 --yes --dry-run
  [dry-run] hipc --output text cdn zones delete <zone> --yes --dry-run
  [write] hipc --output text cdn lists create <waf-list> --kind ip --items-file ~/waf-items.txt
  [write] hipc --output text cdn lists update <waf-list> --description 'hipc manual test'
  [write] hipc --output text cdn lists items create <waf-list> --items-file ~/waf-items-2.txt --yes
  [write] hipc --output text cdn lists items update <waf-list> --items-file ~/waf-items.txt --remove-others --yes
  [write] hipc --output text cdn lists items delete <waf-list> --items-file ~/waf-items.txt --yes
  [read] hipc --json cdn lists get <waf-list>
  [undo] hipc --output text cdn lists delete <waf-list> --yes
  [read] hipc --json cdn lists list
  [check] the list no longer shows the WAF list
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
```

- Reads: the prices, regions, an estimate (a POST that stores nothing), the zones, the WAF lists and the first one, and for the first zone (skipped when the account has none) its settings, cap, hostnames, rules, traffic, WAF settings, status, analytics, events, rules, rate limits, versions and the first version, purges and the first purge, a WAF test (which needs CDN operator access, so `permission` is accepted), and a Domain Connect link for an example hostname (`not_found` is accepted). Writes: every zone write is a dry run below the spend tier, a zone create included, since it is billed for traffic; so are the hostname writes, which need a domain the owner holds (`dns-setup` reads a stand-in credential file, which is masked). The write tier makes a WAF list, describes it, adds, replaces, and removes items, and deletes it; the read tier shows those as dry runs.
- The user sees the price list, the cost of 100 GB served in France and a million requests, the zones, the zone request with its S3 origin, and a zone's settings, traffic, and firewall.

### 8. Private network and Kubernetes

A developer reads the private network and the managed Kubernetes clusters. The network summary and a kubeconfig fetch are shown, not sent.

This journey expects an account that already has a private network and Kubernetes access. On an account whose private network was never set up, every network list answers 403 `permission` ("You must call /api/network/provision/ first."; a newer `hipc` says the network is not set up and names the command that sets it up). The script records that as the state `network_unprovisioned`, with a note, and an account without Kubernetes access answers the cluster list `not_found`; the steps and the snapshot accept both, so the run goes on. It is the end of this journey for that account, not a prompt to run `hipc network create --yes`, which creates the network for good; no tier sends it except as a dry run.

```text
scenario network-kubernetes: the private network and Kubernetes; the network summary and a kubeconfig are dry runs
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [dry-run] hipc --output text network get --dry-run
  [dry-run] hipc --output text network create --yes --dry-run
  [read] hipc --json network peers list
  [read] hipc --json network peers get <peer>
  [dry-run] hipc --output text network peers update <peer> --name hipc-manual --yes --dry-run
  [dry-run] hipc --output text network peers delete <peer> --yes --dry-run
  [read] hipc --json network groups list
  [read] hipc --json network policies list
  [read] hipc --json network vpcs list
  [read] hipc --output text network vpcs resources list <vpc>
  [read] hipc --output text network vpcs routers list <vpc>
  [read] hipc --json compute kubernetes clusters list
  [read] hipc --output text compute kubernetes clusters get <cluster>
  [read] hipc --output text compute kubernetes clusters events <cluster>
  [read] hipc --json compute kubernetes backups list <cluster> --reveal
  [read] hipc --json compute kubernetes node-pools list <cluster>
  [read] hipc --output text compute kubernetes operations list <cluster>
  [read] hipc --json compute kubernetes clusters refusals
  [check] the key file is in this scenario's home
  [dry-run] hipc --output text compute kubernetes node-pools create <cluster> --name hipc-manual --flavor small --dry-run
  [dry-run] hipc --output text compute kubernetes node-pools scale <cluster> 999999999 --count 1 --cluster-key-file ~/hippius-cluster-manual.key --yes --dry-run
  [dry-run] hipc --output text compute kubernetes operations cancel <cluster> 999999999 --yes --dry-run
  [dry-run] hipc --output text compute kubernetes operations resume <cluster> 999999999 --yes --dry-run
  [dry-run] hipc --output text compute kubernetes nodes replace <cluster> hipc-manual-node --cluster-key-file ~/hippius-cluster-manual.key --yes --dry-run
  [dry-run] hipc --output text compute kubernetes nodes delete <cluster> hipc-manual-node --yes --dry-run
  [dry-run] hipc --output text compute kubernetes clusters delete <cluster> --yes --dry-run
  [dry-run] hipc --output text compute kubernetes clusters kubeconfig <cluster> --cluster-key-file ~/hippius-cluster-manual.key --output-file ~/kubeconfig --dry-run
  [check] the dry run wrote no kubeconfig
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
```

- Reads: the peers and the first one, the peer groups, the network policies, the VPC networks and the first one's resources and routers, the clusters, the first cluster's state, events, backups (with `--reveal`; the download links are masked), node pools, and operations, and the launches refused in the last 7 days. The list reads accept `not_found`, for an account with no network yet or no Kubernetes access, and the network lists accept the 403 of a network never set up (`network_unprovisioned`), with a note in the log, and a read of the first peer, VPC, or cluster is skipped when there is none. Writes: none, in every tier; a peer rename (a peer's name is its DNS name in the network, which a person or a service may be using) or removal, a node pool create or scale, an operation cancel or resume, a node replace or delete, and a cluster delete are dry runs, and the node writes read the test key file. `network get` is a dry run because the first one for an account creates its network, and it would show in the snapshot diff. The kubeconfig fetch is a dry run because it needs the cluster's key file, which no test holds. `hipc` reads the key file before it decides to send nothing, so the script writes one with the console's public test vector (never a real key) into the temporary home, with mode 0600, and checks that it is under that home. The key's seeds are in the log mask from the start, and the teardown scans every step's raw stdout and stderr for them. `<cluster>` is the first cluster's id, or an id that names none when the account has no cluster, since a dry run sends nothing.
- The user sees the one GET the summary would send, the peers and networks, the clusters and refusals, the two reads a kubeconfig needs, and no kubeconfig file written.

### 9. Team, projects, and billing accounts

An account owner reviews the team and picks a project and a billing account, then resets both.

```text
scenario team: members, invites, projects and billing accounts; pick one, then reset
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [read] hipc --json account members list
  [dry-run] hipc --output text account members update <member> --manage-team false --yes --dry-run
  [dry-run] hipc --output text account members delete <member> --yes --dry-run
  [read] hipc --json account invites list
  [dry-run] hipc --output text account invites create --email hipc-manual@example.com --dry-run
  [dry-run] hipc --output text account invites revoke <invite> --yes --dry-run
  [dry-run] hipc --output text account invites preview --token-file ~/invite.token --yes --dry-run
  [dry-run] hipc --output text account invites accept --token-file ~/invite.token --yes --dry-run
  [read] hipc --json account projects list
  [dry-run] hipc --output text account projects create --name hipc-manual --dry-run
  [read] hipc --output text account projects budget get <project>
  [read] hipc --output text account projects quota get <project>
  [write] hipc --json account projects create --name <project-name>
  [write] hipc --output text account projects update <new-project> --name <project-name>-renamed
  [write] hipc --output text account projects budget update <new-project> --monthly-limit 1 --alert-at 80 --block-launches false --yes
  [read] hipc --output text account projects budget get <new-project>
  [undo] hipc --output text account projects budget delete <new-project> --yes
  [write] hipc --output text account projects quota update <new-project> --vms 1 --yes
  [undo] hipc --output text account projects delete <new-project> --yes
  [read] hipc --json account projects list
  [check] the list no longer shows the project as live
  [dry-run] hipc --output text account projects transfer <project> --to-account acct_00000000000000000000 --yes --dry-run
  [read] hipc --output text account project-transfers list
  [dry-run] hipc --output text account project-transfers accept 999999999 --yes --dry-run
  [dry-run] hipc --output text account project-transfers decline 999999999 --yes --dry-run
  [dry-run] hipc --output text account project-transfers cancel 999999999 --yes --dry-run
  [read] hipc --json account billing-accounts list
  [read] hipc --output text account billing-accounts costs <billing-account> --group-by project
  [dry-run] hipc --output text account billing-accounts create --name hipc-manual --dry-run
  [write] hipc --json account billing-accounts update <billing-account> --name <new-name>
  [read] hipc --json account billing-accounts list
  [check] the billing account has the new name
  [undo] hipc --json account billing-accounts update <billing-account> --name <billing-name>
  [read] hipc --json account billing-accounts list
  [check] the billing account has its old name
  [local] hipc --output text account projects use <project>
  [local] hipc --output text account projects use default
  [local] hipc --output text account billing-accounts use <billing-account>
  [local] hipc --output text account billing-accounts use default
  [local] hipc --output text account use self
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
```

- Reads: the members, invites, projects, the first named project's budget and quota, the project transfers, the billing accounts, and the first named billing account's costs. The budget, quota, and costs reads are skipped when the account has nothing but the Default, which has no id they take. Writes (write tier): a project, renamed, given a budget (then removed) and a quota, then archived by `account projects delete`; the ledger follows the rename. Dry runs in every tier: an invite, which emails a real address, a member change or removal, an invite revoke, preview, or accept (by a stand-in invite token, masked), a project transfer and the answers to one, and a billing account create, since a billing account has no delete. The write tier renames the first billing account with an id and renames it back, reading the list after each, unless `hipc` would not send its name back as it is; the read tier shows the rename as a dry run. The `use` steps change only the profile in the temporary home, and the last three reset it. With no project or billing account but the Default, the `use` steps pick `default`.
- The user sees who is in the team, pending invites, the invite and project requests, a project's budget and limits, the transfers offered to or by the account, and the profile after each `use`.

### 10. Support

A user reads their tickets and their messages. Opening, answering, or closing one is shown, not sent, below the spend tier, because a person on the support team would read it.

```text
scenario support: read the tickets and their messages; opening or closing one is a dry run below spend
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [read] hipc --json support tickets list --limit 5
  [read] hipc --output text support tickets get <ticket>
  [read] hipc --output text support tickets messages list <ticket> --limit 5
  [dry-run] hipc --output text support tickets create --subject 'hipc manual test' --body-file ~/ticket.txt --dry-run
  [dry-run] hipc --output text support tickets update <ticket> --status closed --dry-run
  [dry-run] hipc --output text support tickets reply <ticket> --body-file ~/ticket.txt --dry-run
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
```

- Reads: the last five tickets, and the first one and its messages (skipped when there is none). Writes: a new ticket, a reply, and a close are dry runs below the spend tier, which opens a real ticket, replies, and closes it (see [The spend tier](#the-spend-tier)).
- The user sees the tickets, one ticket's thread, and the request a new ticket and a reply would send.

### 11. Agent and automation

A coding agent learns `hipc` from its schema, reads JSON, and branches on error codes.

```text
scenario agent: JSON output, the schema, --all paging and the error codes
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [local] hipc schema
  [check] the schema lists the commands and the error codes
  [read] AI_AGENT=1 hipc compute vms list
  [check] the answer is JSON
  [read] hipc --json compute ssh-keys list --all
  [check] the answer is one array
  [read] hipc --json api GET /api/services/status/
  [check] the answer is JSON
  [error] hipc --json api DELETE /api/objectstore/buckets/<bucket>/
  [error] HIPPIUS_TOKEN=hipc-manual-test-bad-token hipc --json auth status
  [error] hipc --json s3 buckets delete <bucket>
  [error] hipc --json s3 tokens create --kind sub --bucket <bucket> --yes
  [error] hipc --json compute vms list --bogus
  [dry-run] hipc --json s3 buckets create <bucket> --dry-run
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
```

- Reads: the VM list in JSON through `AI_AGENT=1`, every page of the SSH keys through `--all`, and `GET /api/services/status/` through `hipc api`. Writes: none. The deletes (one through `hipc api DELETE`) and the create are refused before anything is sent, and the bucket create is a dry run.
- The agent sees the schema with every command and error code, JSON on stdout without a flag, one array for `--all`, and one JSON error object on stderr for each failure: `auth` (exit 1) for a bad token, `confirmation_required` (exit 2) without `--yes`, `reveal_required` (exit 2) without `--reveal`, and `usage` (exit 2) for a flag that does not exist.

### 12. Token hygiene

Where the token goes, and where it never goes.

```text
scenario token-hygiene: where the token goes, and where it never goes
  [local] hipc --output text auth login --token-file - < (the token, on stdin)
  [check] only the owner can read the config file
  [read] hipc --json auth status
  [check] stdout and stderr do not hold the token
  [read] hipc --debug --output text account list
  [check] stdout and stderr do not hold the token
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
  [error] HIPPIUS_TOKEN=hipc-manual-test-bad-token hipc --json account list
  [check] the config file does not hold the bad token
```

- Reads: `auth status` and `account list`, the latter with `--debug`. Writes: none on the server.
- The user sees a config file only they can read, a status with a four-character preview (masked in the log too), `--debug` request lines with no header, no token after the logout, and an `auth` error for a bad `HIPPIUS_TOKEN` that is not stored anywhere. The log scan in the teardown checks the whole transcript.

## Teardown

After the last scenario, or after a failure or Ctrl-C, the exit trap works off the ledger, takes the snapshot again, diffs it with the first one, compares the balance, logs out, and scans the log.

```text
scenario teardown: work off the ledger, snapshot again, diff, log out, scan the log
  [check] every ledger entry is deleted
  [read] hipc --json s3 buckets list
  [read] hipc --json s3 tokens list
  [read] hipc --json compute ssh-keys list --all
  [read] hipc --json hub keys list
  [read] hipc --json hub repositories list
  [read] hipc --json cdn zones list
  [read] hipc --json cdn lists list
  [read] hipc --json compute vms list
  [read] hipc --json compute databases list
  [read] hipc --json compute kubernetes clusters list
  [read] hipc --json network peers list
  [read] hipc --json account invites list
  [read] hipc --json support tickets list --all
  [read] hipc --json drive subscription get
  [read] hipc --json s3 subscription get
  [read] hipc --json hub subscription get
  [read] hipc --json account projects list
  [read] hipc --json account billing-accounts list
  [read] hipc --json billing balance
  [read] hipc --json compute public-ip get <vm> (for each VM)
  [check] the account is as it was found
  [check] the balance is unchanged
  [local] hipc --output text auth logout --yes
  [check] the config file no longer holds the token
  [check] the log holds neither the token nor a revealed secret
  [check] no step output holds the cluster key file's secrets
```
