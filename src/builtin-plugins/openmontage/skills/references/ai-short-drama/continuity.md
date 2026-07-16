# AI Short-drama Continuity Reference

## Stable identity layers

Keep these separate:

- invariant identity: face, age band, body type, hair baseline, signature features;
- wardrobe set: named outfit IDs and accessories;
- temporary state: dirt, injury, wetness, carried prop, emotion;
- shot state: pose, framing, screen direction, lighting.

Do not place temporary state into the permanent character description.

## Continuity ledger by shot

Track at least:

- character IDs present and their wardrobe/state;
- location ID, sub-area, time, weather, and lighting;
- prop IDs, owner, position, and damage state;
- entrances/exits, eyelines, and screen direction;
- what each character knows;
- dialogue refs and elapsed story time.

## Severity

- blocking: changes identity, causal logic, prop ownership, geography, or dialogue meaning.
- high: visible wardrobe/location/time mismatch across adjacent shots.
- medium: lighting, pose, or staging drift that harms polish but not comprehension.
- low: cosmetic variation unlikely to be noticed at delivery size.

Blocking and high issues should be resolved before generation or render. A report score must not hide a blocking issue.

## Regeneration strategy

When drift appears, regenerate the smallest dependent unit:

1. verify the approved reference and stable IDs;
2. simplify the shot action;
3. restate only the missing continuity constraint;
4. preserve seed/reference inputs where supported;
5. replace the specific candidate and re-run adjacent-shot review.

