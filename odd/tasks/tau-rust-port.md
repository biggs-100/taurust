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

```
tau-tui     (frontend, consumes events) -> tau-coding -> tau-agent -> tau-ai
```

The boundary rule from `tau/AGENTS.md`: the core must not know about the terminal,
local config paths, slash commands or rendering. Frontends consume events.

> The detailed per-module map (contracts, JSONL record shape, CLI surface, TUI widget
> inventory) is filled in per phase from a scoped exploration of the corresponding
> layer, and recorded here before that phase's tasks start.

## Tasks

- T1 — Workspace foundation: `git init`, Cargo workspace, four crate skeletons, toolchain pinning, lint/format config, CI, README, AGENTS.md · Spec: S1,S6 · Route: inline · Commit: pending
- T2 — `tau-ai`: content model, provider-neutral event stream, `ModelProvider` trait, fake provider · Spec: S1 · Route: pending · Commit: pending
- T3 — `tau-ai`: HTTP client, retry/backoff policy, error taxonomy · Spec: S1 · Route: pending · Commit: pending
- T4 — `tau-agent`: messages, types, tool protocol and registry · Spec: S1 · Route: pending · Commit: pending
- T5 — `tau-agent`: agent loop and harness · Spec: S1 · Route: pending · Commit: pending
- T6 — `tau-agent`: session storage with the JSONL record format · Spec: S1 · Route: pending · Commit: pending
- T7 — `tau-coding`: paths, provider config resolution, credential store · Spec: S1 · Route: pending · Commit: pending
- T8 — `tau-coding`: built-in tool implementations (read, write, edit, shell, search) · Spec: S1 · Route: pending · Commit: pending
- T9 — `tau-coding`: CLI surface with parity to the typer app · Spec: S1 · Route: pending · Commit: pending
- T10 — `tau-coding`: context and system-prompt assembly, project trust, skills, extensions · Spec: S1 · Route: pending · Commit: pending
- T11 — `tau-tui`: primitives — theme roles, layout, width measurement, box drawing, ANSI · Spec: S2,S3 · Route: pending · Commit: pending
- T12 — `tau-tui`: prompt chrome, idle/working/queued states, working-label animation · Spec: S2,S3 · Route: pending · Commit: pending
- T13 — `tau-tui`: sidebar with Status / Project / Changes / Integrations · Spec: S2,S3 · Route: pending · Commit: pending
- T14 — `tau-tui`: todo panel · Spec: S5 · Route: pending · Commit: pending
- T15 — `tau-tui`: tool cards, output rendering, diff view · Spec: S2,S3 · Route: pending · Commit: pending
- T16 — `tau-tui`: gauges, usage view, stats view · Spec: S2,S3 · Route: pending · Commit: pending
- T17 — `tau-tui`: command palette, native choice lists, overlays · Spec: S2,S3 · Route: pending · Commit: pending
- T18 — `tau-tui`: keybindings and visual configuration · Spec: S2,S3 · Route: pending · Commit: pending
- T19 — Providers: anthropic, openai-compatible, openai-codex, google, mistral, plus OAuth flows · Spec: S1 · Route: pending · Commit: pending
- T20 — Sessions: export, reload, extensions, RPC surface · Spec: S1 · Route: pending · Commit: pending
- T21 — Parity gate: behaviour checklist run against the Rust binary, side by side with tau · Spec: S1,S3,S4 · Route: pending · Commit: pending

## Log

- L1 — User: *"quiro rehacer tau pero con rust y la shell que se vea como gentle-shell  C:\Users\USER\Desktop\herramientas\gentle-shell  C:\Users\USER\Desktop\herramientas\tau"*
- L2 — User: *"y el todo de gentle shell"* (appended while the scope questionnaire was open; the user cancelled the questionnaire). Read together with L3 as confirming the gentle-shell todo panel is in scope.
- L3 — User: *"completo, menos el banner"*. Resolves both open questions: full port (not a vertical slice) and the complete gentle-shell visual experience, excluding the startup banner. The orchestrator's stated defaults for destination and crate layout were not corrected, so they stand as S6.
