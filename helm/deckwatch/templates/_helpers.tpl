{{/*
Expand the name of the chart.
*/}}
{{- define "deckwatch.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "deckwatch.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "deckwatch.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "deckwatch.labels" -}}
helm.sh/chart: {{ include "deckwatch.chart" . }}
{{ include "deckwatch.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "deckwatch.selectorLabels" -}}
app.kubernetes.io/name: {{ include "deckwatch.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use
*/}}
{{- define "deckwatch.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "deckwatch.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Public URL for the embedded OCI registry. Falls back to the in-cluster
Service DNS name if the user didn't set registry.publicUrl explicitly —
this is the URL fed to kaniko's --destination flag and shown in the
"Deckwatch Registry (local)" dropdown entry.
*/}}
{{- define "deckwatch.registryPublicUrl" -}}
{{- if .Values.registry.publicUrl -}}
{{- .Values.registry.publicUrl -}}
{{- else -}}
{{- printf "%s-registry.%s.svc.cluster.local:%d" (include "deckwatch.fullname" .) .Release.Namespace (int .Values.registry.service.port) -}}
{{- end -}}
{{- end }}
