# Kimi terminal error fixtures

Recorded with Kimi 2.4.1 on 2026-11-02. The JSONL rows preserve physical append order and native timestamps; the late inner event has an earlier timestamp than the root commit. The original prompt text is replaced with `Bus task`. OAuth addresses are replaced with RFC 5737 addresses. These are sanitized reductions, not byte-exact native captures.

- `kimi-oauth-error.jsonl`: private capture SHA256 `5fa598173ab6a8207a4f50ae4e07dbf8a7930852aaff7c3949ef1930499b5870`, original physical lines299/491/494/548.
- `kimi-quota-error.jsonl`: private capture SHA256 `f823186e9e85a3b99fd5ff28252644809865ae758d5cf2e9aadc117de8448708`, original physical lines8/359/362/461. Inner `errorMessage` is the recorded literal `[object Object]`; the root error contains the quota name, message and HTTP429 metadata.

Private source/provenance is retained under the main checkout's `temp/kimi-request-lifecycle/evidence/` and `temp/codexretry2/`. No credentials were copied.
