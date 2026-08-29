{{- define "housebot.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "housebot.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{- define "housebot.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "housebot.labels" -}}
helm.sh/chart: {{ include "housebot.chart" . }}
{{ include "housebot.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- with .Values.commonLabels }}
{{ toYaml . }}
{{- end }}
{{- end -}}

{{- define "housebot.selectorLabels" -}}
app.kubernetes.io/name: {{ include "housebot.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "housebot.bot.selectorLabels" -}}
{{ include "housebot.selectorLabels" . }}
app.kubernetes.io/component: bot
{{- end -}}

{{- define "housebot.sandboxApi.selectorLabels" -}}
{{ include "housebot.selectorLabels" . }}
app.kubernetes.io/component: sandbox-api
{{- end -}}

{{- define "housebot.sandboxApi.fullname" -}}
{{ include "housebot.fullname" . }}-sandbox-api
{{- end -}}

{{- define "housebot.postgres.fullname" -}}
{{ include "housebot.fullname" . }}-pg
{{- end -}}

{{/*
The ServiceAccount sandbox Pods run as. The name is compiled into the sandbox
crate (kubernetes.rs SERVICE_ACCOUNT), so it is deliberately not derived from
the release name — renaming it here would leave sandbox Pods unschedulable.
*/}}
{{- define "housebot.sandbox.serviceAccountName" -}}
housebot-sandbox
{{- end -}}

{{- define "housebot.image" -}}
{{- $tag := .image.tag | default .root.Chart.AppVersion -}}
{{- printf "%s:%s" .image.repository $tag -}}
{{- end -}}

{{/*
The bearer token shared by the bot and the sandbox API. An operator-supplied
Secret wins; otherwise the chart generates one on first install and reuses the
live value on upgrade, so a helm upgrade never silently rotates the token out
from under a running bot.
*/}}
{{- define "housebot.sandboxApi.secretName" -}}
{{- if .Values.sandboxApi.auth.existingSecret -}}
{{- .Values.sandboxApi.auth.existingSecret -}}
{{- else -}}
{{- include "housebot.sandboxApi.fullname" . -}}
{{- end -}}
{{- end -}}

{{- define "housebot.sandboxApi.token" -}}
{{- if .Values.sandboxApi.auth.token -}}
{{- .Values.sandboxApi.auth.token -}}
{{- else -}}
{{- $name := include "housebot.sandboxApi.fullname" . -}}
{{- $existing := lookup "v1" "Secret" .Release.Namespace $name -}}
{{- if and $existing $existing.data.token -}}
{{- index $existing.data "token" | b64dec -}}
{{- else -}}
{{- randAlphaNum 48 -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/*
The Secret holding the libpq URI. A CNPG cluster name resolves to the
`<name>-app` Secret the operator publishes; otherwise an explicit Secret.
*/}}
{{- define "housebot.databaseSecretName" -}}
{{- if and .Values.database.cnpgCluster .Values.database.existingSecret -}}
{{- fail "set only one of database.cnpgCluster or database.existingSecret" -}}
{{- else if .Values.database.cnpgCluster -}}
{{- printf "%s-app" .Values.database.cnpgCluster -}}
{{- else -}}
{{- required "one of database.cnpgCluster or database.existingSecret is required" .Values.database.existingSecret -}}
{{- end -}}
{{- end -}}

{{- define "housebot.databaseSecretKey" -}}
{{- if .Values.database.cnpgCluster -}}uri{{- else -}}{{ .Values.database.secretKey }}{{- end -}}
{{- end -}}
