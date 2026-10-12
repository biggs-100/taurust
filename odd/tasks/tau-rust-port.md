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
- T9 — `tau-coding`: built-in tool implementations — `read`, `write`, `edit`, `bash` — plus the image detection and normalization the `read` tool depends on · Spec: S1 · Route: parent-frozen shared core then 4 bounded writers in parallel, later rewritten into chained slices for the reviewer budget · Commit: `2008425`, `a8b661a`, `1482440`, `e052851`
- T10 — `tau-coding`: CLI surface with parity to the typer app · Spec: S1 · Route: parent-mapped surface then bounded writers; option behaviour deferred per module · Commit: pending
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
- T23 — `tau-coding`: port `difflib`'s `_fancy_replace`/`_fancy_helper`/`_qformat` so `edit`'s `details.diff` reaches the reference's output for replaced blocks. Needed because the diff becomes user-visible when the tool cards and diff view land (T16). · Spec: S1 · Route: pending · Commit: pending

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

- Subagent note: two `gentle-ai-explore` subagents were dispatched for the tau
  architecture map and the gentle-shell visual map; both failed with "assistant
  reported an error". Mapping is therefore done inline with bounded reads. Worth
  retrying with narrower prompts at the start of the TUI phase (T12+).
- T9 in progress. The task line was corrected after reading the reference: tau has
  four built-in tools, `read`, `write`, `edit` and `bash`. There is no separate
  search tool — `bash` carries `ls`, `grep` and `find`, and its prompt snippet
  says so. The earlier line guessed "shell, search" and was wrong.
  Scope decision (user, Log L5): the `read` tool detects images by content and,
  by default, attaches them; `process_image` decodes, resizes and re-encodes
  within deterministic limits. That is the default path, so it is ported in T9
  rather than deferred, and it costs the workspace its first image dependency.
  Route (Writer rule, parallelism): the model file is one 1200-line module whose
  helpers nearly all reach more than one tool, so the parent froze the shared
  vocabulary first (`tools/support.rs`: truncation, `format_size`, the argument
  readers and the per-path mutation lock, with `tests/tools_support.rs`) and the
  module skeleton with the four declarations in `tools/mod.rs`; then four bounded
  writers implement disjoint files in one turn — `image_processing.rs`,
  `read.rs`, `write.rs`+`edit.rs`+`diff.rs`, and `bash.rs`. The parent owns
  integration, `fmt`, `clippy` and the seam tests in `tests/tools_catalog.rs`.
  Rust shape deviations, all forced by the language rather than chosen:
  `ToolDefinition` plus `to_agent_tool()` collapses into a `ToolSpec` and a
  struct implementing `tau_agent::tools::Tool`, because the declaration already
  carries the label, snippet and guidelines; `process_image`'s four keyword
  limits become one `ImageLimits`; and the reference's per-call cancellation
  signal has nowhere to go, since the Rust `Tool` trait carries none, so `bash`
  holds an injected token from construction instead.

- T9 done in two commits: `6e75739` the four tools plus the image normalization
  they need, and this close commit. `tools.py` is one 1200-line module whose
  helpers nearly all reach more than one tool, so the parent froze the shared
  vocabulary first and then ran four bounded writers over disjoint files in one
  turn. The parent's own slice went through RED and GREEN (19 tests, 0 before,
  19 after) before anything was delegated.
  Worker-reported counts: `image_processing` 13 integration + 9 unit, `read` 19,
  `write`+`edit`+`diff` 56, `bash` 8 on Windows with 2 more gated to unix. After
  integration the parent measured 200 tests in `tau-coding` and 331 in the
  workspace, against 202 workspace-wide before T9.
  Parent verification after the reports, not instead of them: `cargo fmt --all
  --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean;
  `cargo test --workspace` green; and `cargo check` plus `cargo clippy --
  -D warnings` for `x86_64-unknown-linux-gnu` clean, which is what typechecks
  the `cfg(unix)` code a Windows host never compiles.
  Three defects the parent found and fixed at integration: a dead assignment in
  `support.rs`, an unused import in `tools_catalog.rs`, and the `\ No newline at
  end of file` marker. The last one matters because the marker was this parent's
  own instruction to the writer and it is **wrong**: CPython 3.12.10's
  `difflib.unified_diff` never emits it, and because tau hands it
  `splitlines(keepends=True)` lines, a final line with no terminator leaves the
  next patch line glued to it (`-b+c`). Verified by running the reference's own
  call, and now reproduced rather than corrected.
  Checks the parent ran beyond the workers' own reports: the hand-written base64
  encoder is pinned by a unit test to eight vectors produced by CPython 3.12.10
  including all 256 byte values, because the workspace has no base64 dependency
  and a symmetric encoder/decoder bug would otherwise pass; the two-hunk unified
  patch matches CPython exactly, header ranges included; the Windows
  shell-prefix limitation was verified against the reference's own mechanism on
  this host (`subprocess.run('echo one\necho two', shell=True)` prints only the
  first line), so reproducing it is faithful rather than a defect introduced
  here; and the autojunk behaviour produced the finding below.
  Deviation the parent found by measuring, not by trusting: `difflib.ndiff` hides
  `SequenceMatcher`'s autojunk collapse because `_fancy_replace` re-pairs
  identical lines as context. The port does not reproduce `_fancy_replace`, so
  for a 250-line single-repeated-line input CPython prints 251 lines with one
  deletion and one insertion where the port prints 375 with 125 of each; the
  199-line case agrees exactly, and `first_changed_line` agrees in both. The
  writer disclosed the omission in prose, but its test asserted the port's own
  numbers under a name claiming difflib parity, which would have misled the T22
  gate; the test is renamed and now states both sets of numbers instead. Because
  `details.diff` becomes user-visible with the tool cards and the diff view, the
  missing `_fancy_replace` is tracked as T23 and precedes or accompanies T16.
  Supply chain, since the user approved a dependency choice: the `image` crate
  brings 27 new packages into the lock, and tokio's `process` feature adds `mio`
  and `signal-hook-registry`. `rustix` adds nothing — it was already a
  transitive dependency and is now a direct unix-only one. All 27 new packages
  are permissive (MIT, Apache-2.0, BSD-3-Clause, Zlib, 0BSD, Unlicense, and one
  `Apache-2.0 WITH LLVM-exception`); audited with `cargo metadata` rather than
  assumed, because the repository is public.
  Disclosed fidelity deviations, all because the port has no exact counterpart:
  WebP is encoded lossless (no pure-Rust encoder offers tau's lossy quality
  ladder, so output is larger and the bounded resize loop may shrink further);
  `optimize=True` has no equivalent for PNG or JPEG and the crate always
  palette-quantizes GIF; Python's text-mode `read_text`/`write_text` means tau
  converts an LF file to CRLF on Windows where the port keeps the detected
  ending; `path_arg` joins the raw argument, so paths echo the separators the
  model typed where `pathlib` normalizes them on Windows; and decode and I/O
  failure texts carry the Rust library's message rather than Pillow's or
  Python's.
  Open: the unix path is compiled and linted for Linux but has never been
  executed, because the host is Windows and the local WSL has no Rust toolchain.
  CI runs it on Linux, so closing that gap needs a push, which is the user's
  decision.
- T9 correction round. The independent verifier the risk plan required FAILED the
  candidate above, and the native review was therefore not started even though
  the user had granted consent: starting it would have reviewed a candidate
  already known defective, and any correction would have invalidated the
  lineage. It found two majors the author's own 200 green tests, `fmt`, `clippy`
  and the Linux cross-check did not see, both on paths a happy run never takes.
  F1, Windows quoting: the tool built its shell line as
  `arg("/C").arg(command)`, and Rust's argv quoting escapes every embedded `"`
  as `\"`, which `cmd.exe` echoes back verbatim. Measured against the
  reference's own mechanism: `echo "a"` gave `\"a\"` where the reference gives
  `"a"`; `where "cmd"` exited 1 with "no files found for the given patterns"
  where the reference finds the path; `dir "C:\Windows"` exited 1 with a cmd
  syntax error where the reference exits 0; `python -c "print(1)"` printed
  nothing with exit 0 where the reference prints `1`. A `git commit -m "msg"`
  would have corrupted the message silently. Fixed by building exactly what
  CPython's `subprocess.Popen._execute_child` builds, `<comspec> /c "<command>"`,
  appended with tokio's `raw_arg` (confirmed present in the installed tokio
  1.53.2) rather than with argv quoting.
  F2, the deadline and the token stopped acting once both output streams closed:
  the loop left its receive branch and awaited `child.wait()` with no deadline,
  so a command that closed its own descriptors and kept running was reported as a
  success with `timed_out: false` and `cancelled: false`. The reference does not
  behave that way, because `Process.communicate()` waits for process exit as well
  as for pipe EOF; the verifier measured the reference at 1.05 s with
  `timed_out True` against a child sleeping 30 s. Fixed by awaiting the exit
  inside the same `select!` as the deadline and the token, so the call returns
  only when the readers are done and the child has exited.
  F6 and F7 also fixed: the emptiness filter now precedes the trim, so a
  whitespace-only prefix survives as `""` exactly as
  `shell_command_prefix.strip() if shell_command_prefix else None` leaves it, and
  the Windows shell is resolved from `COMSPEC` with `cmd` as the fallback, which
  is what `subprocess` honours.
  Decision (A) on the one item the writer could not deliver: F2 stays pinned by a
  `#[cfg(unix)]` test with no Windows twin. On Windows `cmd.exe` holds the tool's
  stdout and stderr handles until it exits, so a `cmd /C` command can never reach
  the "pipes closed, child running" state, and the reference itself takes 148.59 s
  for `ping -n 30 127.0.0.1 >NUL 2>&1` under a 0.25 s timeout with
  `timed_out: true`. A Windows test asserting a prompt return would have to
  deviate from the reference, and the writer's option C — returning as soon as the
  child exits without waiting for pipes an orphan still holds — was rejected for
  the same reason: it would return before collecting output the reference still
  collects. The reference wins.
  The honest caveat this round leaves behind: F2's fix has no observed RED or
  GREEN anywhere, because the `#[cfg(unix)]` test cannot run on this host (WSL
  Ubuntu-24.04 is installed but has no `cargo`, and installing a toolchain was not
  authorized). Its discriminating power rests on a socketpair-EOF probe the writer
  ran in real Linux — the socketpair closes 6 ms after a self-redirecting child
  starts, so the pre-fix code would have blocked the full 30 s — and not on an
  observed failure. The whole `cfg(unix)` suite, 12 tests, has only ever been
  compiled here. That is the strongest single reason to push and let CI execute
  it.
  Remaining deviations, disclosed rather than changed, each because the port has
  no faithful cheap counterpart: the Windows stdout and stderr are merged from two
  racing readers rather than the reference's single fd, so their interleaving is
  best-effort; the full-output temp log is written as bytes where the reference
  goes through Python's text mode; the unix streams are merged with a socketpair,
  which is visible to a program that tests its stdout type (`S_ISSOCK` rather than
  the reference's `S_ISFIFO`); `process_group(0)` is `setpgid` and not the
  reference's `start_new_session` `setsid`, so the child keeps the parent's session
  and controlling terminal although the group kill still works; `~user` is not
  expanded where `Path.expanduser` would, needing a passwd lookup this workspace
  cannot make without `unsafe`; and a JSON boolean is rejected by the integer
  reader where Python's `isinstance(True, int)` accepts it as 1, a coercion the
  typed readers deliberately do not reproduce.
  Counts after the correction: 204 tests in `tau-coding` against 200 before, with
  `tools_bash.rs` holding 21 distinct test functions — 11 of them run on Windows
  and 12 would run on Linux.
- T9 second correction round. The second verification round failed the first
  round's corrections as well, and its finding was exact: the F6 fix had traded
  one wrong behaviour for another. The reference's prefix decision is really two
  independent tests, not one. `prefix = shell_command_prefix.strip() if
  shell_command_prefix else None` leaves `""` for a whitespace-only input, and
  `executable="bash" if prefix else None` gates the shell on that value's
  *truthiness* — so such a prefix is applied and reported as applied while the
  command still runs `/bin/sh`. The port had started selecting `bash`, and the
  test written for it asserted that wrong selection, which is worse than no test:
  it would have held the deviation in place. The two decisions are now separate,
  `shell_command_prefix_applied` staying `prefix.is_some()` while the unix
  executable follows the prefix's truthiness, and each is pinned.
  The same round corrected the Windows shell fallback. CPython resolves `ComSpec`,
  then `<SystemRoot>\System32\cmd.exe`, and raises when the result is not
  absolute; the port had used a bare `cmd`. A bare `cmd` now survives only as a
  last resort, which deviates from CPython's raise and is stated in the code
  rather than left silent.
  The parent then replaced the unix test's probe. It had used
  `${BASH_VERSION:+bash}`, which discriminates on Linux but would fail on any host
  where `/bin/sh` is bash: the behaviour would be right and the test wrong. The
  probe is now `printf '%s' "$0"`, which reports the executable the tool actually
  spawned, with a control asserting `bash` for a truthy prefix so a broken probe
  cannot pass for a correct one.
  One further difference from the reference surfaced while making that change, and
  is disclosed rather than fixed: the port spawns `sh` resolved through `PATH`
  where the reference always executes the absolute `/bin/sh`, so a host with a
  different `sh` earlier in `PATH` would diverge. The fix is one line if the
  parity gate ever cares, and the test's expectation would move with it.
  What this round still cannot show: the corrected unix test has never been
  executed, only compiled for the Linux target. Its discriminating power rests on
  a control that proves the probe detects the selected shell, not on an observed
  failure, and the whole `cfg(unix)` suite still has to run on CI.
  Counts after the second round: 205 tests in `tau-coding`, 12 of them unit tests
  in the lib target, with `tools_bash.rs` holding 21 distinct test functions, 11 of
  which run on Windows and 12 on Linux.
  Verification trail for the reader: round 1 `fail` (two majors, six minors),
  round 2 `fail` (the F6 fix traded one wrong behaviour for another), round 3
  `pass`. The third round confirmed both second-round corrections against live
  reference runs, including replaying CPython's Windows resolver across eight
  inputs, verified that the `$0` probe still discriminates on a host where
  `/bin/sh` is bash, and proved the Linux artifacts were freshly rebuilt rather
  than trusting a cached `Finished` line.
  Its one finding is low severity and is disclosed here instead of fixed: Rust's
  `Path::is_absolute` is stricter than `ntpath.isabs`, so a `SystemRoot` that is
  rooted without a drive (for example `\Windows`) is absolute to CPython and not
  to Rust, and the port then degrades to the bare `cmd` last resort where CPython
  uses the resolved path. The CPython side was verified live; the Rust side was
  reasoned from documented `std` behaviour, since no authorized command compiles a
  Rust probe. Real hosts set `SystemRoot=C:\Windows`, and the consequence is the
  bare `cmd` that is already the disclosed last resort, so the fix is one line if
  anything ever cares: test for a rooted path rather than an absolute one.

- T9 was rewritten into chained slices so the native reviewer's context budget
  could cover it. `assess` reported high risk and `reviewDue` over the whole
  change, and the provider then refused the candidate outright with
  `lens_context_budget_exceeded`: its complete reviewer evidence exceeded the
  native context budget, no review authority was created, nothing was mutated, and
  retrying that exact candidate was declared unable to succeed. The provider's own
  remediation was a chained sequence of smaller candidates, and the user chose the
  rewrite over losing the native receipt.
  One honest slicing pass, each slice a coherent work unit carrying its own tests,
  with the changed-line counts including the intermediate `mod.rs` amendments:
  `2008425` the shared vocabulary and the image pipeline (9 files, 1960 lines),
  `a8b661a` the read tool (3 files, 1068),
  `1482440` the write and edit tools with their diff rendering (5 files, 2056),
  `e052851` the bash tool and the assembled tool set (4 files, 1313).
  Every slice was built and its whole suite run before it was committed, because a
  slice that does not compile is a broken candidate and a reviewer would be right
  to report it as one. The suite grew 112, 132, 188 and 205 tests along the chain,
  green at every step.
  The code and tests are byte-identical to the state the three verification rounds
  passed: `git diff --name-only 2a63137 HEAD` lists this document and nothing else.
  Every hash quoted earlier in this entry belongs to the pre-rewrite history and no
  longer exists. The corrections those commits describe are folded into the slices
  above on purpose, so a reviewer never re-finds a defect that is already fixed.
  above on purpose, so a reviewer never re-finds a defect that is already fixed.
- Native review of T9: declared unavailable for this exact candidate, on the user's
  decision, after the provider refused it in every admissible shape. The cause and
  its reproductions are recorded in `odd/rdd-lens-budget-defect.md`: on
  `gentle-ai 4.0.0` a high-risk candidate must select all four lenses, their
  combined reviewer evidence exceeds the native context budget, and neither
  `--focus` nor `--policy` can change that shape. A coherent 1960-line slice fails
  the same way, so the provider's "split it into smaller candidates" remedy cannot
  be followed to success. Two traps came out of the investigation and are in that
  note: the `--consent=relay` question is emitted *before* the evidence is
  materialized, so receiving it proves nothing, and the consent envelope's own
  `granted` invocation omits the flag that made the envelope producible, so a host
  that executes the envelope faithfully cannot succeed.
  `assess` therefore records `nativeReviewOutcome: unavailable` with
  `outcome_source: explicit` for the whole `6ee432f..HEAD` candidate, and the risk
  plan still requires an independent verifier — which T9 has: three rounds,
  verdicts `fail`, `fail`, `pass`. The native review is an additional outside view
  and its absence does not lower the bar: `fmt`, `clippy --workspace --all-targets
  -- -D warnings`, the 331-test workspace suite and the Linux cross-check all
  still stand, and the chained slices remain the reviewable history.
- The gap those verification rounds could not close is now closed by execution
  rather than by argument. The branch was pushed and CI ran on Linux at `ea8ae43`,
  green in 36 s, with `tests/tools_bash.rs` reporting **12 passed, 0 failed** — the
  12 `cfg(unix)` tests, against 11 on Windows. Both items the verifiers had to mark
  unproven passed there: `bash_keeps_a_whitespace_only_shell_command_prefix`, which
  runs the `$0` probe and asserts `/bin/sh` was selected, and
  `bash_times_out_a_child_that_closed_its_own_output`, which is the F2 fix. The
  unix path — the process-group leader, the `rustix` process-group kill and the
  wait loop that keeps its deadline armed after a child closes its own output — is
  therefore observed running, not merely compiled. Every earlier statement in this
  entry that the `cfg(unix)` suite had only been compiled was true when written and
  is superseded by this run.
- A defect T9 shipped and all three verification rounds missed, found later by the
  T10 unit: `cargo build -p tau-coding --bin tau` failed with `error[E0433]: cannot
  find 'select' in tokio` at `tools/bash.rs:133`. The tool uses `tokio::select!` and
  the `macros` feature was declared only in `[dev-dependencies]`, so the binary did
  not build for anyone, while `cargo test`, `cargo clippy --all-targets` and CI all
  passed because they unify dev-dependencies. Reproduced here as exit 101, then
  fixed by declaring the feature in `[dependencies]`.
  The gap that hid it is closed too: CI gained a `cargo build --workspace
  --all-features` step, deliberately without `--all-targets`, because that flag is
  what unifies dev-dependencies. This is the second time in this port that the
  decisive signal came from a command nobody was running — the first was the
  `feat/**` push trigger — and it is worth stating plainly for a reader: a green
  `cargo test` is evidence about tests, never about the artifact.
- T10 mapped before any implementation, by a bounded explorer, because `cli.py` is
  1394 lines and its surface does not fit one reading batch. What the map settled:
  tau's typer app registers **one callback and no subcommands** (`cli.py:104`,
  `cli.py:192`). `install`, `update`, `sessions`, `export`, `providers` and `setup`
  are hand-dispatched pseudo-commands on the first positional argument
  (`cli.py:448-515`). Unknown words and unknown flags are **not** errors:
  `allow_extra_args` with `ignore_unknown_options` (`cli.py:122`) turns them into
  the initial prompt (`cli.py:448-451`). Exit `2` is Click's usage error, `1` covers
  install, update and print-mode failures, and `0` is the normal path.
  Two fidelity defects in what is already committed, both found by that map: the
  reference uses `-v` for version where the port uses `-V` (`lib.rs:68`), and the
  port's `tau: unsupported argument:` error with exit 2 (`main.rs:23`) has **no
  counterpart in the reference at all** — it is port-invented behaviour that the
  T22 parity gate would have recorded as a divergence.
  Scope decision: the *parse surface* is portable now — names, types, defaults,
  repetition and hidden flags, each quoted with its line in the map — but the
  *behaviour* behind almost every option is not, because it needs modules that do
  not exist yet: `session_manager`, `session_export`, `session_preparation`,
  `extensions`, `extension_installer`, `updater`, `update_check`,
  `models_dev_store`, `catalog_loader`, `rendering`, `rpc`, `thinking`,
  `project_trust`, `tui`, and the `tau_ai.env` defaults. Each such option
  therefore lands as a parsed and declared flag plus a tracked deferral, the shape
  T4 and T7 already used, never as a flag that silently accepts and does nothing.
  The CLI's own contract file is `tests/test_cli.py`: 2140 lines and 89 test
  functions driving the real app through `typer.testing.CliRunner`, so only the
  subset exercising parsing, help, version and usage errors is portable inside T10;
  the rest waits for the modules behind it.
  Open decision for the implementation, to be settled from evidence and not taste:
  the reference's `--help` is **generated by Typer** (`cli.py:105-120`) and its tests
  pin fragments of that rendering (`tests/test_cli.py:125-127`). Reproducing Click's
  and Rich's layout byte-for-byte is a far larger job than listing the same flags,
  so the first implementation unit must decide, from the reference tests
  themselves, which parts of the help are contract and which are incidental
  formatting.
  The reference has no `__main__.py`, so the `tau` console script is its only entry
  point.
- T10's first unit is implemented: the parse surface. Before writing anything, the
  unit ran the reference through `typer.testing.CliRunner` the way its own
  `tests/test_cli.py` does and recorded stdout, stderr and the exit code for
  no-arguments, `-v`, `--version`, `-V`, `-h`, `--help`, an unknown word, an unknown
  flag, a bad option value and a pseudo-command name used as a positional. That
  table is the contract now, and it settled four things that reasoning would have
  got wrong: the reference accepts `-v` but neither `-V` nor `-h`, which both become
  prompt text; `--help` is eager and beats value conversion yet loses to a structural
  parse error; value conversion follows command-line order while `--version` is a
  body check that runs last, so `-v --max-retries abc` exits 2 while `--help
  --max-retries abc` exits 0; and `allow_interspersed_args = False` stops parsing at
  the first positional, so `-p hi --session-id x` is a prompt and not a session id.
  Delivered: 31 registered options with the reference's names, types, defaults,
  repetition and hidden flags, a Click-compatible parser, the outcome enum and the
  prompt assembly. The port's two invented behaviours are gone: `-V` is no longer a
  version flag, and `tau: unsupported argument:` with exit 2 — which the reference
  does not have at all — was replaced by the prompt behaviour that was observed.
  `tau-coding` went from 205 to 251 tests: `tests/cli.rs` 4 to 20 and 30 new ones in
  `tests/cli_surface.rs`, each naming the reference test it mirrors, plus a
  differential run of 27 error paths against the built binary that matched exit
  codes and message text in every case.
  Disclosed differences: the help *content* is the reference's while Click's and
  Rich's box layout is not reproduced; a usage error prints `Error: <message>`
  rather than a bordered panel, with the same text and exit code 2; every path whose
  module is unported reports one line naming that module and exits 2, which T22 must
  treat as a known divergence; and `validate_session_id` plus `PrintOutputMode` live
  in `cli.rs` as debts until `session_manager` and `rendering` are ported.

## Log

- L1 — User: *"quiro rehacer tau pero con rust y la shell que se vea como gentle-shell  C:\Users\USER\Desktop\herramientas\gentle-shell  C:\Users\USER\Desktop\herramientas\tau"*
- L2 — User: *"y el todo de gentle shell"* (appended while the scope questionnaire was open; the user cancelled the questionnaire). Read together with L3 as confirming the gentle-shell todo panel is in scope.
- L4 — User: *"ok, antes de continuar crea el repo en github y pushea"*. Published at <https://github.com/biggs-100/taurust>. Asked and answered before publishing, because publishing is not fully reversible: visibility **public**, and therefore the MIT notices of both upstream works reproduced in `THIRD-PARTY-NOTICES.md` before the first push; default branch **`main`**, created at the published state, with `feat/tau-rust-port` kept as the working branch.
- L5 — User, answering the T9 scope question with the option *"Completo ahora, con el crate `image`"*: port `image_processing.py` inside T9, accepting one new third-party dependency (`image` 0.25) in exchange for the `read` tool attaching images on its default path. Asked because the alternative was a dependency-free T9 that omits image attachments with a note and defers normalization to its own task, and because adding a dependency to a public repository is not a decision the orchestrator should make silently. The known cost, disclosed before the choice was offered: no Rust crate reproduces tau's lossy WebP encoding ladder, so only the WebP-requires-resize path is expected to deviate. To be confirmed empirically by the image unit and recorded here.
