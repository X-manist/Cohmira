# Executive Producer — AI Short-drama Pipeline

## Mission

Run an AI short-drama production as a sequence of reviewable artifacts, not as one giant prompt. The host AI is the creative intelligence. Python tools are authoritative only for persistence, schema validation, media execution, revisions, and user decisions.

Read `pipeline_defs/ai-short-drama.yaml`, this file, and `references/ai-short-drama/artifact-contracts.md` before starting.

## Non-negotiable authority split

- AI Agent: understands the source, proposes adaptation choices, writes the bible and screenplay, directs shots, judges quality, and explains tradeoffs.
- Skills: contain the production method and prompt knowledge.
- Python MCP: stores project state, validates artifacts, executes OpenMontage tools, and enforces optimistic revisions.
- MCP App: collects binding user selections and approvals.
- User: owns all creative gates and paid/long-running generation confirmation.

Never add a Python “writer”, “director”, “reviewer”, or hidden autonomous loop. If better creative behavior is needed, improve the appropriate Skill.

## Start protocol

1. Create the project with `drama_project_create`. Include source text when already available.
2. Record the returned `projectId` and `revision`.
3. For every stage, call `drama_stage_context(projectId, stage)` and use the returned latest revision.
4. Read the stage director named by `skillPath` before doing creative work.
5. Produce one schema-valid artifact at a time and save it with `drama_artifact_save`.
6. If the stage is a human gate, call `drama_open_review` and stop until the user acts in the MCP App.
7. After any UI selection or approval message, reload project state. Never continue from a stale revision.

## Stage loop

Run serially:

`research → proposal → script → scene_plan → assets → edit → compose → publish`

For each stage:

1. Verify required input artifacts and user decisions exist.
2. Carry forward stable IDs. Never silently rename a character, location, prop, episode, scene, dialogue line, or shot.
3. Produce only the artifacts declared by the pipeline.
4. Validate and save. A schema failure means revise the artifact, not bypass validation.
5. Review against pipeline `review_focus` and the continuity reference.
6. Open the correct UI view for binding choices.

## Revision protocol

Every mutating tool accepts `expectedRevision`.

- Pass the exact revision most recently returned by a tool.
- On `DRAMA_REVISION_CONFLICT`, discard the attempted write, reload, merge intentionally, and retry.
- Do not overwrite a user selection with an older chat assumption.
- A “revise” decision is a new creative assignment. Preserve the prior revision in history and save a new artifact.

## Human gates

The following stages must stop for the user:

- proposal: story direction, core cast, visual language
- script: episode and dialogue approval
- scene_plan: shot plan and continuity
- assets: character/location/shot candidates and spend
- compose: final cut

Use `drama_open_review` with the matching view. Chat can explain recommendations, but chat text is not a substitute for the persisted UI decision.

## Media execution

- Preflight with `provider_menu`, `list_tools`, and `tool_info`.
- Character references and stable environment references come before dependent shot generation.
- Video generation uses only `video_selector` or `seedance_video`, which route through JiubanAI.
- Use `run_tool` in dry-run mode first. Real execution requires `dryRun=false` and `confirm=true`.
- Register successful media through the existing media catalog flow.
- Long-running tasks must be represented in `generation_manifest`; never hide an in-flight job in chat context.

## Completion definition

A project is complete only when:

- every required artifact is schema-valid;
- all binding approvals are persisted;
- generated files are traceable to manifest IDs;
- final continuity, duration, audio, subtitle, and container checks pass;
- the approved master and requested variants are recorded in `publish_log`.

