# taurust agent instructions

`taurust` is a Rust rewrite of `tau` (Python) with the `gentle-shell` terminal look.

## Architecture rules

Preserve tau's layer separation. Dependencies flow one way only:

```text
tau-tui -> tau-coding -> tau-agent -> tau-ai
```

- `tau-ai` must not know about the terminal, local config paths, slash commands or rendering.
- `tau-agent` is the reusable brain. It must not depend on `tau-coding`.
- `tau-coding` owns the application: CLI, config, credentials, tools, resources, sessions.
- `tau-tui` is one possible frontend. It consumes events and is never depended on by the core.

A change that makes a lower crate import an upper one is wrong, even when it is convenient.

## Conventions

- Tests are written before the implementation, and the RED run is observed.
- `unsafe_code` is forbidden workspace-wide.
- Commit messages are in English and carry no AI attribution.
- Artifacts in English; conversation with the user in Spanish (neutral).

## Reference sources (read-only)

- `C:\Users\USER\Desktop\herramientas\tau` — the Python original.
- `C:\Users\USER\Desktop\herramientas\gentle-shell` — the visual target.

## Tracking

The canonical task document is `odd/tasks/tau-rust-port.md`.
