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

{{/* Not moor.labels: the gate Service moves to these when the gate is its own Deployment. */}}
{{- define "moor.gateLabels" -}}
app.kubernetes.io/name: moor-gate
app.kubernetes.io/instance: {{ .Release.Name | quote }}
{{- end -}}

{{/* Non-empty when the gate is its own Deployment, which dials the controller's ledger. */}}
{{- define "moor.gateSeparate" -}}
{{- if and .Values.gate.enabled .Values.gate.separate -}}
{{- if not .Values.egress.enabled }}{{ fail "gate.separate needs egress.enabled: the gate dials the controller through the ledger listener" }}{{ end -}}
{{- if lt (int .Values.gate.drainSecs) 35 }}{{ fail "gate.drainSecs must outlast the gate's 30 s hold" }}{{ end -}}
true
{{- end -}}
{{- end -}}

{{/* Non-empty when the controller keeps no volume: Postgres holds its store and lock, a bucket its blobs. */}}
{{- define "moor.stateless" -}}
{{- if and .Values.database.url .Values.blobs.s3 -}}
{{- if and .Values.slack.enabled (not .Values.slack.separate) }}{{ fail "a controller without the volume needs slack.separate: the Slack sidecar keeps slack.db on it" }}{{ end -}}
{{- if not .Values.egress.enabled }}{{ fail "a controller without the volume needs egress.enabled: its own egress Proxy keeps the key Box tokens are signed with on it" }}{{ end -}}
true
{{- end -}}
{{- end -}}

{{/* The gate container, beside the controller or in its own Deployment. */}}
{{- define "moor.gateContainer" -}}
- name: gate
  image: {{ include "moor.image" . }}
  imagePullPolicy: {{ .Values.image.pullPolicy }}
  securityContext: {{- include "moor.gateSecurity" . | nindent 4 }}
  command: [moor-gate]
  env:
    - { name: MOOR_GATE_DRAIN_TIMEOUT_SECS, value: {{ (ternary .Values.gate.drainSecs .Values.controller.drainTimeoutSecs (not (empty (include "moor.gateSeparate" .)))) | quote }} }
    {{- if include "moor.gateSeparate" . }}
    # The gate reads --front-key from here, so the key stays out of the process list.
    - name: MOOR_GATE_FRONT_KEY
      valueFrom:
        secretKeyRef: { name: {{ .Release.Name }}-egress, key: gate-front-key }
    {{- end }}
    {{- if .Values.gate.login }}
    - name: MOOR_GATE_COOKIE_KEY
      valueFrom:
        secretKeyRef: { name: {{ .Release.Name }}-gate, key: cookie-key }
    {{- end }}
  args:
    - --listen=0.0.0.0:{{ .Values.gate.port }}
    {{- if include "moor.gateSeparate" . }}
    - --controller=tls://{{ include "moor.ledgerHost" . }}:{{ .Values.egress.ledgerPort }}
    - --controller-ca=/var/lib/moor/gate/ledger-ca.pem
    - --controller-cert=/var/lib/moor/gate/gate-client.crt
    - --controller-key=/var/lib/moor/gate/gate-client.key
    {{- else }}
    - --front-socket=/var/lib/moor/front/front
    {{- end }}
    - --issuer={{ required "authgear.issuer is required" .Values.authgear.issuer }}
    - --client-id={{ required "authgear.clientId is required" .Values.authgear.clientId }}
    {{- with .Values.gate.jwksUrl }}
    - --jwks-url={{ . }}
    {{- end }}
    {{- if .Values.mcp.enabled }}
    - --mcp-resource-url={{ required "mcp.resourceUrl is required" .Values.mcp.resourceUrl }}
    {{- end }}
    {{- if .Values.gate.login }}
    - --public-url=https://{{ required "host is required for gate.login" .Values.host }}
    {{- end }}
  ports:
    - { name: gate, containerPort: {{ .Values.gate.port }} }
  {{- if include "moor.gateSeparate" . }}
  # The Service and the ingress take the pod out before it stops accepting.
  lifecycle:
    preStop:
      exec:
        command: [sleep, "10"]
  {{- end }}
  {{- /* Must not dial the controller: both gates would go unready in a gap, and nothing would hold. */}}
  readinessProbe:
    httpGet: { path: /.well-known/moor, port: gate }
    periodSeconds: 5
  {{- with .Values.gate.resources }}
  resources: {{- toYaml . | nindent 4 }}
  {{- end }}
  volumeMounts:
    {{- if include "moor.gateSeparate" . }}
    - { name: gate-tls, mountPath: /var/lib/moor/gate, readOnly: true }
    {{- else }}
    - { name: state, mountPath: /var/lib/moor/front, subPath: state/fronts/authgear, readOnly: true }
    {{- end }}
{{- end -}}

{{/* Non-empty when the runner is its own Deployment, which dials the controller's ledger. */}}
{{- define "moor.runnerSeparate" -}}
{{- if and .Values.runner.enabled .Values.runner.separate -}}
{{- if not .Values.egress.enabled }}{{ fail "runner.separate needs egress.enabled: the runner dials the controller through the ledger listener and the machine service key" }}{{ end -}}
true
{{- end -}}
{{- end -}}
