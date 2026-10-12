# taurust

A Rust rewrite of [tau](https://github.com/huggingface/tau), the small terminal
coding agent — with the [gentle-shell](https://github.com/Gentleman-Programming/gentle-shell)
terminal experience.

## Architecture

```text
tau-tui  ->  tau-coding  ->  tau-agent  ->  tau-ai
```

The boundary rule is tau's own: the core does not know about the terminal, local
configuration paths, slash commands or rendering. Frontends consume events.

| Crate | Responsibility |
|---|---|
| `tau-ai` | Provider-neutral model streaming |
| `tau-agent` | Portable agent brain: messages, tools, loop, harness, sessions |
| `tau-coding` | The coding application: CLI, config, tools, resources, sessions |
| `tau-tui` | Terminal frontend with the gentle-shell visual language |

## Development

```bash
cargo test --all-features
cargo fmt --all --check
cargo clippy --all-targets --all-features
```

## Status

Work in progress. The task plan lives in `odd/tasks/tau-rust-port.md`.
