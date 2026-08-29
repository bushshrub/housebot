# housebot chart

Deploys Housebot and its horizontally scalable sandbox tier.

## Architecture

The bot is a StatefulSet pinned to one replica: the Discord gateway allows one
connection per shard and Housebot runs a single shard, so that number is a
protocol constraint rather than a capacity choice. Capacity comes from
`sandbox-api`, a stateless Deployment behind an HPA.

`sandbox-api` is the server the bot calls to run code. It holds no local state:
a sandbox is a Pod, the Pod name derives from the sandbox ID, and the session
index is the Pod's own labels, so any replica can serve any request.

The same binary drives two backends. In this chart it always runs with
`SANDBOX_RUNTIME_BACKEND=kubernetes`, creating gVisor Pods through the API
server. Off-cluster — `docker compose`, or a developer laptop — the binary
defaults to the Docker backend and talks to a local daemon instead. Nothing in
the chart is needed for that path.

## Prerequisites

- A CNI that enforces NetworkPolicy. Without one the sandbox isolation
  policies render but do nothing.
- Nodes running the gVisor `runsc` handler, labelled `housebot.dev/gvisor: "true"`.
- A Postgres database (see below).
- The `metrics-server`, if `sandboxApi.autoscaling.enabled` is left on.

## Database

The chart does not create or manage a database. It only needs a Secret holding
a libpq URI, which keeps the database's lifecycle independent of the release —
a `helm uninstall` can never take the data with it.

This is designed to sit on top of CloudNativePG. Create the Cluster with the
operator, then point the chart at it by name:

```yaml
database:
  cnpgCluster: housebot-pg
```

That resolves to the `housebot-pg-app` Secret and the `uri` key CNPG
publishes, and adds the `cnpg.io/cluster` NetworkPolicy egress rule the bot
needs to reach it.

Any other Postgres works the same way:

```yaml
database:
  existingSecret: my-database
  secretKey: uri
```

## Secrets

`bot.existingSecret` names a Secret mounted with `envFrom` — the Discord token,
LLM credentials, and anything else the bot reads from the environment. Create
it out of band; the chart never templates credentials into a manifest.

The bearer token shared between the bot and `sandbox-api` is generated on first
install and preserved across upgrades by looking up the live Secret, so an
upgrade cannot rotate it out from under a running bot. Supply your own with
`sandboxApi.auth.existingSecret` if you manage secrets externally.

## Running without gVisor

`sandbox.runtimeClassName: ""` drops the `runtimeClassName` field from sandbox
Pods entirely, which is the only way to schedule on a cluster that has no
`runsc` handler installed — a k3d cluster in CI, for example.

This removes the syscall boundary that is the entire point of the sandbox
tier. Never do it on a cluster running untrusted code.

## Verifying an install

```bash
helm test <release> -n <namespace>
```

The test Pod carries the bot's own labels and curls the sandbox API's health
endpoint, so a pass proves the Service resolves and the NetworkPolicy admits
traffic from the bot — the two things that are invisible to a template render.
