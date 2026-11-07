{{- define "moor.labels" -}}
app.kubernetes.io/name: moor
app.kubernetes.io/instance: {{ .Release.Name | quote }}
{{- end -}}

{{- define "moor.image" -}}
{{ .Values.image.repository }}:{{ .Values.image.tag | default .Chart.AppVersion }}
{{- end -}}

{{/* The front shim needs pivot_root, which RuntimeDefault seccomp refuses; the pod's user namespace still maps root to an unprivileged host uid. */}}
{{- define "moor.shimSecurity" -}}
allowPrivilegeEscalation: false
procMount: Unmasked
seccompProfile:
  type: Unconfined
capabilities:
  drop: [ALL]
{{- end -}}

{{- define "moor.containerSecurity" -}}
allowPrivilegeEscalation: false
procMount: Unmasked
capabilities:
  drop: [ALL]
  add: [SYS_ADMIN]
{{- end -}}

{{/* The gate only serves HTTP and dials a socket: no capability, no seccomp exception. */}}
{{- define "moor.gateSecurity" -}}
allowPrivilegeEscalation: false
readOnlyRootFilesystem: true
seccompProfile:
  type: RuntimeDefault
capabilities:
  drop: [ALL]
{{- end -}}

{{/* Not moor.labels: the gate Service and the controller Deployment select on those. */}}
{{- define "moor.egressLabels" -}}
app.kubernetes.io/name: moor-egress
app.kubernetes.io/instance: {{ .Release.Name | quote }}
{{- end -}}

{{/* Where Boxes dial the egress Proxy: egress.url, else /egress on the gate ingress, which must then exist. */}}
{{- define "moor.egressUrl" -}}
{{- if .Values.egress.url -}}
{{- if not (hasPrefix "wss://" .Values.egress.url) -}}
{{ fail (printf "egress.url %q must be a wss:// address (the controller refuses any other scheme and crash-loops); leave it empty to use wss://<host>/egress" .Values.egress.url) }}
{{- end -}}
{{ .Values.egress.url }}
{{- else if and .Values.gate.enabled .Values.gate.ingress.enabled -}}
wss://{{ required "host is required" .Values.host }}/egress
{{- else -}}
{{ fail "egress.enabled needs gate.ingress.enabled, which serves /egress, or egress.url naming where Boxes reach the egress Service" }}
{{- end -}}
{{- end -}}

{{/* The name the ledger certificate is issued for and the egress pods dial. */}}
{{- define "moor.ledgerHost" -}}
{{ .Release.Name }}-ledger.{{ .Release.Namespace }}.svc
{{- end -}}

{{/* Not moor.labels: the controller pod and its Services select on those. */}}
{{- define "moor.runnerLabels" -}}
app.kubernetes.io/name: moor-runner
app.kubernetes.io/instance: {{ .Release.Name | quote }}
{{- end -}}

{{/* Not moor.labels: the controller pod and its Services select on those. */}}
{{- define "moor.slackLabels" -}}
app.kubernetes.io/name: moor-slack
app.kubernetes.io/instance: {{ .Release.Name | quote }}
{{- end -}}

{{/* Non-empty when Slack is its own Deployment, which dials the ledger and the shared egress Proxy. */}}
{{- define "moor.slackSeparate" -}}
{{- if and .Values.slack.enabled .Values.slack.separate -}}
{{- if not .Values.egress.enabled }}{{ fail "slack.separate needs egress.enabled: Slack dials the controller through the ledger listener and spends credentials through the shared egress Proxy" }}{{ end -}}
true
{{- end -}}
{{- end -}}

{{/* Non-empty when the runner is its own Deployment, which dials the controller's ledger. */}}
{{- define "moor.runnerSeparate" -}}
{{- if and .Values.runner.enabled .Values.runner.separate -}}
{{- if not .Values.egress.enabled }}{{ fail "runner.separate needs egress.enabled: the runner dials the controller through the ledger listener and the machine service key" }}{{ end -}}
true
{{- end -}}
{{- end -}}
