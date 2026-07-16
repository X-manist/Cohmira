# OpenMontage AI Short-drama Plugin

## Architecture

The short-drama extension follows the existing OpenMontage agent-first model:

| Layer | Responsibility |
|---|---|
| Host AI | intent understanding, adaptation, writing, directing, recommendations, review |
| Skills | reusable production methods and prompt knowledge |
| Python MCP | state, schemas, revisions, tools, media execution, approvals |
| MCP App | deterministic choices and review UI |
| Filesystem | authoritative project artifacts and revision history |

Python intentionally does not contain a creative orchestrator. Improving a writer/director behavior means updating
`skills/pipelines/ai-short-drama/` or its references, not adding a hidden model loop to Python.

## Runtime

The plugin manifest points to `mcp/server.py`. JiubanAI starts it with its bundled `uv` and Python 3.11:

```text
uv run --managed-python --python 3.11 --no-project
  --with-requirements <plugin>/requirements.txt
  <plugin>/mcp/server.py
```

Useful environment variables:

| Variable | Purpose |
|---|---|
| `OPENMONTAGE_ROOT` | installed plugin root |
| `OPENMONTAGE_PROJECTS_DIR` | persistent user project data |
| `OPENMONTAGE_UV` | app-bundled uv path |
| `UV_CACHE_DIR` | shared dependency cache |
| `UV_PYTHON_INSTALL_DIR` | managed Python cache |
| `BEAV_MEDIA_ROOT` | JiubanAI media library root |

## Project layout

```text
<projects>/<project-id>/
├── project.json
├── short_drama/
│   ├── state.json
│   ├── events.jsonl
│   └── history/revision-000001.json
├── artifacts/
├── assets/{images,video,audio,music}/
└── renders/
```

`state.json` owns the global revision, stage state, artifact descriptors, selections, and approvals. Artifacts remain
separate JSON files so they can be validated, reviewed, diffed, and reused by existing OpenMontage tooling.

## Revision behavior

All writes support optimistic concurrency through `expectedRevision`.

1. Read project/stage context.
2. Use the returned revision for the next write.
3. A successful write increments the revision and archives the previous state.
4. `DRAMA_REVISION_CONFLICT` means another UI/agent action won; reload before retrying.

This prevents a delayed AI response from overwriting a newer user choice.

## MCP App

The server exposes one self-contained resource:

```text
ui://openmontage/short-drama
text/html;profile=mcp-app
```

It has no external network or CDN permissions. It receives the triggering tool's `structuredContent`, renders the
current project, and uses app-visible tools to refresh state, save selections, and approve/revise stages. It then
sends a concise chat message so the host AI knows that a binding decision occurred.

Views:

- overview and stage rail;
- story bible and episode plan;
- character selection;
- screenplay review;
- storyboard/shot selection;
- generated asset selection;
- voice selection;
- render/final-review and approval history.

## Pipeline

`pipeline_defs/ai-short-drama.yaml` uses the standard OpenMontage stage names:

`research → proposal → script → scene_plan → assets → edit → compose → publish`

Creative gates stop at proposal, script, scene plan, assets, and final compose. Media generation remains restricted
to JiubanAI's supported `video_selector` / `seedance_video` bridge.

## Testing

Fast MCP/domain tests require only the schema dependencies:

```bash
UV_CACHE_DIR=/tmp/openmontage-uv-cache \
UV_PYTHON_INSTALL_DIR=/tmp/openmontage-uv-python \
uv run --managed-python --python 3.11 --no-project \
  --with pyyaml --with jsonschema \
  python -m unittest discover -s tests -v
```

Before release, also launch the real entrypoint with `requirements.txt`, list tools/resources over stdio, open the MCP
App in JiubanAI, and complete a create → artifact → UI decision → revision-resume smoke flow.

