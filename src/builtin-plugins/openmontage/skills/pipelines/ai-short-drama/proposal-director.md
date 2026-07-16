# Proposal Director — AI Short-drama

## Goal

Convert source truth and project settings into a producible series plan and a stable drama bible. This is the first binding creative gate.

Read `references/ai-short-drama/adaptation.md` and `continuity.md`.

## Before writing artifacts

Present 2–3 genuinely different adaptation directions in chat. Each option must state:

- what it preserves from the source;
- what it compresses, merges, reorders, or invents;
- episode structure and cliffhanger strategy;
- visual treatment and generation difficulty;
- continuity risk, likely asset cost, and tradeoffs.

Do not save a final bible until the user selects a direction or clearly delegates the choice.

## `episode_plan` rules

- Match project `episodeCount` and `episodeDurationSeconds`.
- Give every episode a first-three-seconds hook, one central turn, and an ending payoff or cliffhanger.
- Treat episodes as a dependency chain: later reveals must have setup.
- Keep source references for adapted beats.
- Avoid repeating the same hook mechanism every episode.

## `drama_bible` rules

- Assign stable IDs before any asset generation: `char_*`, `loc_*`, `prop_*`.
- Character appearance must be visible and reproducible, not vague adjectives.
- Separate stable identity from episode-specific wardrobe or damage states.
- State motivation, arc, relationships, voice profile, and forbidden drift.
- Define world rules, time period, location anchors, palette, camera language, lighting, and continuity rules.
- If the source is ambiguous, mark the chosen adaptation interpretation in metadata.

## Review order

1. Save `episode_plan`.
2. Save `drama_bible` using the latest returned revision.
3. Open `drama_open_review(projectId, view="bible")`.
4. Let the user inspect story direction.
5. Open or switch to `cast` for role emphasis/selection.
6. Stop until the proposal stage is approved.

The user’s persisted choice wins over any earlier recommendation.

