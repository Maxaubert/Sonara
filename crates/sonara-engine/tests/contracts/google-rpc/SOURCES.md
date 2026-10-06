# google.rpc.Status: sources

- Source: https://github.com/googleapis/googleapis `google/rpc/status.proto` and `google/rpc/error_details.proto` (branch `master`, commit `7c0638cf8934`; error_details.proto last changed in `ebd1d23ac613`), fetched 2026-10-06; the HTTP mapping in AIP-193 https://google.aip.dev/193 (`{"error": {code, message, status, details}}`).
- Licence: Apache-2.0 (googleapis). Hand-transcribed to JSON Schema in the proto3 JSON form (lowerCamelCase names, `google.protobuf.Duration` as `"39s"`, int64 as a string).
- Kept: Status, ErrorInfo, RetryInfo, QuotaFailure (with `quotaId`), BadRequest, Help, LocalizedMessage. Used by `google` and `gemini`.
