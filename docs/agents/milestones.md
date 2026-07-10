# Devlog milestone design

Use rolling-wave planning: keep the next three to five milestone outcomes visible, but fully design and commit only the next milestone.

## Terms

- **Milestone candidate**: a one-sentence future outcome. It belongs in a roadmap or wayfinder resolution, not in an open `devlog` issue.
- **Draft milestone**: the selected next outcome expressed as an issue containing `<!-- devlog-milestone -->`. It is not yet labeled `devlog` and its reporting window has not started.
- **Active milestone**: the one open issue labeled `devlog`. Adding the label starts its reporting window.
- **Planned work**: implementation issues linked from the active milestone's `Planned work` checklist.
- **Incidental work**: any other substantive issue closed while the milestone is active. Do not add it to `Planned work` after the fact.

Allow at most one active milestone. Keep later candidates at low resolution so discoveries from the current milestone can change their order and scope.

## Design a milestone

Shape a milestone around one coherent, demonstrable outcome rather than a subsystem or task category. Prefer “edit a block and see the chunk mesh update” over “build the chunk module.”

A milestone is ready to become a draft when it has:

1. **Goal**: one sentence describing the completed outcome.
2. **Viewer payoff**: why a player or devlog viewer should care.
3. **Completion demo**: the shortest observable sequence that proves the goal.
4. **Success criteria**: externally verifiable behaviors, including relevant quality or performance constraints.
5. **Technical shape**: the important seams and decisions already known, without volatile file-level instructions.
6. **Risks and rabbit holes**: uncertainties that could invalidate the scope or demonstration.
7. **Non-goals**: adjacent work deliberately excluded.
8. **Capture plan**: useful before/after footage, diagrams, measurements, or tests.

Treat these as evidence requirements, not invitations to fill gaps with plausible ideas. If a completion demo, seam, constraint, or risk treatment has not been decided, keep it unresolved and use wayfinder to resolve it before activation.

Split a candidate when it contains independent payoffs, cannot be demonstrated as one story, or has risks too large to bound. Combine candidates when neither produces a meaningful result on its own.

## Create and activate a milestone

Use this sequence:

1. Use `wayfinder` when the desired engine direction spans multiple uncertain milestones. Keep three to five candidate outcomes at headline level and select the next one.
2. Use `to-spec` to publish the selected outcome as a draft milestone issue. Do not apply `devlog` or `ready-for-agent` yet.
3. Use `to-tickets` on the draft milestone. Approve vertical implementation slices and their blocking edges before publication.
4. Publish the work issues, replace the draft's `Planned work` placeholder with their linked checklist, and then apply `devlog` to activate it.
5. Use `devlog-workflow` during implementation and milestone completion.

Do not start implementation before activation. Do not create detailed issues for later milestone candidates.

## Draft milestone issue

```markdown
<!-- devlog-milestone -->
## Goal

<One completed, viewer-facing outcome.>

## Viewer payoff

<Why this change is worth seeing or using.>

## Completion demo

<Observable sequence that proves the milestone is complete.>

## Success criteria

- [ ] <Externally verifiable behavior or constraint.>

## Planned work

_Populated by `to-tickets` before activation._

## Technical approach

<Stable decisions, seams, and constraints; no file-level implementation plan.>

## Testing decisions

<Highest useful test seam and the behaviors that must be proven.>

## Risks and rabbit holes

- <Risk and how scope avoids or answers it.>

## Non-goals

- <Explicitly excluded adjacent work.>

## Devlog capture plan

- <Before/after footage, diagram, test, or measurement.>

## Further notes

<Relevant context and links.>
```
