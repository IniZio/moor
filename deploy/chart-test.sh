#!/bin/sh
# Renders the chart and checks the rules the spec sets for a deploy without disruption. Offline: needs helm and yq.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
chart="$here/chart"
fails=0

render() { helm template portal "$chart" -f "$chart/ci/values.yaml" "$@"; }
pick() { yq eval-all -N "$1" -; }
vals() { # vals <helm args>... : render with the values file on stdin
	f=$(mktemp)
	cat >"$f"
	render -f "$f" "$@"
	rm -f "$f"
}

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
check "no runner args by default" "$(dep '[.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_RUNNER_ARGS")] | length')" 0
ra=$(render --set 'portal.runnerArgs=-box-image b:1 -desk-image d:1')
check "runner args reach the portal" "$(printf '%s\n' "$ra" | pick 'select(.kind == "Deployment") | .spec.template.spec.containers[0].env[] | select(.name == "PORTAL_RUNNER_ARGS") | .value')" "-box-image b:1 -desk-image d:1"

ing=$(render --set ingress.enabled=true --set ingress.host=portal.example.org)
check "ingress on request" "$(printf '%s\n' "$ing" | pick 'select(.kind == "Ingress") | .spec.rules[0].host')" portal.example.org
check "ingress exposes only the machine paths" "$(printf '%s\n' "$ing" | pick 'select(.kind == "Ingress") | [.spec.rules[0].http.paths[].path] | join(" ")')" "/hooks/github /runner/join /runner/link /internal/policy /internal/credentials /internal/blocked /api/tasks/[^/]+/results"
check "only the wildcard path is a pattern" "$(printf '%s\n' "$ing" | pick 'select(.kind == "Ingress") | [.spec.rules[0].http.paths[] | select(.pathType == "ImplementationSpecific") | .path] | join(" ")')" "/api/tasks/[^/]+/results"
check "patterns are read as regular expressions" "$(printf '%s\n' "$ing" | pick 'select(.kind == "Ingress") | .metadata.annotations["nginx.ingress.kubernetes.io/use-regex"]')" true
check "ingress leaves out what a person uses" "$(printf '%s\n' "$ing" | pick 'select(.kind == "Ingress") | [.spec.rules[0].http.paths[].path | select(. == "/" or . == "/api" or . == "/api/tasks" or test("^/internal/terminal") or . == "/internal")] | length')" 0
noregex=$(render --set ingress.enabled=true --set ingress.host=portal.example.org --set 'ingress.patterns=null')
check "no pattern, no regex annotation" "$(printf '%s\n' "$noregex" | pick 'select(.kind == "Ingress") | .metadata.annotations["nginx.ingress.kubernetes.io/use-regex"] // "none"')" none

# Everything the new options add is absent until it is asked for.
check "no schema by default" "$(dep '[.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_DATABASE_SCHEMA")] | length')" 0
check "no seed by default" "$(dep '[.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_SEED_SETTINGS_FILE")] | length')" 0
check "no volumes by default" "$(dep '.spec.template.spec.volumes // "none"')" none
check "no mounts by default" "$(dep '.spec.template.spec.containers[0].volumeMounts // "none"')" none
check "no rollout annotation by default" "$(dep '.spec.template.metadata.annotations // "none"')" none
sc=$(render --set portal.databaseSchema=moor)
check "schema reaches the portal" "$(printf '%s\n' "$sc" | pick 'select(.kind == "Deployment") | .spec.template.spec.containers[0].env[] | select(.name == "PORTAL_DATABASE_SCHEMA") | .value')" moor

seeded() { vals "$@" <<'EOF'
seed:
  settings: |
    # first settings
    repositories: ["example-org/shop-web"]
    credentials: {shop: {value: "p@ss: word #1"}}
EOF
}
sd=$(seeded)
sdep() { printf '%s\n' "$sd" | pick "select(.kind == \"Deployment\") | $1"; }
check "seed renders a Secret of its own" "$(printf '%s\n' "$sd" | pick 'select(.kind == "Secret") | .metadata.name')" portal-seed
check "seed document is kept as typed" "$(printf '%s\n' "$sd" | pick 'select(.kind == "Secret") | .stringData["settings.yaml"]' | sed '$d')" '# first settings
repositories: ["example-org/shop-web"]
credentials: {shop: {value: "p@ss: word #1"}}'
check "seed file named to the portal" "$(sdep '.spec.template.spec.containers[0].env[] | select(.name == "PORTAL_SEED_SETTINGS_FILE") | .value')" /etc/portal/seed/settings.yaml
check "seed mounted from its Secret" "$(sdep '.spec.template.spec.volumes[] | select(.name == "seed") | .secret.secretName')" portal-seed
check "seed mounted read-only where the portal reads it" "$(sdep '.spec.template.spec.containers[0].volumeMounts[] | select(.name == "seed") | .readOnly | tostring + " "' )$(sdep '.spec.template.spec.containers[0].volumeMounts[] | select(.name == "seed") | .mountPath')" "true /etc/portal/seed"
check "seed leaves the database secret to the existing one" "$(printf '%s\n' "$sd" | pick 'select(.kind == "Secret") | .metadata.name' | wc -l | tr -d ' ')" 1
check "seed is not a ConfigMap" "$(printf '%s\n' "$sd" | pick 'select(.kind == "ConfigMap") | .kind')" ""
both=$(vals --set 'extraVolumes[0].name=home' --set 'extraVolumeMounts[0].name=home' --set 'extraVolumeMounts[0].mountPath=/home-repo' <<'EOF'
seed:
  settings: "repositories: []"
EOF
)
check "seed and extra volumes coexist" "$(printf '%s\n' "$both" | pick 'select(.kind == "Deployment") | [.spec.template.spec.volumes[].name] | join(" ")')" "seed home"
check "seed and extra mounts coexist" "$(printf '%s\n' "$both" | pick 'select(.kind == "Deployment") | [.spec.template.spec.containers[0].volumeMounts[].name] | join(" ")')" "seed home"
sum() { printf '%s\n' "$1" | pick 'select(.kind == "Deployment") | .spec.template.metadata.annotations["checksum/secrets"]'; }
other=$(vals <<'EOF'
seed:
  settings: "repositories: []"
EOF
)
check "same seed, same rollout checksum" "$(sum "$other")" "$(sum "$(vals <<'EOF'
seed:
  settings: "repositories: []"
EOF
)")"
check "another seed rolls the pods" "$([ "$(sum "$other")" != "$(sum "$sd")" ] && echo changed)" changed

secrets() { vals "$@" <<'EOF'
secrets:
  existingSecret: ""
  create: true
  values:
    databaseURL: postgres://u:p@db.example.org:5432/app
    secretKey: a2V5
    internalSecret: shared
EOF
}
cs=$(secrets)
check "chart renders the Secret on request" "$(printf '%s\n' "$cs" | pick 'select(.kind == "Secret") | .metadata.name')" portal-secrets
check "rendered Secret holds the three values" "$(printf '%s\n' "$cs" | pick 'select(.kind == "Secret") | [.stringData["database-url"], .stringData["secret-key"], .stringData["internal-secret"]] | join(" ")')" "postgres://u:p@db.example.org:5432/app a2V5 shared"
check "pod reads the rendered Secret" "$(printf '%s\n' "$cs" | pick 'select(.kind == "Deployment") | [.spec.template.spec.containers[0].env[] | select(.valueFrom.secretKeyRef) | .valueFrom.secretKeyRef.name] | unique | join(" ")')" portal-secrets
check "rendered Secret follows the key names" "$(secrets --set secrets.keys.databaseURL=url | pick 'select(.kind == "Secret") | .stringData.url')" postgres://u:p@db.example.org:5432/app
check "another secret value rolls the pods" "$([ "$(sum "$cs")" != "$(sum "$(secrets --set secrets.values.internalSecret=other)")" ] && echo changed)" changed
check "an existing Secret adds no rollout annotation" "$(sum "$out")" null
refuses "refuses both a created and an existing Secret" --set secrets.create=true --set secrets.values.databaseURL=x --set secrets.values.secretKey=y --set secrets.values.internalSecret=z
refuses "refuses a created Secret without its database URL" --set secrets.existingSecret= --set secrets.create=true --set secrets.values.secretKey=y --set secrets.values.internalSecret=z
refuses "refuses a created Secret without its secret key" --set secrets.existingSecret= --set secrets.create=true --set secrets.values.databaseURL=x --set secrets.values.internalSecret=z
refuses "refuses a created Secret without its internal secret" --set secrets.existingSecret= --set secrets.create=true --set secrets.values.databaseURL=x --set secrets.values.secretKey=y
pa=$(secrets --set podAnnotations.team=ops)
check "pod annotations stay beside the checksum" "$(printf '%s\n' "$pa" | pick 'select(.kind == "Deployment") | .spec.template.metadata.annotations | keys | join(" ")')" "checksum/secrets team"

hosts() { vals --set ingress.enabled=true "$@" <<'EOF'
ingress:
  host: portal.example.org
  className: nginx
  tlsSecretName: portal-tls
  hosts:
    - name: runners
      host: runners.example.org
    - name: hooks
      host: hooks.example.org
      className: internal
      tlsSecretName: ""
      paths: [/hooks/github]
      patterns: []
      annotations: {example.org/zone: edge}
EOF
}
hs=$(hosts)
ing() { printf '%s\n' "$hs" | pick "select(.kind == \"Ingress\" and .metadata.name == \"$1\") | $2"; }
check "one Ingress per host entry" "$(printf '%s\n' "$hs" | pick 'select(.kind == "Ingress") | .metadata.name' | sort | tr '\n' ' ')" "portal-hooks portal-runners "
check "an entry keeps the top-level class" "$(ing portal-runners .spec.ingressClassName)" nginx
check "an entry may replace the class" "$(ing portal-hooks .spec.ingressClassName)" internal
check "an entry keeps the top-level TLS Secret" "$(ing portal-runners '.spec.tls[0].secretName + " " + .spec.tls[0].hosts[0]')" "portal-tls runners.example.org"
check "an entry may drop the TLS Secret" "$(ing portal-hooks '.spec.tls // "none"')" none
check "an entry keeps the default paths" "$(ing portal-runners '[.spec.rules[0].http.paths[].path] | join(" ")')" "/hooks/github /runner/join /runner/link /internal/policy /internal/credentials /internal/blocked /api/tasks/[^/]+/results"
check "an entry may replace the paths and empty the patterns" "$(ing portal-hooks '[.spec.rules[0].http.paths[].path] | join(" ")')" "/hooks/github"
check "an empty pattern list leaves the regex annotation out" "$(ing portal-hooks '.metadata.annotations["nginx.ingress.kubernetes.io/use-regex"] // "none"')" none
check "an entry with patterns gets the regex annotation" "$(ing portal-runners '.metadata.annotations["nginx.ingress.kubernetes.io/use-regex"]')" true
check "an entry sets its own annotations" "$(ing portal-hooks '.metadata.annotations["example.org/zone"]')" edge
check "an entry's host is its own" "$(ing portal-hooks .spec.rules[0].host)" hooks.example.org
check "every Ingress backs onto the portal Service" "$(printf '%s\n' "$hs" | pick 'select(.kind == "Ingress") | .spec.rules[0].http.paths[0].backend.service.name' | sort -u)" portal
nameless=$(printf 'ingress:\n  hosts:\n    - host: a.example.org\n')
refuses "refuses a host entry without a name" --set ingress.enabled=true -f /dev/stdin <<EOF
$nameless
EOF
refuses "refuses a host entry without a host" --set ingress.enabled=true --set ingress.host= --set 'ingress.hosts[0].name=a'
inherit=$(render --set ingress.enabled=true --set ingress.host=portal.example.org --set 'ingress.hosts[0].name=a')
check "an entry without a host takes the top-level one" "$(printf '%s\n' "$inherit" | pick 'select(.kind == "Ingress") | .metadata.name + " " + .spec.rules[0].host')" "portal-a portal.example.org"

np=$(render --set service.type=NodePort --set service.nodePort=30080)
check "node port when set" "$(printf '%s\n' "$np" | pick 'select(.kind == "Service") | .spec.ports[0].nodePort')" 30080

one=$(render --set replicaCount=1)
check "one replica still renders" "$(printf '%s\n' "$one" | pick 'select(.kind == "Deployment") | .spec.replicas')" 1

[ "$fails" -eq 0 ] || { echo "chart-test: $fails failed" >&2; exit 1; }
echo "chart-test: all passed"
