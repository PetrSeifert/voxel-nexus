---
name: write-devlog-script
description: Turn a finished GitHub devlog milestone into a factual, production-ready YouTube story with conversational narration, shot directions, evidence, tradeoffs, and editing notes. Use after an issue labeled `devlog` has been closed when drafting or revising a milestone video, including a first or genesis episode that must introduce the project and series premise.
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

## Establish the episode premise

Decide whether this is a genesis episode or a continuation. For a genesis episode, also read the project's domain summary, roadmap, and decision documents needed to explain what is being built and why the series exists.

Before outlining, write down six private one-sentence answers:

1. What future result should make the viewer care?
2. What project-level bet or constraint makes this series distinct?
3. What contrast makes this episode interesting?
4. What central decision or tradeoff shaped the work?
5. What honest visible payoff proves progress?
6. What specific next promise closes the episode?

Build the script around those answers, not around the issue inventory.

Do not infer personal motives, tastes, feelings, or remembered setbacks from code. If a genesis hook needs creator reference footage or a personal reason that is not documented, ask one concise question or mark `[NEEDS INPUT: ...]`. Commit sequences prove iteration; they do not prove what looked broken, felt difficult, or happened between commits.

## Establish the production target

Follow any audience, tone, or duration supplied by the user. Otherwise use these defaults:

- Target 8–10 minutes. Start around 900–1,150 spoken words when the edit includes silent hooks, held results, or montages; use up to roughly 1,300 only for narration-heavy episodes. Estimate final runtime from narration plus explicit visual breathing room, not words alone.
- Address voxel-engine enthusiasts and technically curious developers.
- Use direct, conversational first-person narration without fabricating personal reactions.
- Explain why technical work matters on screen before diving into implementation details.
- Balance useful technical information with restrained humor. Aim for an informed devlog, not a tutorial and not a comedy sketch.
- Prefer one memorable contrast, one central tradeoff, and two to four technical ideas over exhaustive coverage.

Mark material uncertainty as `[NEEDS INPUT: ...]`. Keep capture suggestions distinct from footage known to exist.

## Build the story

Organize the script around cause, effect, and contrast rather than reciting issue numbers:

1. **Hook:** Show the future vision, strongest result, or clearest problem before setup. Establish the gap between what the project wants and what this episode can honestly deliver.
2. **Why this project or milestone:** Explain the project-level bet and why this step exists. In a genesis episode, ensure a new viewer understands the series goal before implementation details.
3. **Decision and messy middle:** Frame the work through one consequential tradeoff and one evidence-backed sequence of iteration. Use commits, diffs, code, or tests as receipts rather than as the structure itself.
4. **Payoff:** Let the visible result land, even when it is small. Explain what the unglamorous proof establishes without pretending it is more visually impressive than it is.
5. **Next promise:** Return to the larger vision and name one grounded next outcome.

Integrate incidental fixes only where they affected that arc. Group minor work into a short montage or reserve it for editing notes.

When technology choices matter, explain each with a compact `what it does → why it fits → tradeoff` progression. Do not turn the script into a dependency catalogue or setup tutorial.

## Turn evidence into screen story

Classify source material before writing:

- **Story evidence:** decisions, reversals, review fixes, and commit sequences that change the narrative.
- **Proof:** runtime footage, tests, measurements, and retained artifacts that substantiate the payoff.
- **Texture:** code, issue text, logs, and documentation that make the work concrete without requiring narration to recite them.

Use only the strongest material from each category. The milestone inventory is a source pool, not a checklist for spoken coverage.

Prefer a few legible visuals over a production dossier:

- Hold the strongest visible result long enough to register.
- Keep code, commit, log, and document excerpts brief and identify the exact line or change the viewer should notice.
- Use diagrams only when they materially simplify a relationship or state transition; usually zero to two are enough.
- Interleave shot cues at the exact narration transition where the editor needs them. Do not make the editor map a long narration block back to a detached shot list.
- Treat aspirational or third-party reference footage as `Capture` until the creator supplies and clears it. Never label it `Existing` merely because the script wants it.
- Put useful but nonessential footage ideas, missing messy-middle material, and reserved topics in editing notes rather than forcing them into narration.

## Control the voice

- Write short, speakable paragraphs with contractions and natural transitions.
- Use humor through honest contrast, understatement, or technical absurdity. Keep it brief—normally no more than one joke in a beat—and never obscure the explanation.
- Explain the consequence before the mechanism. Show only enough mechanism to make the decision valuable.
- Do not narrate a tutorial sequence. Avoid installation steps, API tours, dependency inventories, and line-by-line code explanation unless the episode is explicitly about them.
- Do not include issue numbers or raw test inventories in spoken narration unless they are genuinely part of the story.
- Never invent a creator reaction such as “I wanted,” “I learned,” or “this finally clicked” without a source or creator confirmation.

Give every important claim a supporting issue or verified artifact. Prefer before/after demonstrations, diagrams, profiling captures, tests, and gameplay over generic editor footage.

## Write the artifact

Copy the structure from `assets/devlog-script-template.md` into:

```text
docs/devlogs/<milestone-number>-<short-slug>.md
```

Create `docs/devlogs/` when needed. If the target already exists, read it and preserve intentional edits while revising.

For each beat:

- Write speakable narration with natural transitions.
- Interleave shots that directly prove or clarify the narration. Use separate narration and shot subsections only when the user requests that format.
- Label unrecorded suggestions with `Capture`; label confirmed material with `Existing` only when verified.
- Estimate timestamps from word count after drafting; do not imply frame-accurate timing.
- Link factual claims to their milestone issue, work issue, pull request, test, or measurement in the production-only source map.

Add concise editing notes covering the strongest messy-middle material, capture gaps worth investigating, and good material deliberately reserved for another episode.

Do not include issue numbers in spoken narration unless they are part of the story.

## Review the draft

Before handing off the script:

1. Read narration aloud mentally and shorten awkward sentences.
2. Check that the first 30 seconds creates curiosity and that a new viewer understands what is being built and why within roughly 90 seconds.
3. Check that the hook pays off, one tradeoff drives the middle, and the visible result receives an honest reaction.
4. Remove technical material that is accurate but does not strengthen the story.
5. Check every factual or numerical claim against a source; move speculation to `[NEEDS INPUT]` or conditional editing notes.
6. Ensure planned and incidental work are represented without making minor fixes dominate.
7. Ensure every retained technical explanation has a useful visual, but do not create redundant diagrams.
8. Check that jokes are sparse, natural, and never replace information.
9. Report estimated word count and duration, unresolved input markers, and the output path.
