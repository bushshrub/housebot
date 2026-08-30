# Helm chart for Housebot, and the untested-deployment problem

Date: 2026-08-29
Branch: `claude/k8s-deployment-testing-fjobnz` (based on PR #325's head, `d7d81b3`)

## Why this session existed

PR #325 ("Scale the sandbox tier on Kubernetes behind an HTTP API") added a
Kubernetes deployment that had never been run against a real cluster. The goal
was to stand up k3d locally, deploy the bot under gVisor, and fix what broke.

**That end-to-end test never happened.** See the blocker below. Everything in
this handoff was verified by rendering, compiling and unit tests only — no
manifest in this repo has yet been applied to a live API server.

## Blocker: no container images can be pulled

The session's egress policy denies both registry blob CDNs:

- `production.cloudfront.docker.com` (Docker Hub)
- `pkg-containers.githubusercontent.com` (GHCR)

Registry auth and manifest fetches succeed; only the blob hosts 403. Plain
`curl` fails identically to `docker pull`, so it is the egress proxy, not the
daemon. That blocks k3d (needs `rancher/k3s`), any `docker build`, and any
deploy. The user approved the domains mid-session but the change never reached
this container — egress policy appears to be stamped at session start.

**Next session: retry `docker pull rancher/k3s:v1.31.5-k3s1` first.** If it
succeeds, the whole plan below is unblocked. If it still 403s, stop and raise
it rather than hunting for mirrors.

## Environment setup that worked (redo in a fresh session)

- `dockerd` is installed but not running; start it manually. It inherits
  `HTTPS_PROXY` from the shell, which it needs.
- The k3d install script fails (403 on the GitHub API). Download the binary
  directly: `https://github.com/k3d-io/k3d/releases/download/v5.8.3/k3d-linux-amd64`.
  Release *assets* on github.com are allowed even though the API is not.
- `get.helm.sh` is blocked. Go 1.24 is present and `proxy.golang.org` is
  allowed, so build helm from source:
  `GOBIN=/usr/local/bin go install helm.sh/helm/v3/cmd/helm@v3.16.4`
- `kubectl` installs normally from `dl.k8s.io`.

## What changed on this branch

### `deploy/helm/housebot/` — a new chart

Covers the same ground as `deploy/kubernetes/` plus the gaps below. Validated
with `helm lint` and `helm template` across the cnpg, external-secret, no-db,
both-set and no-gvisor value paths. A `values.schema.json` rejects bad input at
template time.

The database has four mutually exclusive sources, validated in one helper:
`database.url` (a literal URI the chart wraps in a Secret),
`database.existingSecret`, `database.cnpgCluster` (an existing CNPG Cluster by
name), and `database.cnpg.create` (a CNPG Cluster the chart renders, configured
from values.yaml). Setting none or several fails at template time.

**The chart never installs the CloudNativePG operator.** `database.cnpg.create`
renders the Cluster resource only and refuses to template when the
`postgresql.cnpg.io/v1` CRD is absent, pointing the operator at installation
instead. The Cluster carries `helm.sh/resource-policy: keep` so an uninstall
cannot take the data.

### `crates/sandbox/src/kubernetes.rs` — a real bug fix

`runtimeClassName` was unconditionally emitted, defaulting to `gvisor` when
`HOUSEBOT_SANDBOX_RUNTIME_CLASS` was unset. There was **no way to run on a
cluster without runsc** — every sandbox Pod would sit unschedulable. This is
precisely what would have blocked the k3d test, since k3d has no gVisor.

`runtime_class_from` is now a pure function; an empty override drops the field.
Made pure rather than env-mutating on purpose: the existing tests at
`kubernetes_tests.rs:20,28` assert the gvisor default, and a parallel test
mutating that env var would flake them. Three tests added (26 → 29).

## Defects found in PR #325 (all still unfixed in `deploy/kubernetes/`)

The chart fixes these; the kustomize manifests do not.

1. **No governing Service.** `bot.yaml` sets `serviceName: housebot`, but the
   only Service in all 19 rendered objects is `sandbox-api`. StatefulSet Pods
   get no stable DNS.
2. **No Secrets exist.** `housebot-secrets`, `housebot-sandbox-api` and
   `housebot-pg-app` are all referenced; `grep -c '^kind: Secret'` is 0. Every
   Pod lands in `CreateContainerConfigError` on a fresh apply.
3. **CNPG `Cluster` with no operator.** `apply -k` fails outright on the
   unknown CRD.

**Why CI did not catch any of this:** `cargo test -p housebot-sandbox` is fully
green — 229 tests including 12 `manifest_tests` and 29 `kubernetes_tests`. Those
tests assert over generated structs and re-parsed YAML and never contact an API
server. Green here is not evidence the deployment works. A cluster smoke test is
the only thing that would have caught items 1-3.

## Open decision: two deployment paths

`deploy/kubernetes/` (kustomize) and `deploy/helm/housebot/` now overlap. I did
not delete the former because `manifest_tests.rs:11` reads
`deploy/kubernetes/base` and would fail to compile.

Someone should decide: keep the chart as the only path and repoint or drop
those tests, or keep both and accept that they will drift. Two divergent
deployment paths is how #325 shipped untested in the first place.

## Next steps, in order

1. Retry the image pull. If blocked, stop and escalate.
2. `k3d cluster create` (1 server + 1 agent is enough).
3. Install the CloudNativePG operator, create a `housebot-pg` Cluster.
4. Build and import the bot / sandbox-api / sandbox images into the cluster
   (`k3d image import`) — they are not published anywhere reachable.
5. `helm install hb deploy/helm/housebot --set database.cnpgCluster=housebot-pg
   --set sandbox.runtimeClassName=""` — k3d has no runsc, so the escape hatch
   above is required.
6. Assert Pods reach Ready, then `helm test hb`.
7. **Then** test gVisor properly, which needs a cluster with runsc installed on
   the nodes. k3d cannot do this out of the box; this is the one requirement no
   local setup here has yet satisfied, and the original ask.
8. Fold the fixes back into `deploy/kubernetes/`, or resolve the decision above.
