# Canonical root provenance

The address-review-comments skill owns this mapping. It serializes main's existing canonical root decisions after PASS 1 deduplication and PASS 3 adjudication. It never adds ratings, performs another deduplication, or changes an outcome.

The source JSON is an array of objects with exactly `claim_id`, `outcome`, and `sources`. Copy IDs and outcomes from the canonical triage; copy source assertions from sanitized review provenance. Outcomes are APPLY/REJECT/FLAG/WITHDRAWN. Empty sources mean unavailable reviewer attribution; never invent one. Sync-only wording corrections are not additional substantive bug roots.

A reviewer source is `lane@round:finding-id`. A runner source is `run@relative-report.json#exact-failing-test-id`. Scope comes from the adjudication's existing scope header. The consumer must verify referenced evidence and omit ambiguous links. A withdrawal serializes an existing withdrawal, not a new author rating.

Run `python3 <skill>/scripts/root_provenance.py --claims <canonical-claims.json> --triage <triage.md> --output <provenance.md>`. The input triage's existing outcome section must contain each claim ID as a top-level item. Append the emitted block unchanged to triage.md. The deterministic projector rejects duplicate claims/sources, unknown fields, unknown outcomes, and outcome disagreement. Main still owns all grouping and dispositions.

The public consumer API is `parse_provenance(text)`. It validates the owning section and returns canonical claim objects, or None when absent. The section has `claim_id | outcome | sources` columns under `## Root cause provenance`. The projector emits sources as a JSON array, preserving opaque failing IDs containing commas or pipes. Existing comma-separated source cells remain readable. No consumer defines another version of this contract.
