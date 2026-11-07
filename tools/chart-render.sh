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
# The PodDisruptionBudget is opt-in and covers only pods that run as their own Deployment.
pdb_names() { awk '/^kind:/ { kind = $2; name = "" } kind == "PodDisruptionBudget" && /^  name:/ && name == "" { name = $2; print name }' | sort | tr '\n' ' '; }
pdb_on=(--set podDisruptionBudget.enabled=true)
[ -z "$(pdb_names <<<"$(helm template moor "${chart[@]}" "${pdb_on[@]}")")" ] ||
  fail "renders a PodDisruptionBudget for a gate and runner that share the controller pod"
gate_pdb=$(helm template moor "${chart[@]}" --set gate.separate=true --set egress.ledgerTls=null "${pdb_on[@]}")
[ "$(pdb_names <<<"$gate_pdb")" = "moor-gate " ] || fail "gate.separate with the budget on renders [$(pdb_names <<<"$gate_pdb")], want moor-gate"
[ "$(grep -c 'maxUnavailable: 1$' <<<"$gate_pdb")" -ge 1 ] || fail "the gate budget lacks maxUnavailable: 1"
both_pdb=$(helm template moor "${chart[@]}" --set gate.separate=true --set egress.ledgerTls=null --set runner.separate=true "${pdb_on[@]}")
[ "$(pdb_names <<<"$both_pdb")" = "moor-gate moor-runner " ] || fail "gate and runner separate render [$(pdb_names <<<"$both_pdb")] budgets"
one_pdb=$(helm template moor "${chart[@]}" --set gate.separate=true --set egress.ledgerTls=null --set runner.separate=true --set runner.replicas=1 "${pdb_on[@]}")
[ "$(pdb_names <<<"$one_pdb")" = "moor-gate " ] || fail "a single runner replica gets a budget that would block every drain"
off_pdb=$(helm template moor "${chart[@]}" --set gate.separate=true --set egress.ledgerTls=null --set runner.separate=true)
grep -q PodDisruptionBudget <<<"$off_pdb" && fail "renders a PodDisruptionBudget with the budget off"

bucket=(--set blobs.s3.endpoint=https://s3.example.com --set blobs.s3.bucket=moor --set blobs.s3.accessKey=ak --set blobs.s3.secretKey=sk)
stateless=$(helm template moor "${chart[@]}" "${bucket[@]}" --set slack.separate=true)
controller_pod=$(awk '
  /^kind:/ { kind = $2; name = "" }
  kind == "Deployment" && /^  name:/ && name == "" { name = $2 }
  kind == "Deployment" && name == "moor" { print }
' <<<"$stateless")
if grep -qE 'claimName: moor-state|name: state-aside' <<<"$controller_pod"; then
  fail "a controller with a bucket and a database still mounts the volume"
fi
if grep -qE '^  name: moor-state$' <<<"$stateless"; then fail "a controller without the volume still renders its PVC"; fi
grep -q 'rollingUpdate: { maxSurge: 1, maxUnavailable: 1 }' <<<"$controller_pod" ||
  fail "a controller without the volume does not roll with a surge"
grep -q 'key: blobs-secret-key' <<<"$controller_pod" || fail "the controller gets no bucket credentials"
grep -q 'kind: s3' <<<"$stateless" || fail "the controller config names no bucket"
if helm template moor "${chart[@]}" "${bucket[@]}" 2>&1 | grep -qv 'needs slack.separate'; then
  fail "a controller without the volume renders with the Slack sidecar"
fi
if helm template moor "${chart[@]}" "${bucket[@]}" --set slack.separate=true --set egress.enabled=false 2>&1 | grep -qv 'needs egress.enabled'; then
  fail "a controller without the volume renders with its own egress Proxy"
fi
if grep -qE 'blob-copy|kind: Job' <<<"$stateless"; then fail "renders the blob copy without blobs.copyFromVolume"; fi
copy=$(helm template moor "${chart[@]}" "${bucket[@]}" --set slack.separate=true --set blobs.copyFromVolume=true)
copy_job=$(awk '/^kind:/ { kind = $2 } kind == "Job" { print }' <<<"$copy")
grep -q 'claimName: moor-state, readOnly: true' <<<"$copy_job" || fail "the blob copy does not read the volume read-only"
grep -q -- '--ignore-existing' <<<"$copy_job" || fail "the blob copy may replace a blob the bucket already holds"
grep -qE '^  name: moor-state$' <<<"$copy" || fail "the blob copy renders without the volume it reads"
if grep -q 'claimName: moor-state' <<<"$(awk '/^kind:/ { kind = $2 } kind == "Deployment" { print }' <<<"$copy")"; then
  fail "the controller mounts the volume while its blobs are copied"
fi
on_volume=$(helm template moor deploy/helm/moor -f deploy/helm/required-values.yaml "${bucket[@]}")
grep -q 'claimName: moor-state' <<<"$on_volume" || fail "a SQLite controller with a bucket drops the volume"
grep -q 'type: Recreate' <<<"$on_volume" || fail "a SQLite controller with a bucket rolls"
echo "chart-render: ok"
