# RDD targeted-validator binding is rejected as stale

Status: open, provider-side. Blocks closing any review transaction that required a correction.

## Symptom

After a bounded correction is applied and committed, STATUS asks for a targeted
validator:

```
"next_transition": { "kind": "collect", "reason_code": "targeted_validation_required" }
```

and issues one `provider_targeted_validator` collect binding. Capturing that
binding always fails:

```
{"tool":"gentle_review_capture","status":"blocked",
 "outcome":"capture-binding-rejected",
 "reason":"collectBinding is missing or stale for current STATUS",
 "mutation_performed":false,"mutation_outcome":"none"}
```

The result is that a review which finds a severe finding can never reach a
terminal state: the correction is applied, the candidate tree changes, and the
validation that would close the transaction cannot be submitted.

## Reproduction

Two independent lineages, different targets, different revisions, identical
failure.

| | lineage | correction commit | candidate tree |
|---|---|---|---|
| T7 session storage | `review-d12fce92d6b852ed` | `edc5569` | `0b1f83d1` → `04771b70` |
| Agent core | `review-c5a29792348d5f1c` | `a311b83` | `04771b70` → `0b1f83d1` |

Both were submitted through `gentle_review_capture` with the binding exactly as
STATUS issued it.

## The asymmetry

Within a single STATUS response, the binding and the authority disagree about
which revision is current.

T7 lineage:

```
"authority": { "revision": "sha256:d6a383f63cd8e77b3fd2e4d0f92e4f2ca0525ce42420c3f7736fbf0462cc0a04" }
"validation_request": { "expected_revision": "sha256:8551191ecee682c34fefb767a9bc6ba5ec738eccf0a0ffc2db4144e55b9d3093" }
```

Agent-core lineage:

```
"authority": { "revision": "sha256:0b1ece7c171ac8c9a9361cf2897efc3585be34fb23e0b9533f52d72e8f3d3427" }
"validation_request": { "expected_revision": "sha256:4ad5fc75ab5a20224766e1a47ea8d50ff50d0d52ef3d8559ba2ba67239bca144" }
```

The binding carries the session/store revision, while `authority.revision` still
reports the pre-correction one. The capture validator appears to compare against
`authority.revision`, so a binding the provider itself just issued is rejected by
the provider's own validator. The `request_hash` and `repository_context` are
issued consistently with the binding, so the revision is the only disagreeing
field.

## What was tried

- The binding passed as the serialized string exactly as STATUS emitted it.
- The binding passed as a parsed JSON object, to remove one escaping layer.
- A freshly fetched STATUS immediately before each attempt, to rule out staleness
  between calls.
- One attempt with the extra `validationRequest` field included, one without.

All produce the same rejection. Earlier captures in the same lineages — the
reviewer group, the refuter, and the correction plan — all succeeded with the
same call shape, so the difference is specific to the validator slot.

## Reference captures that did work

For contrast, in the agent-core lineage these all returned successfully:
`reviewer_results_required` (group of four, forecast then acknowledged),
`provider_refuter_required`, and `correction_plan_required`
(`correction_lines: 150` accepted).

## Impact

`main` cannot receive a review that reports itself closed, because the step that
closes it is unreachable. The corrections themselves are applied, committed and
verified by the project's own checks — tests, `fmt`, `clippy`, and the Linux
target through CI — but that is verification, not the approval RDD is meant to
provide. Until this is fixed, treat every correction as verified-but-unapproved.
