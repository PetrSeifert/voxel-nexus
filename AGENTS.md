# Rust coding guidelines

* Prioritize code correctness and clarity. Speed and efficiency are secondary priorities unless otherwise specified.
* Do not write organizational or comments that summarize the code. Comments should only be written in order to explain "why" the code is written in some way in the case there is a reason that is tricky / non-obvious.
* Avoid using functions that panic like `unwrap()`, instead use mechanisms like `?` to propagate errors.
* Be careful with operations like indexing which may panic if the indexes are out of bounds.
* Never silently discard errors with `let _ =` on fallible operations. Always handle errors appropriately:
  - Propagate errors with `?` when the calling function should handle them
  - Use `.log_err()` or similar when you need to ignore errors but want visibility
  - Use explicit error handling with `match` or `if let Err(...)` when you need custom logic
  - Example: avoid `let _ = client.request(...).await?;` - use `client.request(...).await?;` instead
* When implementing async operations that may fail, ensure errors propagate to the UI layer so users get meaningful feedback.
* Never create files with `mod.rs` paths - prefer `src/some_module.rs` instead of `src/some_module/mod.rs`.
* When creating new crates, prefer specifying the library root path in `Cargo.toml` using `[lib] path = "...rs"` instead of the default `lib.rs`, to maintain consistent and descriptive naming (e.g., `gpui.rs` or `main.rs`).
* Avoid creative additions unless explicitly requested
* Use full words for variable names (no abbreviations like "q" for "queue")
* Use variable shadowing to scope clones in async contexts for clarity, minimizing the lifetime of borrowed references.
  Example:
  ```rust
  executor.spawn({
      let task_ran = task_ran.clone();
      async move {
          *task_ran.borrow_mut() = true;
      }
  });
  ```

## Agent skills

### Issue tracker

Issues and specs are tracked in GitHub Issues using the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Devlog tracking

YouTube devlog milestones and their issue history are managed with the `devlog-workflow` skill at `.agents/skills/devlog-workflow/SKILL.md`.

Design and activate milestones according to `docs/agents/milestones.md`. Keep future milestones at roadmap resolution and fully specify only the selected next milestone.

Before starting or completing issue-backed work, inspect the active devlog milestone. While one is active:

* Ensure each substantive feature, bugfix, or maintenance change has a GitHub issue.
* Keep unplanned work outside the milestone's `Planned work` checklist; it is tracked automatically as incidental work.
* After verification and before closing a work issue, use the skill to add its structured devlog note.

After a devlog milestone is closed, use the `write-devlog-script` skill when producing its YouTube narration and shot plan.

### Domain docs

This repository uses the single-context domain documentation layout. See `docs/agents/domain.md`.
