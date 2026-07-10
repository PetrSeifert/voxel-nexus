---
name: to-spec
description: Turn the current conversation into a spec and publish it to the project issue tracker — no interview, just synthesis of what has already been discussed. Use for ordinary implementation specs and for turning a selected roadmap outcome into a draft devlog milestone with a completion demo, bounded scope, risks, and capture plan.
---

# To Spec

Synthesize the current conversation and codebase understanding without interviewing the user. Do not silently invent missing decisions.

The issue tracker and triage label vocabulary should have been provided — run `/setup-matt-pocock-skills` if not.

## Prepare the spec

1. Explore the repository to understand its current state. Use the domain glossary vocabulary and respect applicable ADRs.
2. Identify the highest useful seam at which the behavior can be tested. Prefer an existing seam and minimize the number of new seams.
3. Use decisions already present in the conversation. Put unresolved material in Further notes instead of deciding it without evidence.
4. Choose milestone mode when the conversation or a wayfinder resolution identifies a devlog milestone; otherwise use ordinary-spec mode.

## Milestone mode

Read `docs/agents/milestones.md` and `.agents/skills/devlog-workflow/SKILL.md`.

Before publishing, query open issues labeled `devlog`. If one exists, do not create a second milestone; report the conflict unless the request is explicitly to revise the active one.

Write the selected next outcome using the draft milestone template from `docs/agents/milestones.md`. Ensure:

- The goal describes one completed outcome, not a module or implementation activity.
- The completion demo would visibly prove the goal.
- Success criteria are externally verifiable.
- Technical decisions stay above volatile file-level details.
- Technical approach contains only decisions explicitly supported by the conversation, codebase, ADRs, prototypes, or linked wayfinder resolutions. Mark every other choice `[UNRESOLVED]` in Further notes.
- Risks and non-goals bound the work tightly enough for one coherent devlog.
- The capture plan supports the important claims.

Publish it as an open GitHub issue titled `Milestone: <visible outcome>`. Preserve the `<!-- devlog-milestone -->` marker. Do not apply `devlog` or `ready-for-agent`: this issue is a draft planning parent, not an executable ticket, and its reporting window must not start yet.

After publication, hand the draft issue to `to-tickets`. That skill creates the implementation issues, fills `Planned work`, and activates the milestone after the ticket breakdown is approved.

## Ordinary-spec mode

Write the spec using the template below, publish it to the project issue tracker, and apply `ready-for-agent`. No additional triage is needed.

```markdown
## Problem Statement

<The problem from the user's perspective.>

## Solution

<The solution from the user's perspective.>

## User Stories

1. As an <actor>, I want <feature>, so that <benefit>.

Include the stories needed to cover the feature's behavior and important edge cases. Do not inflate the list with restatements.

## Implementation Decisions

<Modules and interfaces affected, architectural decisions, schemas, contracts, and specific interactions.>

Do not include file paths or code snippets because they become stale quickly. If a prototype snippet expresses a decision more precisely than prose, include only the decision-rich portion and identify it as coming from the prototype.

## Testing Decisions

<Externally observable behaviors, the modules tested through the chosen seams, and relevant prior art in the repository.>

## Out of Scope

<Explicitly excluded adjacent work.>

## Further Notes

<Unresolved material and relevant context.>
```
