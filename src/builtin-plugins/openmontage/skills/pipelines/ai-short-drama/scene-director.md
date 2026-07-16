# Scene Director — AI Short-drama

## Goal

Translate the approved screenplay into an executable director storyboard while protecting identity and spatial continuity.

Read `references/ai-short-drama/storyboard-direction.md` and `continuity.md`.

## Shot design rules

- One shot should have one dominant visual intention and one dominant action.
- Assign stable shot IDs that encode episode and scene order.
- Every shot declares duration, size, camera behavior, composition, action, emotion, dialogue refs, prompt, and continuity anchors.
- A prompt describes what should be visible in this shot; it does not retell the whole plot.
- Use camera movement only when it reveals, follows, isolates, or transforms meaning.
- Preserve screen direction across cuts unless a neutral reset or intentional reversal is planned.
- Avoid unmotivated coverage. Each close-up, insert, reaction, and establishing shot must earn its time.

## AI generation constraints

- Keep simultaneous actions simple enough for the selected model.
- Split identity-critical dialogue and complex physical action when needed.
- Reuse character/location reference assets instead of restating unstable appearance prose.
- Put stable identity in asset references and shot-specific state in the prompt.
- Write explicit negative constraints for common drift: extra fingers, changed wardrobe, age shift, face swap, unreadable text, duplicate people, and broken props.

## Continuity review

Run a dedicated pass across all shots and produce `continuity_report`:

- character face/body/wardrobe state;
- location geography and time of day;
- prop possession and damage state;
- eyelines and screen direction;
- dialogue/action timing;
- story knowledge and causal order.

Any blocking issue must be fixed in `drama_storyboard` before opening the `storyboard` UI gate.

