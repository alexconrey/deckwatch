# API Reference

The live Deckwatch binary hosts an interactive Swagger UI at
[`/api/docs`](/api/docs) and the raw OpenAPI 3.0 spec at
[`/api/openapi.yaml`](/api/openapi.yaml).

The spec is the source of truth for:

- request / response shapes
- HTTP status codes
- streaming endpoints (SSE for logs, WebSocket for exec)
- the OCI Distribution Spec v1.1 surface under `/v2/*`

Refer to it directly rather than duplicating it here — the markdown will drift.
