# Edit Director — AI Short-drama

## Goal

Turn approved shots and audio plans into a deterministic episode timeline with readable dialogue, controlled pacing, and reproducible edit decisions.

## Voice plan

- Keep one stable voice identity per character unless the story explicitly changes it.
- Describe performance intention per character, not merely gender/age.
- Use dialogue-level overrides only for exceptional emotional beats.
- Confirm pronunciation, speed, pauses, overlap, and whether lip sync is required.
- Open the `voice` UI when voice choices need user confirmation.

## Edit construction

- Build the cut from storyboard IDs and approved asset IDs.
- Preserve the episode hook and cliffhanger timing.
- Cut on action, reaction, information reveal, or audio motivation—not at arbitrary equal intervals.
- Let reaction shots breathe where emotional comprehension matters.
- Use subtitles inside the target platform safe area and keep each line readable.
- Duck music under dialogue; never solve weak dialogue by simply making everything louder.
- Record trims, retimes, replacements, transitions, subtitle policy, and audio decisions in `edit_decisions`.

## Verification

- Sum actual durations and compare with the project target.
- Check missing/duplicate shots, frozen frames, black frames, broken audio, subtitle overflow, and dialogue/action mismatch.
- If a source asset is unusable, send back a specific shot or asset ID rather than regenerating the whole project.

