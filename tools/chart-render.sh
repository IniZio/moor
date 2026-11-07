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
if grep -E ' (ledger-ca\.key|slack-client\.(crt|key)|runner-client\.(crt|key))$' <<<"$mounted" | grep -v '^moor-runner '; then
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

slack=$(helm template moor "${chart[@]}" --set slack.separate=true)
if grep -E '^(apiVersion: policy/|kind: (NetworkPolicy|PodDisruptionBudget))' <<<"$slack"; then
  fail "renders a policy kind with slack.separate"
fi
slack_mounted=$(awk '
  /^kind:/ { kind = $2; name = "" }
  kind == "Deployment" && /^  name:/ && name == "" { name = $2 }
  kind == "Deployment" && /- \{ key: / { key = $4; sub(/,$/, "", key); print name, key }
' <<<"$slack")
own=$(awk '$1 == "moor-slack" { print $2 }' <<<"$slack_mounted" | sort | tr '\n' ' ')
[ "$own" = "ledger-ca.pem slack-app-token slack-bot-token slack-client.crt slack-client.key slack-forward.json slack-front-key slack.yaml " ] ||
  fail "Slack pods mount [$own], want the CA, their client certificate, front key, tokens and config only"
if awk '$1 == "moor-slack" { print $2 }' <<<"$slack_mounted" | grep -E '^(ledger-ca\.key|ledger-tls|egress|runner|vault|agent37|database)'; then
  fail "Slack pods mount a key of the controller, the Proxy or the runner"
fi
if grep -E ' slack-client\.(crt|key)$' <<<"$slack_mounted" | grep -v '^moor-slack '; then
  fail "mounts the Slack client certificate outside the Slack pods"
fi
controller_pod=$(awk '
  /^kind:/ { kind = $2; name = "" }
  kind == "Deployment" && /^  name:/ && name == "" { name = $2 }
  kind == "Deployment" && name == "moor" { print }
' <<<"$slack")
if grep -qE '^ +- name: slack$|moor-slack' <<<"$controller_pod"; then
  fail "the controller pod still runs Slack with slack.separate"
fi

# With the flag off, Slack stays a sidecar and no Slack Deployment renders.
if grep -qE '^  name: moor-slack$' <<<"$full"; then
  fail "renders the Slack Deployment with slack.separate off"
fi
echo "chart-render: ok"
