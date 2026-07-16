# Compose Director — AI Short-drama

## Goal

Render the approved edit, perform final technical and creative QA, and present a clearly identified cut for user approval.

## Render rules

- Use the project aspect ratio, resolution, frame rate, and audio profile consistently.
- Keep a deterministic render command or composition configuration in the report.
- Never overwrite the last approved render; create a new versioned output.
- Register successful local media with the JiubanAI media library flow.

## Final review

Probe and inspect the actual output, not only the timeline plan:

- file opens and duration is within tolerance;
- frame size, frame rate, codec, and audio stream are valid;
- no black/frozen/duplicate frames or accidental watermarks;
- dialogue is intelligible and synchronized;
- subtitles remain in safe zones and match spoken content;
- character, wardrobe, location, prop, and screen direction continuity hold;
- episode hook and final beat land at intended times;
- no placeholder, test asset, or rejected candidate remains.

Save `render_report` and `final_review`. Open the `render` UI and stop. Only the persisted approval identifies the deliverable master.

