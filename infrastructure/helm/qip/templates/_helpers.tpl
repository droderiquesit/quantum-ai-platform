{{/*
A digest, refused at render time if it is absent or shaped like a tag.

Binary Authorization's admission webhook does not merely prefer a digest
reference, it refuses to evaluate anything else: ".github/workflows/deploy.yml"
(commit 28e005a) exists because a manifest asking for "<image>:<commit-sha>"
got every pod denied with "Expected digest with sha256 scheme, but got tag or
malformed digest" — the attestor signed the bytes at "@sha256:…", and a tag
between promotion and apply can move out from under what was attested. Kargo
promotes exactly this shape (a set of per-binary digests), so a value that is
present but tag-shaped is a caller error this chart can catch before the
cluster does, the same way `required` already catches one that is absent.

Usage: {{ include "qip.digest" (dict "value" .Values.images.api "name" "images.api") }}
*/}}
{{- define "qip.digest" -}}
{{- $val := required (printf "%s is required" .name) .value -}}
{{- if not (hasPrefix "sha256:" $val) -}}
{{- fail (printf "%s must be a digest (\"sha256:<hex>\"), not %q — Binary Authorization's admission webhook refuses to evaluate a tag reference" .name $val) -}}
{{- end -}}
{{- $val -}}
{{- end -}}
