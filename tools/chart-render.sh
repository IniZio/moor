#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
chart=(deploy/helm/moor -f deploy/helm/required-values.yaml -f deploy/helm/full-values.yaml)
fail() { echo "chart-render: $*" >&2; exit 1; }

full=$(helm template moor "${chart[@]}")
no_ingress=$(helm template moor "${chart[@]}" --set gate.ingress.enabled=false)

grep -qx 'kind: Deployment' <<<"$full" || fail "the render holds no Deployment"
if grep -E '^(apiVersion: policy/|kind: (NetworkPolicy|PodDisruptionBudget))' <<<"$full"; then
  fail "renders a PodDisruptionBudget, NetworkPolicy or policy/ kind"
fi
if grep -E '^apiVersion: (networking\.k8s\.io|policy)/' <<<"$no_ingress"; then
  fail "renders a networking.k8s.io or policy/ kind with the gate ingress off"
fi

# The Secret keys mounted as files, per Deployment: "<deployment> <key>".
mounted=$(awk '
  /^kind:/ { kind = $2; name = "" }
  kind == "Deployment" && /^  name:/ && name == "" { name = $2 }
  kind == "Deployment" && /- \{ key: / { key = $4; sub(/,$/, "", key); print name, key }
' <<<"$full")
if grep -E ' (ledger-ca\.key|runner-client\.(crt|key))$' <<<"$mounted" | grep -v '^moor-runner '; then
  fail "mounts the CA key, or the runner certificate outside the runner pods"
fi
egress=$(awk '$1 == "moor-egress" { print $2 }' <<<"$mounted" | sort | tr '\n' ' ')
[ "$egress" = "egress-client.crt egress-client.key ledger-ca.pem " ] ||
  fail "egress pods mount [$egress], want the CA and their client certificate only"
controller=$(awk '$1 == "moor" { print $2 }' <<<"$mounted" | grep -E '^(ledger|egress|runner)-' | sort | tr '\n' ' ')
[ "$controller" = "ledger-ca.pem ledger-tls.crt ledger-tls.key " ] ||
  fail "the controller mounts [$controller], want the CA and the ledger certificate only"

separate=$(helm template moor "${chart[@]}" --set runner.separate=true)
if grep -E '^(apiVersion: policy/|kind: (NetworkPolicy|PodDisruptionBudget))' <<<"$separate"; then
  fail "renders a policy kind with runner.separate"
fi
runner=$(awk '
  /^kind:/ { kind = $2; name = "" }
  kind == "Deployment" && /^  name:/ && name == "" { name = $2 }
  kind == "Deployment" && name == "moor-runner" && /- \{ key: / { key = $4; sub(/,$/, "", key); print key }
' <<<"$separate" | sort | tr '\n' ' ')
[ "$runner" = "ledger-ca.pem runner-client.crt runner-client.key " ] ||
  fail "runner pods mount [$runner], want the CA and their client certificate only"

apart=$(helm template moor "${chart[@]}" --set gate.separate=true --set egress.ledgerTls=null)
gate_pod=$(awk '
  /^kind:/ { kind = $2; name = "" }
  kind == "Deployment" && /^  name:/ && name == "" { name = $2 }
  kind == "Deployment" && name == "moor-gate" { print }
' <<<"$apart")
[ -n "$gate_pod" ] || fail "gate.separate renders no gate Deployment"
grep -q 'replicas: 2' <<<"$gate_pod" || fail "the gate Deployment does not run two replicas"
grep -q 'type: RollingUpdate' <<<"$gate_pod" || fail "the gate Deployment does not roll"
grep -q -- '--controller=tls://moor-ledger\.' <<<"$gate_pod" || fail "the gate pods dial no ledger"
grep -q 'path: /.well-known/moor' <<<"$gate_pod" || fail "the gate pods have another readiness probe"
grep -q 'name: MOOR_GATE_FRONT_KEY' <<<"$gate_pod" || fail "the gate pods hold no front key"
if grep -qE 'persistentVolumeClaim|name: state|--front-socket' <<<"$gate_pod"; then
  fail "the gate pods use the volume"
fi
grace=$(awk '/terminationGracePeriodSeconds:/ { print $2 }' <<<"$gate_pod")
[ "$grace" -gt 40 ] || fail "the gate pods' grace $grace does not outlast the preStop and the hold"
gate_keys=$(grep -E '^ +- \{ key: ' <<<"$gate_pod" | awk '{ print $4 }' | tr -d , | sort | tr '\n' ' ')
[ "$gate_keys" = "gate-client.crt gate-client.key ledger-ca.pem " ] ||
  fail "gate pods mount [$gate_keys], want the CA and their client certificate only"
grep -q 'key_file: /var/lib/moor/config/gate-front-key' <<<"$apart" || fail "gate.separate leaves the authgear front local"
if grep -E '^(apiVersion: policy/|kind: (NetworkPolicy|PodDisruptionBudget))' <<<"$apart"; then
  fail "renders a policy kind with gate.separate"
fi
controller_pod=$(awk '
  /^kind:/ { kind = $2; name = "" }
  kind == "Deployment" && /^  name:/ && name == "" { name = $2 }
  kind == "Deployment" && name == "moor" { print }
' <<<"$apart")
if grep -q 'command: \[moor-gate\]' <<<"$controller_pod"; then fail "gate.separate keeps the sidecar"; fi
grep -q 'name: moor-agents$' <<<"$apart" || fail "gate.separate leaves the sidecar runner's agents without a Service"
if helm template moor deploy/helm/moor -f deploy/helm/required-values.yaml --set gate.separate=true 2>&1 | grep -qv 'gate.separate needs egress.enabled'; then
  fail "gate.separate without egress.enabled renders"
fi
helm template moor "${chart[@]}" --set gate.separate=true --set gate.clientCert=c --set gate.clientKey=k >/dev/null ||
  fail "gate.separate refuses a given client certificate"
if helm template moor "${chart[@]}" --set gate.separate=true 2>&1 | grep -qv 'your own ledger CA needs'; then
  fail "gate.separate with your own CA and no way to a certificate renders"
fi
if helm template moor "${chart[@]}" --set gate.separate=true --set egress.ledgerTls=null --set gate.drainSecs=30 2>&1 | grep -qv 'gate.drainSecs must outlast'; then
  fail "a drain shorter than the hold renders"
fi
echo "chart-render: ok"
