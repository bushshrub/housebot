# housebot-operator

Owns which version of the `housebot` chart is running, so that deploying and
rolling back are patches on a field rather than commits in a GitOps repo.

The operator watches a `Release`, resolves it to a concrete chart version, and
writes that version to the tag on a Flux `OCIRepository`. Flux does everything
after that: pulling the chart, the upgrade, health checks, failed-upgrade
remediation, and alerting. The operator never touches a Deployment and never
templates a chart.

## Why this is a separate chart

An operator must not be deployed by the thing it manages. If one chart held both,
its `HelmRelease` would deploy the operator, which would then patch the
`OCIRepository` that same `HelmRelease` reads from — a bootstrap cycle, and an
operator that can delete itself mid-upgrade.

So this chart is the only thing declared in the GitOps repo. The `housebot`
chart is never referenced from git at all.

## Prerequisites

Flux's `source-controller` and `helm-controller` must already be running. They
are cluster-scoped with their own release cadence, so they are a prerequisite
rather than a subchart.

## Install

```bash
helm install housebot-operator \
  oci://ghcr.io/bushshrub/housebot/charts/housebot-operator \
  --namespace housebot --create-namespace
```

CRDs in `crds/` are installed but never upgraded by Helm. Have Flux replace them
on upgrade instead:

```yaml
spec:
  install:
    crds: Create
  upgrade:
    crds: CreateReplace
```

## The GitOps repo side

Three objects, none of which name a chart version:

```yaml
apiVersion: source.toolkit.fluxcd.io/v1
kind: OCIRepository
metadata:
  name: housebot
  namespace: housebot
spec:
  interval: 5m
  url: oci://ghcr.io/bushshrub/housebot/charts/housebot
  # ref.tag is deliberately absent: the operator owns it. Flux applies
  # server-side and only manages fields it sets, so a different field manager
  # owning ref.tag is a supported arrangement — but verify that
  # kustomize-controller is not configured to force-apply, or it will fight the
  # operator on every sync.
---
apiVersion: helm.toolkit.fluxcd.io/v2
kind: HelmRelease
metadata:
  name: housebot
  namespace: housebot
spec:
  interval: 10m
  chartRef:
    kind: OCIRepository
    name: housebot
  upgrade:
    remediation:
      strategy: rollback
      retries: 2
  values: {}   # configuration lives here, in git, and is the only thing edited by hand
---
apiVersion: housebot.dev/v1alpha1
kind: Release
metadata:
  name: housebot
  namespace: housebot
spec:
  track: "0.1.0-sha.*"
  # spec.version is absent, not empty. Writing it here would make Flux its field
  # manager, and every sync would clear an operator pin and undo a rollback.
  ociRepositoryRef:
    name: housebot
  helmReleaseRef:
    name: housebot
  interval: 2m
```

Version churn stays in the cluster; configuration stays in git. `values` on the
`HelmRelease` is the payload, not pollution — an unversioned ConfigMap would
lose your tuning on a cluster rebuild with nothing to restore from.

## Deploying, rolling back, resuming

All three are patches on `spec.version`.

| Intent | Patch |
|---|---|
| roll back | `spec.version` = `status.history[1].version` |
| deploy a specific build | `spec.version` = that chart version |
| resume the track | `spec.version` = `""` |

```bash
kubectl -n housebot patch release housebot --type=merge \
  -p '{"spec":{"version":"0.1.0-sha.4b21f0a"}}'
```

**Setting `spec.version` suspends track following**, and `status.pinned` goes
true. This is the interlock the whole design rests on: without it the operator
would see the newer, broken chart still sitting in the registry and redeploy it
within one poll, undoing the rollback by itself.

The cost is that a pin is forever until someone clears it. Surface
`status.pinned` in your alerting, or the cluster will sit on an old version for
weeks after an incident and nobody will notice.

## How a track is ordered

Master builds are tagged `<chart version>-sha.<short sha>`. These are semver
prereleases, and semver compares prereleases lexically — so ordering them by
semver alone means ordering them by the hex of a git hash, which carries no
information at all.

Versions are therefore ranked by **core version first, then by push time**, read
from the `org.opencontainers.image.created` annotation Helm stamps on the chart
manifest. A `0.2.0` release outranks any `0.1.0-sha.*`; among sha builds the most
recently pushed wins; and a real release outranks a prerelease of the same core
version regardless of when it was pushed.

Tags that match the track but are not valid semver — `latest`, for one — are
ignored rather than deployed.

## Cluster rebuild

The registry is the source of truth, not the operator's `status`. On an empty
etcd the operator lists the registry, finds the newest version on the track, and
deploys it. Losing `status` costs the history ledger, not the ability to run.

## Known limits

- **Migrations are the floor on rollback.** Reverting the version reverts the
  image, not the schema, so rollback is only genuinely safe across additive
  migrations. Nothing here enforces that.
- **`status.history` is lossy.** etcd is not a backup. If history matters beyond
  convenience it should be mirrored somewhere durable.
