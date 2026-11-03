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
