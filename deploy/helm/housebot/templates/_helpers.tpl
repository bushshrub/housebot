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
Exactly one of the four database sources must be set. Checking it in one place
keeps every consumer below free to assume a single answer.
*/}}
{{- define "housebot.database.source" -}}
{{- $sources := list -}}
{{- if .Values.database.url }}{{ $sources = append $sources "url" }}{{ end -}}
{{- if .Values.database.existingSecret }}{{ $sources = append $sources "existingSecret" }}{{ end -}}
{{- if .Values.database.cnpgCluster }}{{ $sources = append $sources "cnpgCluster" }}{{ end -}}
{{- if .Values.database.cnpg.create }}{{ $sources = append $sources "cnpg.create" }}{{ end -}}
{{- if .Values.database.host }}{{ $sources = append $sources "host" }}{{ end -}}
{{- if gt (len $sources) 1 -}}
{{- fail (printf "set exactly one database source, got: %s" (join ", " $sources)) -}}
{{- else if eq (len $sources) 0 -}}
{{- fail "set exactly one of database.url, database.existingSecret, database.host, database.cnpgCluster or database.cnpg.create" -}}
{{- else -}}
{{- first $sources -}}
{{- end -}}
{{- end -}}

{{/*
The CNPG Cluster the bot talks to, whether the chart creates it or not. Empty
when Postgres is not CloudNativePG, which is what gates the NetworkPolicy rule
selecting on the operator's own label.
*/}}
{{- define "housebot.database.cnpgClusterName" -}}
{{- if .Values.database.cnpg.create -}}
{{- .Values.database.cnpg.name -}}
{{- else -}}
{{- .Values.database.cnpgCluster -}}
{{- end -}}
{{- end -}}

{{/*
The bot's DATABASE_URL env entry. Every source but `host` resolves to a Secret
holding a ready-made URI. `host` instead assembles one at runtime from
Kubernetes' own $(VAR) interpolation, so the password reaches the process from
its Secret without ever being rendered into a manifest or into `helm get values`.
*/}}
{{- define "housebot.databaseEnv" -}}
{{- if eq (include "housebot.database.source" .) "host" -}}
{{- $db := .Values.database }}
{{- if not $db.passwordSecret.name }}
{{- fail "database.host needs database.passwordSecret.name, the Secret holding the password for database.user" }}
{{- end }}
- name: DATABASE_PASSWORD
  valueFrom:
    secretKeyRef:
      name: {{ $db.passwordSecret.name }}
      key: {{ $db.passwordSecret.key }}
- name: DATABASE_URL
  value: postgres://{{ $db.user }}:$(DATABASE_PASSWORD)@{{ $db.host }}:{{ $db.port }}/{{ $db.name }}?sslmode={{ $db.sslMode }}
{{- else }}
- name: DATABASE_URL
  valueFrom:
    secretKeyRef:
      name: {{ include "housebot.databaseSecretName" . }}
      key: {{ include "housebot.databaseSecretKey" . }}
{{- end }}
{{- end -}}

{{/*
Host and port to probe before starting the bot. Empty when the connection
string is opaque to the chart (`url` and `existingSecret` hide it inside a
Secret), which is what disables the wait entirely.
*/}}
{{- define "housebot.database.waitHost" -}}
{{- $source := include "housebot.database.source" . -}}
{{- if eq $source "host" -}}
{{- .Values.database.host -}}
{{- else if or (eq $source "cnpgCluster") (eq $source "cnpg.create") -}}
{{- printf "%s-rw" (include "housebot.database.cnpgClusterName" .) -}}
{{- end -}}
{{- end -}}

{{- define "housebot.database.waitPort" -}}
{{- if eq (include "housebot.database.source" .) "host" -}}
{{- .Values.database.port -}}
{{- else -}}
5432
{{- end -}}
{{- end -}}

{{/*
The Secret holding the libpq URI.
*/}}
{{- define "housebot.databaseSecretName" -}}
{{- $source := include "housebot.database.source" . -}}
{{- if eq $source "url" -}}
{{- include "housebot.fullname" . }}-database
{{- else if eq $source "existingSecret" -}}
{{- .Values.database.existingSecret -}}
{{- else -}}
{{- printf "%s-app" (include "housebot.database.cnpgClusterName" .) -}}
{{- end -}}
{{- end -}}

{{- define "housebot.databaseSecretKey" -}}
{{- if eq (include "housebot.database.source" .) "existingSecret" -}}
{{- .Values.database.secretKey -}}
{{- else -}}
uri
{{- end -}}
{{- end -}}
