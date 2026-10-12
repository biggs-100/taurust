# taurust — port of tau (Python) to Rust, with the gentle-shell visual experience

Port the Python coding agent `tau` to Rust in `C:\Users\USER\Desktop\dev-biggz\taurust`,
and make its terminal shell look and behave like `gentle-shell` (minus the startup banner).

Sources read-only for reference: `C:\Users\USER\Desktop\herramientas\tau` (Python, ~58k LOC)
and `C:\Users\USER\Desktop\herramientas\gentle-shell` (TypeScript Pi package).

## Specs

- **S1** — Rewrite `tau` in Rust. User, verbatim: *"quiro rehacer tau pero con rust y la shell que se vea como gentle-shell  C:\Users\USER\Desktop\herramientas\gentle-shell  C:\Users\USER\Desktop\herramientas\tau"*
- **S2** — The shell must look like `gentle-shell`. User, verbatim: *"y la shell que se vea como gentle-shell"*.
- **S3** — Scope is the complete thing, both sides. User, verbatim: *"completo"*.
- **S4** — One explicit exclusion: the startup banner. User, verbatim: *"menos el banner"*.
- **S5** — The gentle-shell todo panel is explicitly in scope. User, verbatim: *"y el todo de gentle shell"*.
- **S6** — Destination and layout, stated by the orchestrator and not corrected by the user: a new Rust repository in `C:\Users\USER\Desktop\dev-biggz\taurust`, as a Cargo workspace with one crate per tau layer — `tau-ai`, `tau-agent`, `tau-coding` — plus a separate `tau-tui` crate. This preserves tau's documented architectural principle `tau_coding -> tau_agent -> tau_ai`.

## Design

### Reference: the Python source

| Layer | Files | LOC | Role |
|---|---|---|---|
| `tau_ai` | 20 | 5,456 | Provider-neutral streaming: providers, content, events, retry, HTTP |
| `tau_agent` | 16 | 2,367 | Portable brain: messages, types, tools, loop, harness, session |
| `tau_coding` | 82 | 50,105 | Coding app: CLI, TUI (16,506 LOC), tools, sessions, skills, extensions |
| tests | 74 | — | pytest, per layer |

Runtime dependencies of `tau` (from `tau/pyproject.toml:12-23`): `anyio`, `httpx[socks]`,
`packaging`, `pillow`, `pydantic`, `pygments`, `rich`, `textual`, `typer`.
Entry point: `tau = "tau_coding.cli:app"` (`tau/pyproject.toml:32`), a `typer` app.

### Reference: the visual target

`gentle-shell` is not a shell of its own; it is a Pi package that overlays the visual
identity on Pi's TUI. The identity to reproduce lives in:

- `themes/Gentle.json` — the palette and the semantic role map (plus `Gentleman-Cute.json`,
  `Gentleman-Sexy.json`).
- `lib/shell-*.ts` — the components: bar, prompt, sidebar, sidebar-layout, card, gauge,
  todo, changes, usage.
- `lib/terminal-theme.ts`, `lib/native-choice-list.ts`, `lib/command-palette.ts`,
  `lib/animation-policy.ts`, `lib/visual-*.ts`.

Out of scope by S4: the startup banner.

### Rust architecture

```text
tau-tui  ->  tau-coding  ->  tau-agent
tau-coding, tau-agent, tau-ai  ->  tau-protocol
```

The boundary rule from `tau/AGENTS.md`: the core must not know about the terminal,
local config paths, slash commands or rendering. Frontends consume events.

**Layer invariant (from the reference).** `tau_agent` must never import `tau_ai`;
tau asserts this in `tau/tests/test_pi_event_protocol.py:34` by scanning its own
sources. The Rust crate graph enforces it structurally: a provider is consumed
through the `tau-protocol` trait, never by name, so `tau-agent` declares no
dependency on `tau-ai`. An earlier `Cargo.toml` here did declare it; that was a
real violation, found only by consulting the reference.

**Deviation from S6, deliberate.** The agreed layout was four crates. A fifth was
added, `tau-protocol`, holding the shared vocabulary. Reason: in tau the provider
contract (`ModelProvider`, the canonical assistant events) is owned by
`tau_agent`, while the adapters live in `tau_ai` — so `tau_ai` imports
`tau_agent` (`tau/src/tau_ai/events.py:3`, `tau/src/tau_ai/provider.py:3`,
`tau/src/tau_ai/fake.py:9-11`). A Rust crate graph cannot reproduce that arrow
without inverting the layer meaning, so the vocabulary became a leaf that both
sides depend on. The four agreed crates keep their names and responsibilities.

### The wire contract (verified, not guessed)

Checked against `tau/tests/test_agent_types.py`, which is the Python original's
own contract test:

- Field names are camelCase (`toolCallId`, `thoughtSignature`, `timeToFirstOutputMs`).
- `None` fields are **serialized as `null`**, not omitted (`thoughtSignature: null`).
- Unknown fields are **rejected** on deserialization.
- `UserMessage.content` is a bare JSON string for text-only input and an array once
  an image is attached.
- Every message carries its own `role` marker, so a message stored alone stays
  self-describing.
- One trap: Python's `_to_camel` turns `cache_write_1h` into `cacheWrite1H`, which
  Rust's `rename_all = "camelCase"` would render as `cacheWrite1h`. Handled with an
  explicit rename.
- Two decisions forced by Rust's serializer: `AgentMessage` is an **untagged** enum
  (serde cannot combine an internally tagged enum with `deny_unknown_fields`, and
  the `role` key has to remain a real field), and the assistant variant is **boxed**
  (368 bytes against 144 for the next largest variant, in a transcript held in memory).
- Events are deliberately **not** strict. Serde silently ignores
  `deny_unknown_fields` on an internally tagged enum. This was probed, not assumed:
  the strict test written to confirm the attribute failed. The real behaviour is now
  pinned by a test — an event tolerates an unknown field, a stored message rejects one.
  Leniency is right for a transient wire event: a provider that starts sending a new
  field must not break the stream.
- The nested provider event inside `message_update` is boxed, because that event is
  produced once per streamed token and would otherwise carry roughly half a kilobyte.

### Portability deviations

Two parts of the reference are not portable as written, both for the same reason:
Rust marks environment mutation as `unsafe` in edition 2024, and this workspace
forbids unsafe code.

| tau | taurust | Why |
|---|---|---|
| `http.py` rewrites `os.environ` around client construction so a generic `socks://` proxy becomes `socks5://`, then restores it | `http::normalize_proxy_url` is a pure function applied where the proxy is read | Mutating process state around a construction races against every other thread |
| `env.py` reads `os.environ` directly; its tests monkeypatch the environment | Reading goes through an `EnvSource`; `MapEnv` serves tests and injected configuration | Test isolation without global mutable state |

Both changes also make the port testable without process-global side effects, which
is why they are kept rather than worked around.

> The remaining per-module maps (JSONL record shape, CLI surface, TUI widget
> inventory) are recorded here before the tasks that depend on them start.

## Tasks

- T1 — Workspace foundation: `git init`, Cargo workspace, crate skeletons, lint/format config, CI, README, AGENTS.md · Spec: S1,S6 · Route: inline · Commit: `dcc7382`
- T2 — `tau-protocol`: content blocks, transcript messages, usage and timing, Pi wire fidelity · Spec: S1 · Route: inline · Commit: `b9ae762`
- T3 — `tau-protocol`: streaming events (assistant and agent), the `ModelProvider` trait; the fake provider lives in `tau-ai` · Spec: S1 · Route: inline · Commit: `5976a91`
- T4 — `tau-ai`: error taxonomy, retry/backoff policy, proxy normalization, configuration reading; the HTTP transport binding moved into T20 · Spec: S1 · Route: inline · Commit: `31cda32`
- T5 — `tau-agent`: tool execution protocol and tool registry · Spec: S1 · Route: inline · Commit: `6fafd6b`
- T6 — `tau-agent`: agent loop and harness · Spec: S1 · Route: inline · Commit: `e6cea1f`, `625825a`, `bee74bc`
- T7 — `tau-agent`: session storage with the JSONL record format · Spec: S1 · Route: inline · Commit: `7201283`, `6a77906`
- T8 — `tau-coding`: paths, provider config resolution, credential store · Spec: S1 · Route: worker + parent verification · Commit: `dd3cd30`, `f56178b`
- T9 — `tau-coding`: built-in tool implementations (read, write, edit, shell, search) · Spec: S1 · Route: pending · Commit: pending
- T10 — `tau-coding`: CLI surface with parity to the typer app · Spec: S1 · Route: pending · Commit: pending
- T11 — `tau-coding`: context and system-prompt assembly, project trust, skills, extensions · Spec: S1 · Route: pending · Commit: pending
- T12 — `tau-tui`: primitives — theme roles, layout, width measurement, box drawing, ANSI · Spec: S2,S3 · Route: pending · Commit: pending
- T13 — `tau-tui`: prompt chrome, idle/working/queued states, working-label animation · Spec: S2,S3 · Route: pending · Commit: pending
- T14 — `tau-tui`: sidebar with Status / Project / Changes / Integrations · Spec: S2,S3 · Route: pending · Commit: pending
- T15 — `tau-tui`: todo panel · Spec: S5 · Route: pending · Commit: pending
- T16 — `tau-tui`: tool cards, output rendering, diff view · Spec: S2,S3 · Route: pending · Commit: pending
- T17 — `tau-tui`: gauges, usage view, stats view · Spec: S2,S3 · Route: pending · Commit: pending
- T18 — `tau-tui`: command palette, native choice lists, overlays · Spec: S2,S3 · Route: pending · Commit: pending
- T19 — `tau-tui`: keybindings and visual configuration · Spec: S2,S3 · Route: pending · Commit: pending
- T20 — Providers: anthropic, openai-compatible, openai-codex, google, mistral, plus OAuth flows and the HTTP transport binding · Spec: S1 · Route: pending · Commit: pending
- T21 — Sessions: export, reload, extensions, RPC surface, and the deferred legacy Tau-v1 record migration · Spec: S1 · Route: pending · Commit: pending
- T22 — Parity gate: behaviour checklist run against the Rust binary, side by side with tau · Spec: S1,S3,S4 · Route: pending · Commit: pending

## Progress

- T1 done. Four crates, `unsafe_code` forbidden workspace-wide, CI running fmt +
  clippy + tests, `--version`/`--help`/no-args/unknown-argument behaviour covered by
  4 tests. RED observed before the implementation (unresolved imports). `fmt` and
  `clippy` clean, binary smoke-tested: `tau 0.1.0` exit 0, unknown flag exit 2.
  Deliberate omission: no `rust-toolchain.toml`, so no toolchain is force-downloaded
  on every invocation; the floor is pinned by `rust-version = "1.85"` in the workspace.
- T2 done. `tau-protocol` with `content`, `message`, `json`; 10 wire-format tests
  mirroring `tau/tests/test_agent_types.py`. RED observed, then GREEN on the first
  implementation pass. One clippy finding (`large_enum_variant`) fixed by boxing
  rather than by silencing, because CI runs with `-D warnings`.
- T3 done. 11 tests added (7 event-shape, 4 provider), 26 tests in the suite,
  clippy clean. RED observed in both crates before implementing. The fake provider
  went to `tau-ai`, matching `tau_ai/fake.py`, rather than to `tau-protocol`.
  Cancellation is checked before every event and the stream is hand-written, which
  removed the `async-stream` dependency.
- Reference-first paid off twice in this task alone: it caught the real layer
  violation in `tau-agent`, and it is why the `deny_unknown_fields` claim was probed
  instead of trusted.
- T4 done. 34 tests added, 60 in the suite, clippy clean. RED observed before
  implementing. Checked the reference twice against my own work and it was wrong
  both times: the `"{}"` error-body case (my assertion contradicted tau, and tau's
  behaviour was kept) and the retry-event accessors. Two deliberate portability
  deviations recorded above.
- T4 partial by design: the HTTP transport binding is deferred to T20, where a
  real caller and tau's request construction are available. The whole remaining
  surface of T4 — the policy that decides when to retry, what to say and how to
  read configuration — is done and under test.
- Published at <https://github.com/biggs-100/taurust> (public), default branch `main`,
  with `feat/tau-rust-port` as the working branch. Publishing surfaced a real gap:
  `Cargo.toml` declared `license = "MIT"` but no `LICENSE` file existed. Because the
  repository is public and this is a derivative work, both upstream MIT notices
  were reproduced in `THIRD-PARTY-NOTICES.md` before the first push, read from the
  upstream `LICENSE` files rather than recalled.
- T5 done. 17 tests added, 77 in the suite, clippy clean, RED observed first.
  Consulting the reference corrected a wrong assumption: `execution_mode` and
  `prepare_arguments` are declared on every tool in tau but its own loop never
  reads them, so they are ported as declarations with no scheduler pretending to
  use them.
- T6 done, delivered as three commits because the reference spreads this over
  ~800 lines across `loop.py`, `harness.py` and `tool_history.py`: `e6cea1f`
  provider-safe tool history repair (8 tests mirroring
  `tau/tests/test_tool_history.py`), `625825a` the agent loop (11 tests),
  `bee74bc` the harness (9 tests). 105 tests in the suite, clippy clean.
  TDD order was inverted for the harness alone: the implementation was written
  first, because the design had to be settled before tests could be written
  against it — shared state behind an `Arc`, and a `Drop` guard for the
  cancellation repair, since Rust cannot await inside `Drop`. The tests are
  therefore verification rather than observed-RED evidence, which is stated here
  instead of implied.
- CI gap found and closed. The workflow only ran on a push to `main` and on pull
  requests, so every push to the working branch went unverified: the suite was only
  ever exercised on Linux at merge time. `feat/**` was added to the push trigger
  (`bc2a7cc`) and the first run on the feature branch is green, which is the point
  of the change rather than an assumption about it. Two runner annotations remain:
  `actions/checkout` is pinned two majors behind (v4 against v7.0.1, verified
  against the repository rather than guessed), which is why the runner reports
  Node.js 20 as deprecated, and `ubuntu-latest` migrates to Ubuntu 26 in October
  2026.
- T7 done, two commits: `7201283` the ten entry types and the persisted JSONL
  format (12 tests), `6a77906` in-memory and file-backed storage (14 tests). 131
  tests in the suite, clippy clean.
  Deferred, with the reason recorded because it changes what taurust can read:
  `jsonl.py`'s migration of pre-bookmark Tau-v1 records. Nothing in taurust writes
  that shape, so it only matters when pointing taurust at an existing tau session
  directory; it moves to T21 with the rest of the session-management work.
  TDD order was inverted again for the storage: the synchronous-versus-async
  decision and the lock-and-rename design had to be settled before tests could be
  written against them. The two tests that failed during T7 were real defects — an
  empty replaced-id list stripped on read instead of on write, and a line-number
  assertion pointing at the wrong line.
- T8 done in two commits: `dd3cd30` path resolution (11 tests, written inline) and the
  credential store plus provider configuration resolution (56 tests), written by a
  delegated `gentle-ai-worker` and verified here — only the allowed surfaces
  changed, `fmt` clean, `clippy` clean under `-D warnings`, 202 tests against 146
  before, and all 80 user-facing message strings checked against the reference
  literals (73 literally in the two ported modules, 3 from the unported
  `thinking.py`, and the interpolation templates confirmed by prefix).
  Disclosed scope gaps, each because the module it needs is not ported yet:
  `load_provider_settings` reads and parses but does not merge the built-in catalog
  in, so a preferences-only file returns only the preferred providers where the
  reference returns the credentialed catalog; persistence write-back, the OAuth
  registry gate, and the startup thinking-level policy are absent; and the runtime
  adapter config is exposed as resolution functions rather than built, because the
  types it returns belong to `tau_ai`.
  The thinking-level vocabulary currently lives in `provider_config` and must move
  to `tau-coding::thinking` when that module is ported.
- Subagent note: two `gentle-ai-explore` subagents were dispatched for the tau
  architecture map and the gentle-shell visual map; both failed with "assistant
  reported an error". Mapping is therefore done inline with bounded reads. Worth
  retrying with narrower prompts at the start of the TUI phase (T12+).

## Log

- L1 — User: *"quiro rehacer tau pero con rust y la shell que se vea como gentle-shell  C:\Users\USER\Desktop\herramientas\gentle-shell  C:\Users\USER\Desktop\herramientas\tau"*
- L2 — User: *"y el todo de gentle shell"* (appended while the scope questionnaire was open; the user cancelled the questionnaire). Read together with L3 as confirming the gentle-shell todo panel is in scope.
- L4 — User: *"ok, antes de continuar crea el repo en github y pushea"*. Published at <https://github.com/biggs-100/taurust>. Asked and answered before publishing, because publishing is not fully reversible: visibility **public**, and therefore the MIT notices of both upstream works reproduced in `THIRD-PARTY-NOTICES.md` before the first push; default branch **`main`**, created at the published state, with `feat/tau-rust-port` kept as the working branch.
