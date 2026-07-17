---
name: devlog-workflow
description: Track YouTube devlog milestones in GitHub Issues without losing unrelated bugfixes and maintenance work. Use when starting, inspecting, updating, reporting, or finishing an issue labeled `devlog`, and whenever Codex starts or completes issue-backed work while a devlog milestone is active.
---

# Devlog Workflow

Use one open issue labeled `devlog` as the active milestone. Treat its `Planned work` checklist as milestone scope. Treat every other issue closed during the milestone window as incidental work for the devlog. Read `docs/agents/milestones.md` when designing or activating a milestone.

Read `docs/agents/issue-tracker.md` before making GitHub changes. Run `scripts/devlog.py` from this skill for the operations below; it infers the repository from the current checkout.

## Inspect the active milestone

Run:

```text
python .agents/skills/devlog-workflow/scripts/devlog.py active
```

Stop and resolve the ambiguity if more than one open issue has the `devlog` label. No active milestone is a valid state.

## Start a milestone

For a milestone produced through `wayfinder` and `to-spec`, use `to-tickets` on the draft milestone. It publishes the planned work checklist and applies `devlog` only after the ticket breakdown is approved.

For a small milestone that is already fully shaped without a draft spec, create or identify the planned work issues first. Then run:

```text
python .agents/skills/devlog-workflow/scripts/devlog.py start --title "<title>" --goal "<viewer-facing outcome>" --planned <issue numbers>
```

Keep only intended milestone work in `Planned work`. Do not add later bugfixes or maintenance issues to that checklist merely because they happened during the milestone; the report discovers them as incidental work.

Use exactly one active devlog milestone. Do not start another until the current one is finished.

## Record completed issue work

When an active milestone exists, ensure each substantive feature, bugfix, or maintenance change has a GitHub issue before completing it. An unplanned issue remains outside the milestone checklist.

After verifying the work and before closing its issue, add a structured devlog note:

```text
python .agents/skills/devlog-workflow/scripts/devlog.py note --issue <number> --impact "<why a viewer or player should care>" --before "<previous behavior>" --after "<new behavior>" --evidence "<tests, measurements, or manual proof>" --visual "<footage or graphic worth capturing>"
```

Require `--impact` and `--evidence`. Omit the other fields only when they do not apply. Record observed facts; do not invent measurements or footage.

## Prepare a devlog

Run the read-only report:

```text
python .agents/skills/devlog-workflow/scripts/devlog.py report
```

Pass `--issue <number>` to report on a closed milestone. The report separates completed planned work, open planned work, and incidental issues closed between the milestone's creation and closure. It also flags completed issues that lack a structured note.

Use the inventory as source material for a script or shot list. Preserve links and distinguish verified evidence from suggested visuals.

## Finish a milestone

Only finish when the user explicitly asks. First run `report` and resolve missing notes. Require every planned issue to be closed unless the user explicitly accepts the reduced scope.

Then run:

```text
python .agents/skills/devlog-workflow/scripts/devlog.py finish
```

Pass `--allow-open-planned` only after explicit acceptance. `finish` posts the generated inventory to the milestone issue and closes it, freezing the reporting window.

After the milestone is closed, use `write-devlog-script` to turn the frozen inventory into the narration and shot plan.
