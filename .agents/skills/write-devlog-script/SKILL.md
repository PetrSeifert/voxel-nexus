---
name: write-devlog-script
description: Turn a finished GitHub devlog milestone into a factual, production-ready YouTube script with narration and shot directions. Use after an issue labeled `devlog` has been closed, when drafting or revising the milestone's video narrative, voice-over, visual plan, or evidence-linked devlog script.
---

# Write Devlog Script

Write a coherent video story from the frozen milestone inventory. Use issue notes as evidence, not as the final structure.

## Resolve the source milestone

Use the requested milestone issue. If none is given, list recently closed issues labeled `devlog` and select the most recent unambiguous match:

```text
gh issue list --state closed --label devlog --limit 10 --json number,title,closedAt,url
```

Verify the selected issue is closed. Do not produce a final script from an open milestone; use `devlog-workflow` to finish it first.

Generate its frozen inventory:

```text
python .agents/skills/devlog-workflow/scripts/devlog.py report --issue <number>
```

Read the milestone goal, completed planned work, incidental work, structured notes, evidence, and capture gaps. Follow issue links or inspect related code and pull requests only when necessary to substantiate a claim. Never invent behavior, measurements, motivations, setbacks, or footage.

## Establish the production target

Follow any audience, tone, or duration supplied by the user. Otherwise use these defaults:

- Target 6–8 minutes, roughly 780–1,040 spoken words at 130 words per minute.
- Address voxel-engine enthusiasts and technically curious developers.
- Use direct, conversational first-person narration without fabricating personal reactions.
- Explain why technical work matters on screen before diving into implementation details.

Mark material uncertainty as `[NEEDS INPUT: ...]`. Keep capture suggestions distinct from footage known to exist.

## Build the story

Organize the script around cause and effect rather than reciting issue numbers:

1. Open with the most visible result or compelling problem.
2. State the milestone goal and why it matters to the engine or player.
3. Develop the main planned work as a small number of narrative beats.
4. Integrate incidental fixes where they affected the journey; group minor fixes into a concise montage when appropriate.
5. Demonstrate the final outcome using recorded evidence.
6. Close with an honest retrospective and a short next-step tease grounded in known plans.

Give every important claim a supporting issue or verified artifact. Prefer before/after demonstrations, diagrams, profiling captures, tests, and gameplay over generic editor footage.

## Write the artifact

Copy the structure from `assets/devlog-script-template.md` into:

```text
docs/devlogs/<milestone-number>-<short-slug>.md
```

Create `docs/devlogs/` when needed. If the target already exists, read it and preserve intentional edits while revising.

For each beat:

- Write speakable narration with natural transitions.
- Add shots that directly prove or clarify the narration.
- Label unrecorded suggestions with `Capture`; label confirmed material with `Existing` only when verified.
- Estimate timestamps from word count after drafting; do not imply frame-accurate timing.
- Link factual claims to their milestone issue, work issue, pull request, test, or measurement in the production-only source map.

Do not include issue numbers in spoken narration unless they are part of the story.

## Review the draft

Before handing off the script:

1. Read narration aloud mentally and shorten awkward sentences.
2. Check that the hook pays off and the milestone goal remains the main arc.
3. Check every factual or numerical claim against a source.
4. Ensure planned and incidental work are both represented without making minor fixes dominate.
5. Ensure every technical explanation has a useful visual.
6. Report estimated word count and duration, unresolved input markers, and the output path.
