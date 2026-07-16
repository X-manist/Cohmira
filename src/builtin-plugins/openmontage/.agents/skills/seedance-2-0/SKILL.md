---
name: seedance-2-0
description: |
  Generate cinematic Seedance clips through JiubanAI's built-in video service. OpenMontage supplies planning and prompts; JiubanAI owns Volcengine credentials, requests, polling, downloads, and media-library storage.
allowed-tools: Bash, Read, Write
metadata:
  jiuban:
    runtime: app-cli-video-generate
---

# Seedance 2.0 (ByteDance)

Seedance is the video-generation model family used by JiubanAI. In this plugin there is no provider marketplace: `video_selector` and `seedance_video` both route to the desktop app's `app_cli video generate` command. Never ask for or read provider keys inside OpenMontage.

## Why it is the OpenMontage premium default

| Capability | Seedance 2.0 | Notes |
|---|---|---|
| Single-pass native synced audio | Yes | Speech + SFX + ambience generated jointly, not post-sync |
| Multi-shot inside one generation | Yes | Multiple cuts/shots in a single prompt |
| Director-level camera control | Yes | Camera language (dolly, tilt, arc, crane, handheld) honored |
| Lip-sync from quoted dialogue | Yes | `Character says: "..."` matches mouth shapes |
| Reference conditioning | Up to 9 images + 3 video clips + 3 audio clips | 12-asset multimodal |
| Character identity consistency | Yes | Face/subject stable across shots |
| Max shot duration | 15 s | auto / 4–15 s |
| Resolution ceiling | 1080p on some endpoints (720p default on fal.ai) | Provider-dependent |
| Elo (Artificial Analysis) | 1269 (#1 as of Feb 2026) | Beat Veo 3, Sora 2, Runway Gen-4.5 |

Do not switch to another provider. If generation is unavailable, report the JiubanAI video configuration error and keep the approved plan/checkpoint intact.

## Runtime surface

| Surface | OpenMontage tool | Owner |
|---|---|---|
| JiubanAI local App Bridge | `video_selector` / `seedance_video` | Desktop `generateVideosToMediaLibrary` |

## Calling Seedance 2.0 inside OpenMontage

Call `video_selector` (recommended) or `seedance_video` directly. Both use the same JiubanAI runtime:

```python
from tools.tool_registry import registry
registry.ensure_discovered()
selector = registry.get("video_selector")
result = selector.execute({
    "prompt": PROMPT,
    "preferred_provider": "jiuban-video",
    "operation": "text_to_video",       # or image_to_video / reference_to_video
    "aspect_ratio": "16:9",             # 16:9 / 9:16
    "duration": "10",                   # 5..12
    "resolution": "720p",               # 720p / 1080p
    "output_path": "projects/<proj>/assets/video/clip_01.mp4",
})
```

Direct call to the provider tool (only when you must bypass the selector):

```python
seedance = registry.get("seedance_video")
seedance.execute({
    "prompt": PROMPT,
    "operation": "text_to_video",
    "aspect_ratio": "16:9",
    "duration": "10",
    "resolution": "720p",
    "generate_audio": True,
    "output_path": "...",
})
```

### Multi-clip continuity

For a video longer than one generated clip, do not independently animate every segment from the same still image.

1. Generate clip 1 from the approved image or text prompt.
2. Generate clip 2 and later clips with `operation="text_to_video"`, `first_clip=<previous managed clip path>`, and no original `image_path`.
3. Describe only the next action that has not happened yet. The continuation prompt must begin from the previous clip's final state and must not replay the opening action.
4. Stitch with `continuity_check="strict"`. If repeated-intro QA fails, regenerate the later clip with the correct `first_clip` rather than accepting the duplicate opening.

```python
clip_2 = seedance.execute({
    "prompt": "妈妈从上一段结束姿势继续，双手托住婴儿并轻轻抱起。",
    "operation": "text_to_video",
    "first_clip": clip_1.data["output"],
    "duration": 10,
    "aspect_ratio": "9:16",
})
```

## Prompt structure — The Higgsfield Methodology (canonical as of 2026)

**CRITICAL: Open every prompt with a shot-structure declaration.** Seedance rewards prompts that declare format upfront before any creative description. This is the single biggest quality lever.

### Opener templates (copy one verbatim, then extend)

**For action/combat/multi-shot (highest-performing format):**
```
Montage, multi-shot Hollywood action, don't use one camera angle or single cut, cinematic lighting, photorealistic, 35mm film quality, ARRI ALEXA aesthetic, heavy film grain, sharp but imperfect focus, motion blur on fast actions, halation on highlights, soft highlight rolloff, wide-angle lens with strong distortion, subtle chromatic aberration near frame edges, no 3D, no cartoon, no VFX aesthetic.
```

**For single-POV continuous shots (orbs, walkthrough):**
```
Single continuous shot, first-person POV perspective, the camera IS [his/her] eyes, hyper-chaotic handheld motion, completely unstabilized, violent raw human movement, constant micro-jitters, aggressive head swings, abrupt jerks, frequent over-rotation, no smoothness at all, no cuts, no zoom, 35mm film, photorealistic.
```

**For locked-POV reaction scenes:**
```
One continuous shot, POV [setting] perspective, no cuts, no zoom, natural head movement, photorealistic, 35mm film grain.
```

### Body structure (after the opener)

1. **Environment/location** — sensory detail (wet asphalt, sodium lamps, neon bleed, rain particulates, volumetric haze)
2. **Character block** — with reference tags and identity-lock language (see Reference-to-video below)
3. **Enemy/secondary character block** — same detail level
4. **Beat-by-beat choreography** with TEMPORAL MARKERS: `0–3s: …  3–6s: …  6–10s: …`
5. **VFX inline in brackets:** `[VFX: branching white-blue electric arcs pulsing along forearms, sparks jumping between fingers]`
6. **Slow-motion markers:** write `RAMPS TO SLOW MOTION` before the impact beat, `SNAPS BACK TO REAL TIME` on resume
7. **Sound design block:** either `no music, only raw SFX` or explicit SFX sequence. Music language stays textural.

### Combat vocabulary (proven to hit)

- `snaps forward`, `lunges`, `sprints`, `weaves`, `chambers`, `drives`, `pivots`, `redirects`, `ducks`, `slips`
- `explodes outward`, `devastating`, `raw force`, `kinetic`, `overload`, `compresses`, `erupts`, `fractures`, `ripples`
- Avoid soft verbs: `attacks`, `hits`, `fights` — these read generic and Seedance underdelivers on them

### Camera behavior — state what it IS and ISN'T doing

Seedance misfires when camera intent is ambiguous. Always explicitly negate what you don't want:
- `no cuts` (for continuous POV)
- `no zoom` (prevents unnatural perspective punch-ins)
- `no stabilization` (when you want chaotic handheld)
- `no smoothness at all`
- `no 3D, no cartoon, no VFX aesthetic` — counter-intuitive but forces photoreal skin/texture/lighting even when the scene has heavy VFX elements

### Realism enforcement phrase

When the brief has VFX but you want photoreal skin/textures (not plastic Marvel-cartoon look), include:
```
no 3D, no cartoon, no VFX aesthetic — photorealistic textures, real skin pores, authentic fabric detail, grounded in reality
```

### Format priority (Higgsfield empirical ordering)

| Format | Best for | Pattern |
|---|---|---|
| **Transformation** | calm → threat → transformation → aftermath | 6 numbered shots × 2.5s each @ 15s total |
| **Orbs** | single continuous POV | 1 shot × 15s, hyper-chaotic handheld |
| **Fights** | combat choreography | Beat-by-beat, clear power mismatch, RAMPS/SNAPS |
| **POV** | locked reaction | Continuous, "no cuts no zoom" mantra |
| **Animation** | stylized 3D | `@image` keyframe + timed segments |

The **2.5-second-per-shot rhythm** appears optimal for multi-shot generations.

## Legacy 8-part template (use only for single simple shots, not action)

Seedance 2.0 is unusually literal about camera language, multi-shot cuts, and quoted dialogue. Use this 8-part template:

```
[Shot / framing] + [Camera movement] +
[Subject description — physical detail that must persist across shots] +
[Action beat 1] → [optional cut] → [Action beat 2] +
[Setting / environment] + [Lighting / palette] +
[Style / grade / era] + [Audio — ambient, diegetic, music, dialogue]
```

### Multi-shot inside one generation

Seedance honors explicit shot lists inside a prompt. Format each shot:

```
Shot 1 (wide establishing, slow aerial push-in): ...
Shot 2 (medium close-up, handheld): ...
Shot 3 (extreme close-up, rack focus): ...
```

Keep subject description consistent across shots for identity stability.

### Lip-sync from quoted dialogue

```
Aang stands on the cliff edge, staff raised, wind in his cloak.
Aang says: "I won't run anymore."
Sokka, half a step behind, replies: "Then we fight."
```

Use `Character says: "..."` / `Character replies: "..."` exactly — mouth shapes key off quoted strings. Keep each line under ~6 words; longer lines risk drift on fast clips.

### Audio cues that work

Ambient: `distant thunder rolling over mountains`, `wind through reeds`, `crackling campfire`
Diegetic: `boots crunching snow`, `staff planting on stone`, `wingbeats overhead`
Music direction (light touch only): `low orchestral swell building`, `taiko drums entering on Shot 3`
Do **not** request complex multi-instrument scores — keep music language textural.

### Reference-to-video

When you have character / product / wardrobe references, use the reference-to-video endpoint. Seedance 2.0 honors an explicit bracket tagging syntax:

```
[reference_image: hero_portrait.png]
[identity_lock]
The same character — bald, blue arrow tattoo, orange robes — consistent across all shots, no drift or deformation. Do not alter clothing category or primary color.

Shot 1 (wide, slow push-in): hero walks across the snowy Air Temple courtyard, wind lifting robes.
Shot 2 (medium close-up): hero turns toward camera, staff in hand.
Shot 3 (extreme close-up, rack focus): hero's eyes open, wind whipping.
```

**Identity-anchor phrases that measurably reduce face drift** (stack them — redundancy helps):
- `the same character`
- `consistent across different scenes / all shots`
- `maintain exact appearance from reference image`
- `no deformation, no drift, no face morph`
- `Do not alter clothing category or primary color`

**Single-reference workflow (common in practice):** When you only have one photo:
- Use a clear, front-facing portrait with neutral lighting and minimal motion blur; avoid occluded faces (e.g., phones, sunglasses, heavy shadow).
- Reuse the SAME reference image across all shots — do not generate new refs per shot.
- Put all shots in ONE prompt under a single `[identity_lock]` block so the model treats them as a coherent sequence.
- If wardrobe is changing by design (e.g., civilian → costume), describe the costume verbatim on every shot it appears and add `Do not alter clothing category or primary color` to lock it once generated.

**Anti-drift fallback:** If face morphs across frames on first render, drop to a shorter duration (5-6s instead of 10s), tighten the identity-lock language, and if you have multiple reference images, cull to the 3 most consistent ones rather than flooding with 9.

## Parameter guidance

| Parameter | Guidance |
|---|---|
| `duration` | `5`–`8` for hero shots, `10`–`12` for full scenes with multi-shot cuts, `4` for quick inserts. `auto` when unsure. |
| `aspect_ratio` | `21:9` for cinematic trailers, `16:9` for broadcast / YouTube, `9:16` for Reels/Shorts/TikTok |
| `resolution` | `720p` default. Drop to `480p` for cost-capped batch previews, not for finals |
| `generate_audio` | Keep **on** unless you have a specific reason to mute — Seedance's moat is synced audio. Strip audio downstream in compose if needed. |
| `model_variant` | `standard` for hero/cinematic shots; `fast` only for b-roll, previews, or when latency is the hard constraint |
| `seed` | Set a seed before iterating variants of a chosen shot — everything else held constant |

## What to avoid

| Don't | Why |
|---|---|
| Cram four-plus simultaneous character actions into one shot | Motion coherence breaks; split into multi-shot |
| Request readable text / logos inside the clip | Text rendering is unreliable — handle text in Remotion overlay |
| Mix conflicting lighting ("bright noon" + "neon night") | Model picks one and ignores the other |
| Write dialogue longer than ~6 words on fast-cut shots | Lip-sync drift |
| Use `fast` variant for slow-mo, multi-shot, or complex camera moves | Routinely misses on first try — route to `standard` |
| Generate music through Seedance audio | Texture-only is fine; for real scoring use `music` / `pixabay_music` / `elevenlabs` and mix in compose |
| Bypass `video_selector` without a reason | Loses cost/availability/fallback handling and scoring context |

## Iteration strategy

1. **Block out shape** with a single `duration=5` `fast` T2V pass at the intended framing. Confirm the composition works.
2. **Lock the seed** once the composition reads.
3. **Upgrade to `standard`** with the same seed, tighten camera and lighting language.
4. **Extend and add shots** — move to multi-shot or longer duration only after a single-shot version is clean.
5. **Keep a per-clip README** with prompt + seed + variant for every shot that makes the cut, so the compose stage can re-render consistent retakes.

## Integration notes for OpenMontage pipelines

- **Cinematic pipeline:** Seedance 2.0 is the default video model. Use 21:9 for hero, multi-shot for montage beats, reference-to-video when the brief has a visual bible.
- **Animated explainer:** Use Seedance 2.0 for the establishing / mood clips only; most shots should stay in Remotion. Don't replace Remotion motion graphics with Seedance — different tool, different job.
- **Screen demo / podcast / clip factory:** Seedance is not the right default — these are footage-led. Only use for stylized cold-opens.
- **Cost discipline:** `standard` at 10 s ≈ $3.03 per clip. Budget accordingly in the proposal stage. `fast` at 5 s ≈ $1.21 for previews.

## Verification checklist for every Seedance shot

- [ ] Motion reads coherently at the chosen shot length
- [ ] Audio is actually synced (check dialogue + foot/impact hits)
- [ ] Character identity matches reference / prior shots
- [ ] Camera direction matches the prompt (no auto-dolly when you asked for static)
- [ ] No readable text the model tried to render
- [ ] Grade matches the approved style playbook
- [ ] Output duration matches what you requested (some endpoints round)

## Sources

- fal.ai Seedance 2.0: https://fal.ai/seedance-2.0
- fal.ai how-to-use: https://fal.ai/learn/tools/how-to-use-seedance-2-0
- Replicate bytedance collection: https://replicate.com/bytedance
- HeyGen Seedance 2.0: https://www.heygen.com/blog/introducing-seedance-2-and-heygen
- Runway Seedance: https://runwayml.com/product/seedance
- BytePlus Dreamina Seedance 2.0: https://www.byteplus.com/en/product/seedance
- Freepik Seedance 2.0: https://www.freepik.com/seedance-2
- Higgsfield Seedance 2.0: https://higgsfield.ai/seedance/2.0
- Pollo Seedance 2.0: https://pollo.ai/m/seedance/seedance-2-0
- ByteDance Seed official: https://seed.bytedance.com/en/seedance2_0
- Seedance 2.0 Wikipedia: https://en.wikipedia.org/wiki/Seedance_2.0
