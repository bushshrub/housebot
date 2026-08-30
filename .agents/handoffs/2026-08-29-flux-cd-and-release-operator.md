# Continuous delivery on Flux, with an in-cluster release operator

Date: 2026-08-29
Branch: `claude/k8s-deployment-testing-fjobnz` (head `24ea1fe`)

## Why this session existed

The previous handoff left a Helm chart that had been proven against a live k3d
cluster and a CI pipeline publishing it to GHCR as an OCI artifact. What was
missing was delivery: how a merge to master reaches the cluster, and how a bad
release gets rolled back.

This session produced **no code** — it is a design decision, recorded here so
the next session can implement it. Nothing below has been built.

## The decision

Deploy with Flux. Keep versioning out of git entirely: a small Kubernetes
operator owns which version is running, and the Discord deployment bot becomes
a thin frontend that patches one field on a custom resource.

Three alternatives were considered and rejected, in this order:

1. **Push-based `helm upgrade` from GitHub Actions.** Fights Flux's
   reconciliation, needs a cluster credential in a public repo's CI, and gives
   no rollback ledger.
2. **Git-pinned chart version, bumped by CI.** Rollback is `git revert`. This
   is the conventional answer and it works, but it puts a deploy commit in the
   GitOps repo for every merge, which is exactly the churn the user wanted to
   avoid.
3. **An `OCIRepository` carved out of git and patched directly.** Cheap — no
   new component — but a bare field with no memory: no history, no "what was
   the previous version", nothing to roll back *to*.

The operator is (3) plus a ledger.

## Architecture

### The registry is the source of truth

The operator polls GHCR for chart versions rather than being told about them by
CI. There is no webhook and no cluster credential in CI, in either direction.

This is what makes a cluster rebuild well-defined: on an empty etcd the
operator queries the registry, finds the newest version on the track, and
deploys it. Losing `status` costs history, not the ability to run.

### The custom resource

```yaml
apiVersion: housebot.dev/v1alpha1
kind: Release
metadata:
  name: housebot
spec:
  track: "0.1.0-sha.*"      # follow master builds
  version: ""               # explicit pin; when set, overrides track
status:
  current: 0.1.0-sha.8cf170e
  pinned: false
  history:                  # bounded, ~10 entries, newest first
    - {version: 0.1.0-sha.4b21f0a, at: "2026-08-28T19:02:11Z", by: "auto"}
  helmRelease: {ready: true, message: "..."}
```

`housebot.dev` is already the label domain the chart uses
(`housebot.dev/gvisor` in `values.yaml`), so the group is consistent.

### The pin/track interlock

**This is the detail that makes or breaks rollback.** Rolling back must set
`spec.version`, which suspends track-following. Otherwise the operator sees the
newer, broken chart still sitting in the registry and immediately redeploys it
— the rollback undoes itself within one poll interval.

| Command | Effect |
|---|---|
| `/rollback` | `spec.version = status.history[1].version`, pinned |
| `/deploy <sha>` | resolve SHA to chart version, set `spec.version`, pinned |
| `/resume` | clear `spec.version`, resume the track |

`/resume` is new and is the price of this model. Without it you sit pinned on
an old version for weeks after an incident and nobody notices. The operator
must surface `pinned: true` prominently in its Discord alerts for the same
reason.

`/update` from the old bot disappears — merging to master already deploys.

### The operator is thin

It reconciles `Release` → patches the chart version on the Flux
`OCIRepository`, records history, mirrors `HelmRelease` status back into its
own. It never touches Deployments and never templates a chart.

Flux keeps doing the hard parts: the upgrade itself, health checks, failed
upgrade remediation (`spec.upgrade.remediation.strategy: rollback`), and
Discord alerts through `notification-controller`, which has a native Discord
provider. Anything fatter than this is rebuilding Flux badly.

### Two charts, not one

- **`housebot-operator`** — the CRD, the operator Deployment, its RBAC. This is
  the only thing declared in the GitOps repo: one `HelmRelease`, roughly twenty
  lines, changed maybe twice a year.
- **`housebot`** — bot, sandbox-api, sandbox namespace, database. Never
  referenced from git. The operator installs and versions it.

This split is not cosmetic. **An operator must not be deployed by the thing it
manages.** One chart holding both would mean the `HelmRelease` deploys the
operator, which then patches the `OCIRepository` that same `HelmRelease` reads
from — a genuine bootstrap cycle, plus an operator that can delete itself
mid-upgrade.

Note this means splitting the existing `deploy/helm/housebot` chart, and that
`helm-publish.yml` must publish both.

### Values stay in git

Version churn belongs in-cluster; configuration does not. Moving values into an
unversioned ConfigMap means a cluster rebuild loses your tuning with nothing to
restore from. Values are the payload, not pollution — and they should be the
only thing ever edited in the GitOps repo.

The `Release` CR itself also lives in git: it is the declaration that housebot
exists. `spec.version` is simply absent from the git manifest. Flux applies
with server-side apply and only manages fields it sets, so the operator owning
`spec.version` under a different field manager is a supported arrangement, not
a conflict. **Verify this against your Flux version before relying on it** — if
kustomize-controller is configured to force-apply, it will fight the operator.

## Language: Go

The operator has essentially no housebot business logic. It is pure Kubernetes
and OCI plumbing, and both ecosystems are Go-native.

- Flux's API types are Go packages: `import
  github.com/fluxcd/source-controller/api/v1` gives a typed `OCIRepository`.
  Patching Flux CRs is the operator's whole job, so this is the argument that
  decided it.
- kubebuilder scaffolds the CRD YAML from Go structs via `controller-gen`, plus
  deepcopy, RBAC from markers, and leader election.
- `go-containerregistry` is the reference client for listing GHCR tags.
  Registry auth is exactly where a thinner library would bite.

Costs, accepted: a second toolchain lane in CI, and a `docker-publish.yml`
matrix entry that skips all the Rust/musl steps. The CRD contract spanning two
languages is cheap in practice — the bot patches `spec.version` with a
two-field JSON merge patch and needs the group/version/kind strings, not
generated types.

What would justify revisiting: if the operator grows logic shared with the bot
(version resolution, migration floors, permission checks), one language and one
test suite would win. As specced it shares nothing.

**Location:** `operator/` in this repo, with its own `go.mod` and its own
workflow. A separate repo would be cleaner in isolation but three repos for one
system is worse. `CLAUDE.md` needs a line saying the Go tree has its own build
commands and is not covered by `cargo test --all`.

## What happens to the deployment bot

It becomes a Discord frontend that patches one CR field. No Docker socket, no
GitHub token, no health checks, no persisted deployment state.

Delete:

- `crates/deployment-bot/src/docker.rs` entirely (408 lines)
- the webhook-listening `message` handler in `handler.rs`
- `checkpoint_current_image`, `current_running_sha`,
  `ensure_house_chatbot_running`, `remove_compose_managed_duplicates`
- the whole `DeploymentStage` health-check ladder
- the `/update` command

Keep:

- `permissions.rs` unchanged — authorization is still genuinely its job
- `/rollback`, `/deploy`, `/deployment-access`, reimplemented as CR patches
- add `/resume`

Roughly half the crate goes. **Dropping the Docker socket mount is the largest
security improvement in this change** — that mount is root-equivalent on the
host today, and it is replaced by patch permission on a single named CR.

## Build order

The bot cannot switch to CR-patching until housebot is actually Flux-managed,
and running the old Docker path alongside Flux would be a mess. So:

1. Split the chart in two; publish both from `helm-publish.yml`.
2. Scaffold the operator, CRD, and RBAC. Get `Release` → `OCIRepository` patch
   working with the version set by hand.
3. Add the registry poller and the track/pin logic.
4. Stand up Flux in the cluster: `OCIRepository`, `HelmRelease`, the Discord
   `Provider` and `Alert`. Verify a deploy lands end to end.
5. Only then gut the bot and rewire its three commands.
6. Retire the old Docker path and `scripts/deploy.sh`.

Steps 4 and the GitOps-repo side are the user's to commit — that repo is
private and not touched from here.

## Known limits, not designed away

- **Migrations are the floor on rollback.** Reverting the version reverts the
  image, not the schema. Rollback is only genuinely safe for additive
  migrations, which is a constraint on how migrations are written rather than
  something the operator can enforce today. A `minVersion` floor that a
  destructive migration bumps, so the operator refuses to roll back past it,
  is a plausible later addition — explicitly out of scope for v1.
- **`status.history` is lossy.** etcd is not a backup. If history matters
  beyond convenience, the operator should also emit Kubernetes Events or mirror
  history into the bot's Postgres.
- **gVisor is still untested.** Carried over from the previous handoff: the
  chart's `runtimeClassName: gvisor` default has never been exercised, because
  the test host's Docker only has `runc`.
- **`sandbox.image` is still a literal `:latest`** in `values.yaml`, unlike the
  bot and sandbox-api which resolve through `.Chart.AppVersion`. A
  version-pinned chart therefore still pulls a floating sandbox runtime image,
  which undermines the whole point of pinning. Fixing it means splitting the
  value into `repository`/`tag` — a small breaking values change, deliberately
  not made without asking.
- **`deploy/kubernetes/` still exists** alongside `deploy/helm/`, and
  `manifest_tests.rs` reads the former. The two-path question is still open,
  and the chart split in step 1 is a good moment to settle it.
