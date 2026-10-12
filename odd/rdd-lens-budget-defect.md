# A high-risk candidate cannot start a native review at any real size

Status: open, provider-side. Observed on `gentle-ai 4.0.0`. Blocks the native
review of any high-risk change whose reviewer evidence is not tiny.

## Symptom

`review start` refuses the candidate at preflight and creates no review
authority:

```json
{
  "schema": "gentle-ai.review-integration.failure/v2",
  "operation": "review.start",
  "phase": "preflight",
  "code": "lens_context_budget_exceeded",
  "message": "The candidate's complete reviewer evidence exceeds the native context budget, so no review authority was created; review this change as smaller candidates that each fit under it.",
  "mutation_outcome": "not_started",
  "lineage_id": "review-1c04dc741504d9a4",
  "next_action": "stop"
}
```

The provider's own remediation is to split the candidate. That advice cannot be
followed to success, because the constraint is not the candidate alone.

## Why splitting does not help

The installed binary carries both rules as strings, and they are mutually
exclusive in practice:

```
high-risk negotiated START requires canonical 4R lenses
medium-risk negotiated START requires one supported lens
```

A high-risk candidate must select all four lenses. Their *combined* reviewer
evidence is what exceeds the native context budget, so a high-risk candidate has
no admissible size: shrinking it does not change the lens count, and the only way
to leave the high tier is to remove what made it high-risk in the first place
(the new dependencies, or the code that starts processes).

`--focus` and `--policy` cannot change this shape either, which the binary states
directly:

```
frozen recovery review shape is incompatible with current repository risk;
evidence reuse cannot preserve that shape, and changing --focus or --policy
cannot make it compatible
```

## Reproduction

Seven attempts, all on branch `feat/tau-rust-port`.

| # | candidate | lens selection | result |
|---|---|---|---|
| 1 | `6ee432f..HEAD`, 19 files, 6626 lines | 4 lenses via the tool | `native-start-input-invalid` / `lenses-invalid`, `lineage_created: false` |
| 2 | same | tier-derived, no explicit lenses | `lens_context_budget_exceeded`, lineage `review-43971bb7411cdf4b` |
| 3 | same, after the candidate was rewritten | tier-derived | `lens_context_budget_exceeded`, lineage `review-1c04dc741504d9a4` |
| 4 | `6ee432f..2008425`, 9 files, 1960 lines | tier-derived | `lens_context_budget_exceeded` |
| 5 | same as 3 | native CLI `--focus=reliability --consent=relay` | **consent envelope returned** — see the trap below |
| 6 | same as 3 | native CLI, granted invocation as emitted | `lens_context_budget_exceeded` |
| 7 | same as 3 | native CLI, granted invocation plus `--focus=reliability` | `lens_context_budget_exceeded` |

Attempt 4 is the decisive one: it is a coherent slice of 1960 changed lines over
9 files, and it still fails. The earlier project history reviewed a 4715-line
candidate successfully on an earlier installed version, while the same directory
also holds backups of `3.7.0`, `2.4.0-rc.6`, `2.3.0` and `2.2.5`, so the budget
behaviour is version-dependent and this note is pinned to `4.0.0`.

## The consent-envelope trap

`--consent=relay` asks the provider for the typed consent question; it does not
grant anything. The question is emitted **before** the reviewer lens context is
materialized, so a `--consent=relay` run that returns a
`gentle-ai.review-integration.consent/v3` envelope proves only that the question
could be built. The refusal arrives later, when the human's answer is submitted.

Worse, the envelope's own choice invocations do not round-trip the flag that made
the envelope producible. Attempt 5 was run as the emitted command plus
`--focus=reliability`; the envelope's `granted` invocation omits that flag, and
running it verbatim (attempt 6) fails. A host that relays consent faithfully and
then executes the envelope's invocation cannot succeed.

## Consequence

For this repository, a high-risk change cannot obtain a native review receipt.
The work is recorded as `nativeReviewOutcome: unavailable` for its exact
candidate, and it is verified by the independent verifier the risk plan requires:
three rounds on T9, verdicts `fail`, `fail`, `pass`, with the two correction
rounds committed. Delivery under ordinary repository policy remains available and
is not silently approved:

```
gentle-ai review mode disable --scope clone --cwd <repo>
```
