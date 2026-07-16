# Asset Director — AI Short-drama

## Goal

Generate and select reusable visual/audio assets in dependency order, with every job traceable to a stable manifest item.

## Dependency order

1. Approved character identity references.
2. Approved location and prop references.
3. Wardrobe/state variants.
4. Storyboard frames or shot keyframes.
5. Motion clips derived from approved references.
6. Voice, music, and sound assets needed by edit.

Do not start dependent shot generation before the relevant identity references are selected.

## Manifest protocol

Create `generation_manifest` before spending money or launching long tasks. Every item needs:

- stable ID, asset type, subject ID, status, and prompt;
- provider/model/seed when known;
- candidate group and output path;
- explicit failure state instead of disappearing from the plan.

Update status as tools return. Never represent an untracked local file as an approved asset.

## Provider rules

- Use `image_selector` for images unless the user explicitly chooses a supported provider.
- Use only `video_selector` or `seedance_video` for video generation; both route through JiubanAI.
- Read each selected tool’s Layer 3 `agent_skills` before crafting provider prompts.
- Dry-run first. Paid or long-running execution requires explicit confirmation.
- Keep seeds and reference inputs when supported so failed shots can be reproduced.

## Candidate strategy

- Generate a small, purposeful candidate set, not an unbounded batch.
- Vary one decision at a time when diagnosing quality.
- Preserve candidate IDs even when rejected.
- Explain your recommendation, then open the `cast`, `assets`, or `storyboard` UI view.
- Continue only from the persisted selection.

After selection, save `selection_manifest` and `asset_manifest` with final paths and provenance.

