# portal chart

Runs the portal, which serves pages, the webhook route and runner links. State lives in an existing Postgres. The chart ships no Postgres and holds no secret value.

The values schema describes every value, and the values file holds the defaults.

## Install

Create the Secret first. The chart only names it.

```sh
kubectl create namespace portal
kubectl -n portal create secret generic portal-secrets \
  --from-literal=database-url='postgres://USER:PASSWORD@HOST:5432/DB?sslmode=require' \
  --from-literal=secret-key="$(openssl rand -base64 32)" \
  --from-literal=internal-secret="$(openssl rand -hex 16)"
helm upgrade --install portal ./deploy/chart -n portal \
  --set image.repository=REGISTRY/portal --set image.digest=sha256:DIGEST \
  --set secrets.existingSecret=portal-secrets \
  --set portal.userHeader=X-Forwarded-Email \
  --set portal.publicURL=https://portal.example.org \
  --wait --timeout 5m
```

Keep the secret key. It encrypts stored secret values, and a new key cannot read them.

## Migrations

Each pod migrates on start. The store takes a Postgres advisory lock through goose. Pods that start together apply each migration once and the rest find nothing to do. The cluster test starts both replicas from an empty database three times and checks the migration table each time.

The spec describes a migration Job before the rollout. The portal has no migrate-only command, so the chart has no Job. Schema changes only add, so a pod of the previous version keeps working while a new pod migrates.

## Stop rule

A pod told to stop sleeps for stop.waitSeconds (10) while the Service stops sending traffic. The portal then drains for at most stop.drainSeconds (30). The termination grace period (60) must be longer than both, and the chart refuses to render otherwise. Rollouts run one pod at a time with a disruption budget, and pods spread across nodes when the cluster has them.

The sleep is the kubelet's own preStop sleep, so the chart needs Kubernetes 1.30 or newer.

## Sign-in

The portal trusts the header named by portal.userHeader and checks nothing else. An OIDC proxy in front sets it, and the chart writes none. Use oauth2-proxy and its official chart.

```sh
helm repo add oauth2-proxy https://oauth2-proxy.github.io/manifests
kubectl -n portal create secret generic oauth2-proxy \
  --from-literal=client-id=CLIENT_ID --from-literal=client-secret=CLIENT_SECRET \
  --from-literal=cookie-secret="$(openssl rand -base64 24 | tr -- '+/' '-_')"
helm upgrade --install oauth2-proxy oauth2-proxy/oauth2-proxy -n portal \
  -f deploy/chart/examples/oauth2-proxy-values.yaml
```

The example values set the issuer, the allowed email domain, the upstream (this chart's Service) and the proxy's own Ingress. In reverse-proxy mode the proxy sets X-Forwarded-Email, which is the value to give portal.userHeader.

People reach the portal only through the proxy's host. A request that reaches the Service directly can name any person, so nothing else may route to it.

## Machine paths

Some callers cannot sit behind a browser sign-in. They are the GitHub webhook, the runner join and link, and the proxies and desks of tasks, which call with per-task tokens. Turn on the chart's own Ingress with a host of its own. It routes only these paths and never the pages. They are `/hooks/github`, `/runner/join`, `/runner/link`, `/internal/policy`, `/internal/connections`, `/internal/blocked` and the desk's publish route `/api/tasks/<id>/results`. The publish route has a wildcard segment, so it is a regular expression in `ingress.patterns`. The chart sets the ingress-nginx `use-regex` annotation for it. Another controller needs its own way to read it. The terminal hop between replicas, under `/internal/terminal`, stays inside the cluster. Event streams are long responses, so turn response buffering off in the controller.
