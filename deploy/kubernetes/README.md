# Kubernetes deployment

The Compose stack runs the bot and one `sandboxd` on a single host. This
deployment splits that into a gateway tier and a sandbox tier, so the part of
the workload that actually grows can grow.

## Why only the sandboxes scale

Discord allows one gateway connection per shard, and Housebot runs a single
shard, so a second bot replica would receive every event twice. `bot.yaml` is
therefore a one-replica StatefulSet, and that is a protocol constraint rather
than a capacity choice.

Sandboxes are where the work is: each one is a container running a user's
code. `sandbox-api` puts every sandbox in its own Pod and keeps no local
state — the session index is the Pod's own labels — so any replica can serve
any session and the Deployment scales on CPU behind an HPA.

## Isolation

Sandbox Pods run under the `gvisor` RuntimeClass (`handler: runsc`), so a
container escape has to get through gVisor's userspace kernel before it
reaches the host. On top of that, every sandbox Pod:

- drops all capabilities, forbids privilege escalation, and runs non-root on a
  read-only root filesystem under the `RuntimeDefault` seccomp profile;
- mounts no host path — `/workspace`, `/tmp`, and `/home/sandbox` are
  memory-backed `emptyDir`s, so nothing written in a sandbox reaches a disk;
- mounts no ServiceAccount token and gets no service environment variables;
- lives in `housebot-sandboxes` under a default-deny NetworkPolicy, with
  `network: public` sandboxes granted the public internet and every private
  range excluded;
- carries CPU, memory, PID, and ephemeral-storage limits, with a namespace
  ResourceQuota capping the tier as a whole.

Both namespaces enforce Pod Security Admission's `restricted` profile.
`crates/sandbox/tests/manifest_tests.rs` asserts these manifests still match
what the code builds.

## Prerequisites

- Nodes with the `runsc` handler installed, labelled `housebot.dev/gvisor=true`.
- The [CloudNativePG](https://cloudnative-pg.io) operator.
- A CNI that enforces NetworkPolicy (Cilium, Calico); without one the network
  isolation above is not applied.

## Secrets

Nothing here contains a credential.

```sh
kubectl create secret generic housebot-sandbox-api \
  --namespace housebot --from-literal=token="$(openssl rand -hex 32)"

kubectl create secret generic housebot-secrets \
  --namespace housebot --from-env-file=.env
```

CloudNativePG creates `housebot-pg-app` itself; the bot reads `DATABASE_URL`
from it and applies its migrations on startup as it does under Compose.

## Deploy

```sh
kubectl apply -k deploy/kubernetes/overlays/prod
```

## Images

`Dockerfile.sandbox-api` builds the API image from `dist/sandbox-api`, staged
the same way as `dist/sandboxd`. The publish workflow has no matrix entry for
it yet — `CLAUDE.md` puts CI workflows out of scope for automated changes, so
add one alongside the existing `sandboxd` entry before deploying.
