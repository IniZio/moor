#!/bin/sh
# Renders the chart and checks the rules the spec sets for a deploy without disruption. Offline: needs helm and yq.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
chart="$here/chart"
fails=0

render() { helm template portal "$chart" -f "$chart/ci/values.yaml" "$@"; }
pick() { yq eval-all -N "$1" -; }

check() { # check <name> <got> <want>
	if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'" >&2; fails=$((fails + 1)); fi
}
refuses() { # refuses <name> <helm args>... : the render must fail
	name=$1
	shift
	if render "$@" >/dev/null 2>&1; then echo "FAIL $name: rendered, want a refusal" >&2; fails=$((fails + 1)); else echo "ok   $name"; fi
}

out=$(render)
dep() { printf '%s\n' "$out" | pick "select(.kind == \"Deployment\") | $1"; }
kinds=$(printf '%s\n' "$out" | pick '.kind' | sort | tr '\n' ' ')

check "kinds rendered by default" "$kinds" "Deployment PodDisruptionBudget Service ServiceAccount "
check "two replicas" "$(dep .spec.replicas)" 2
check "roll one at a time" "$(dep '.spec.strategy.rollingUpdate | .maxSurge + "/" + .maxUnavailable')" "1/0"
check "readiness on /readyz" "$(dep '.spec.template.spec.containers[0].readinessProbe.httpGet.path')" /readyz
check "liveness on /healthz" "$(dep '.spec.template.spec.containers[0].livenessProbe.httpGet.path')" /healthz
check "waits for traffic to move" "$(dep '.spec.template.spec.containers[0].lifecycle.preStop.sleep.seconds')" 10
check "grace period beats wait plus drain" "$(dep '.spec.template.spec.terminationGracePeriodSeconds > 40')" true
check "pods spread across nodes" "$(dep '.spec.template.spec.topologySpreadConstraints[0].topologyKey')" kubernetes.io/hostname
check "runs as non-root" "$(dep '.spec.template.spec.securityContext.runAsNonRoot')" true
check "root filesystem read-only" "$(dep '.spec.template.spec.containers[0].securityContext.readOnlyRootFilesystem')" true
check "no service account token" "$(dep '.spec.template.spec.automountServiceAccountToken')" false
check "listens on every address" "$(dep '.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_LISTEN") | .value')" 0.0.0.0:8080
check "replica reachable by pod address" "$(dep '.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_INTERNAL_URL") | .value')" 'http://$(POD_IP):8080'
check "database url from a secret" "$(dep '.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_DATABASE_URL") | .valueFrom.secretKeyRef.name')" portal-secrets
check "secret key from a secret" "$(dep '.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_SECRET_KEY") | .valueFrom.secretKeyRef.key')" secret-key
check "internal secret from a secret" "$(dep '.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_INTERNAL_SECRET") | .valueFrom.secretKeyRef.key')" internal-secret
check "chart holds no secret value" "$(printf '%s\n' "$out" | pick 'select(.kind == "Secret") | .kind')" ""
check "disruption budget allows one" "$(printf '%s\n' "$out" | pick 'select(.kind == "PodDisruptionBudget") | .spec.maxUnavailable')" 1
check "no GitHub settings by default" "$(dep '[.spec.template.spec.containers[0].env[] | select(.name | test("GITHUB"))] | length')" 0

refuses "refuses a missing Secret name" --set secrets.existingSecret=
refuses "refuses a missing user header" --set portal.userHeader=
refuses "refuses a grace period that cuts the drain" --set stop.waitSeconds=10 --set stop.drainSeconds=30 --set terminationGracePeriodSeconds=40
refuses "refuses a GitHub App without its key Secret" --set portal.github.appID=123

gh=$(render --set portal.github.appID=123 --set portal.github.privateKeySecret=portal-github --set portal.github.apiURL=https://api.example.org/)
ghenv() { printf '%s\n' "$gh" | pick "select(.kind == \"Deployment\") | $1"; }
check "GitHub App id set" "$(ghenv '.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_GITHUB_APP_ID") | .value')" 123
check "GitHub webhook secret from a secret" "$(ghenv '.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_GITHUB_WEBHOOK_SECRET") | .valueFrom.secretKeyRef.key')" github-webhook-secret
check "GitHub key mounted from a secret" "$(ghenv '.spec.template.spec.volumes[] | select(.name == "github-key") | .secret.secretName')" portal-github

ing=$(render --set ingress.enabled=true --set ingress.host=portal.example.org)
check "ingress on request" "$(printf '%s\n' "$ing" | pick 'select(.kind == "Ingress") | .spec.rules[0].host')" portal.example.org
check "ingress exposes only the machine paths" "$(printf '%s\n' "$ing" | pick 'select(.kind == "Ingress") | [.spec.rules[0].http.paths[].path] | join(" ")')" "/hooks/github /runner/join /runner/link"

np=$(render --set service.type=NodePort --set service.nodePort=30080)
check "node port when set" "$(printf '%s\n' "$np" | pick 'select(.kind == "Service") | .spec.ports[0].nodePort')" 30080

one=$(render --set replicaCount=1)
check "one replica still renders" "$(printf '%s\n' "$one" | pick 'select(.kind == "Deployment") | .spec.replicas')" 1

[ "$fails" -eq 0 ] || { echo "chart-test: $fails failed" >&2; exit 1; }
echo "chart-test: all passed"
