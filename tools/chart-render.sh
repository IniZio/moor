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
grep -q 'controller.alive' <<<"$full" || fail "a database-backed controller has no liveness probe"
if helm template moor deploy/helm/moor -f deploy/helm/required-values.yaml | grep -q 'controller.alive'; then
  fail "a file-backed controller has a liveness probe, but nothing feeds it"
fi

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

gate_tls=(--set gate.overTls.enabled=true --set gate.overTls.frontKey=k --set gate.overTls.clientCert=c --set gate.overTls.clientKey=k)
over=$(helm template moor "${chart[@]}" "${gate_tls[@]}")
grep -q -- '--controller=tls://moor-ledger\.' <<<"$over" || fail "gate.overTls renders no --controller"
grep -q 'name: MOOR_GATE_FRONT_KEY' <<<"$over" || fail "gate.overTls renders no front key"
grep -q 'key_file: /var/lib/moor/config/gate-front-key' <<<"$over" || fail "gate.overTls leaves the authgear front local"
if grep -q -- '--front-socket=' <<<"$over"; then fail "gate.overTls still dials the front socket"; fi
if grep -q 'subPath: state/fronts/authgear' <<<"$over"; then fail "gate.overTls still mounts the volume"; fi
if helm template moor deploy/helm/moor -f deploy/helm/required-values.yaml "${gate_tls[@]}" 2>&1 | grep -qv 'gate.overTls needs egress.enabled'; then
  fail "gate.overTls without egress.enabled renders"
fi

# Every gate replica seals sign-in cookies with one shared key, whether given or made.
key_env() { grep -c 'name: MOOR_GATE_COOKIE_KEY' <<<"$1" || true; }
[ "$(key_env "$full")" = 1 ] || fail "the gate gets no cookie key with gate.login on"
grep -q 'cookie-key: "[A-Za-z0-9]\{64\}"' <<<"$full" || fail "no generated cookie key in the gate Secret"
given=$(helm template moor "${chart[@]}" --set gate.cookieKey=first-key-first-key-first-key-first\\,old-key)
grep -q 'cookie-key: "first-key-first-key-first-key-first,old-key"' <<<"$given" ||
  fail "gate.cookieKey does not reach the gate Secret"
login_off=$(helm template moor "${chart[@]}" --set gate.login=false)
[ "$(key_env "$login_off")" = 0 ] || fail "the gate gets a cookie key with gate.login off"
echo "chart-render: ok"
