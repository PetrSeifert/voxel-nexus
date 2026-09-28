# Rust coding guidelines

* Prioritize correctness and clarity. Speed is secondary unless the task says otherwise.
* Write comments only to explain *why*: a non-obvious reason, invariant, or constraint. The code itself says *what*.
* Handle every `Result`: propagate it with `?`, or `match` on it where the caller needs custom recovery. Each library crate defines its own typed errors with `thiserror`; binaries report `String` errors.
* Reserve panics for broken invariants (bugs, never runtime conditions), and write them as `expect("<why this cannot fail>")` so the message states the invariant. Tests may `unwrap()`.
* Reach elements with `get()`, iterators, or pattern matching; index directly only where the bound is established in plain sight.
* Give an `unsafe` block a `// SAFETY:` comment when its soundness rests on more than valid Vulkan handles.
* Name module files `src/some_module.rs`, never `src/some_module/mod.rs`.
* Root each new crate at `src/<crate_name>.rs` via `[lib] path` in its `Cargo.toml`, and opt it into workspace lints with `[lints] workspace = true`.
* Build exactly what the task requires; propose extra features, abstractions, or files rather than adding them.
* Use full words for names (`queue`, not `q`).
* Finish with `cargo fmt --all` and `cargo clippy --workspace --all-targets --all-features` clean: zero warnings.

## Agent skills

### Issue tracker

Issues and specs are tracked in GitHub Issues using the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Domain docs

This repository uses the single-context domain documentation layout. See `docs/agents/domain.md`.
