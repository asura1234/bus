# Skill: create-workflow

Use this skill after the Room Brief is confirmed, when the room needs a workflow: selecting a built-in SOP, adapting one, or writing a new one.

## 1. Understand the work

Re-read the confirmed Goal and Non-goals and the room facts. When requirements are unclear, ask with `SendMessage`: the Human for intent and authority, a coding agent for technical feasibility.

## 2. Select a source

- **Built-in SOP**: choose a reference from the index and read it with `ReadContent`, for example `reference/sop-review-plan`. A built-in SOP can be executed as written. Do not create a draft just to read it.
- **Adapted or new SOP**: only when you decide the room needs a different process, read `reference/workflow-template` with `ReadContent` and write the SOP as Markdown with exactly one Mermaid flowchart.

The current SOP source is always either a `ReadContent` reference name or a `ReadWorkflowDraft` draft id. It is never a filesystem path.

## 3. Persist an adapted SOP

Call `PersistWorkflowDraft` with `draft_id: null` and `expected_revision: null` to create a draft, or with the draft id and its current revision to revise it. Set `workflow_id` only when the draft is meant to become a named standard workflow; that target is fixed when the draft is created. The runtime derives where the draft lives; you never submit a path or filename. Read the result back with `ReadWorkflowDraft` using the draft id from the receipt.

Keep the diagram and the prose in agreement. The diagram is a map for participants; it is not executable, and room facts decide what actually happens.

## 4. Share the plan

Send the Human an advisory summary with `SendMessage`: the SOP source, to-do items, participants and roles, models and effort, resources, and permission needs. This is not an approval step. Continue with `skill/execute-workflow` unless the Human redirects you. If the plan needs authority or scope beyond the confirmed Room Brief, ask the Human for a new room or a Human-controlled action instead.

## 5. Publish a standard workflow

Publication is separate from running a workflow.

1. The draft must have a `workflow_id` target.
2. The Human reviews the immutable draft revision, its content, the current standard base, and the exact diff in the Human review surface, and creates the approval there.
3. Only after that approval exists may you or the Human call `PromoteWorkflowDraft` with its `approval_id`. The runtime re-verifies the approval and settles the operation.

A published standard workflow is a record for later incorporation into the built-in content. You do not read it back; keep running from your current SOP source.
