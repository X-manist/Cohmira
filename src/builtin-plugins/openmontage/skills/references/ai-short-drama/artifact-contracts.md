# AI Short-drama Artifact and Tool Contracts

## State contract

`projectId` identifies the workspace. `revision` is a monotonically increasing global version.

All writes should pass `expectedRevision` from the immediately preceding tool result. If a UI action wins the race, the next agent write must reload and intentionally merge.

## Tool sequence

```text
drama_project_create
  → drama_stage_context(stage)
  → read returned skillPath
  → create one artifact
  → drama_artifact_save(expectedRevision)
  → use returned revision for the next artifact
  → drama_open_review at human gates
```

The MCP App calls `drama_selection_commit`, `drama_stage_decide`, and `drama_ui_refresh`. Treat its decisions as authoritative project state.

## Stable ID prefixes

| Object | Recommended form |
|---|---|
| episode | `ep_001` |
| scene | `ep_001_sc_001` |
| dialogue | `ep_001_sc_001_dl_001` |
| shot | `ep_001_sc_001_sh_001` |
| character | `char_<slug>` |
| location | `loc_<slug>` |
| prop | `prop_<slug>` |
| asset candidate | `asset_<subject>_<nn>` |

IDs remain stable across revisions. If an object is deleted, do not recycle its ID for a different object.

## Artifact ownership

- `source_document`: normalized user source; never generated as fake canon.
- `episode_plan`: series/episode dramatic architecture.
- `drama_bible`: stable world, cast, visual language, continuity anchors.
- `screenplay`: executable scenes, actions, and dialogue.
- `drama_storyboard`: executable shots tied to screenplay and bible IDs.
- `continuity_report`: explicit pass/warn/fail findings.
- `generation_manifest`: all planned/running/completed generation jobs.
- `selection_manifest`: binding user-selected IDs.
- `asset_manifest`: final files and provenance.
- `voice_plan`: stable voice identity and performance direction.
- `edit_decisions`: reproducible timeline choices.
- `render_report` / `final_review`: actual output verification.
- `publish_log`: approved deliverables and registration/upload state.

## Validation behavior

`drama_artifact_save` rejects schema-invalid data. Fix the object. Do not bypass validation, write files manually, or stuff creative content into project metadata.

