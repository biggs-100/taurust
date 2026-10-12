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

> The remaining per-module maps (JSONL record shape, CLI surface, TUI widget
> inventory) are recorded here before the tasks that depend on them start.

## Tasks

- T1 — Workspace foundation: `git init`, Cargo workspace, crate skeletons, lint/format config, CI, README, AGENTS.md · Spec: S1,S6 · Route: inline · Commit: `dcc7382`
- T2 — `tau-protocol`: content blocks, transcript messages, usage and timing, Pi wire fidelity · Spec: S1 · Route: inline · Commit: `b9ae762`
- T3 — `tau-protocol`: streaming events (assistant and agent), `ModelProvider` trait, fake provider · Spec: S1 · Route: pending · Commit: pending
- T4 — `tau-ai`: HTTP client, retry/backoff policy, error taxonomy · Spec: S1 · Route: pending · Commit: pending
- T5 — `tau-agent`: tool protocol and tool registry · Spec: S1 · Route: pending · Commit: pending
- T6 — `tau-agent`: agent loop and harness · Spec: S1 · Route: pending · Commit: pending
- T7 — `tau-agent`: session storage with the JSONL record format · Spec: S1 · Route: pending · Commit: pending
- T8 — `tau-coding`: paths, provider config resolution, credential store · Spec: S1 · Route: pending · Commit: pending
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
- T20 — Providers: anthropic, openai-compatible, openai-codex, google, mistral, plus OAuth flows · Spec: S1 · Route: pending · Commit: pending
- T21 — Sessions: export, reload, extensions, RPC surface · Spec: S1 · Route: pending · Commit: pending
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
- Subagent note: two `gentle-ai-explore` subagents were dispatched for the tau
  architecture map and the gentle-shell visual map; both failed with "assistant
  reported an error". Mapping is therefore done inline with bounded reads. Worth
  retrying with narrower prompts at the start of the TUI phase (T12+).

## Log

- L1 — User: *"quiro rehacer tau pero con rust y la shell que se vea como gentle-shell  C:\Users\USER\Desktop\herramientas\gentle-shell  C:\Users\USER\Desktop\herramientas\tau"*
- L2 — User: *"y el todo de gentle shell"* (appended while the scope questionnaire was open; the user cancelled the questionnaire). Read together with L3 as confirming the gentle-shell todo panel is in scope.
- L3 — User: *"completo, menos el banner"*. Resolves both open questions: full port (not a vertical slice) and the complete gentle-shell visual experience, excluding the startup banner. The orchestrator's stated defaults for destination and crate layout were not corrected, so they stand as S6.
