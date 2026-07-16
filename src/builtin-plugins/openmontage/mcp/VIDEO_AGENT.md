# OpenMontage Video Agent

This MCP package is intended to be mounted in a dedicated Goose video sub-agent session.
The main operations agent should delegate video generation or editing tasks through Beav MCP's
`delegate_video_agent` tool instead of mounting all OpenMontage tools directly.

## Runtime Contract

- Use `list_pipelines`, `get_pipeline`, `list_tools`, `tool_info`, and `provider_menu` for planning.
- Use `run_tool` for execution. Real paid generation requires `dryRun=false` and `confirm=true`.
- Successful local media artifacts are registered into Beav media library when `BEAV_MEDIA_ROOT` is set.
- Existing local outputs can be indexed with `register_asset`.

## Production Flow

1. Build a compact CTS from the user brief, reference assets, platform, audience, and target duration.
2. Use multimodal reference analysis inside this video agent session when reference images or videos exist.
   By default use `OPENMONTAGE_VISION_ANALYZER_PROVIDER=openai` with
   `OPENMONTAGE_VISION_ANALYZER_MODEL=gpt-5.5`, reusing the same OpenAI-compatible runtime as Goose.
   Other VLM providers are optional fallbacks for cost or provider-specific workflows.
3. Split a 60-90 second target into multiple short shots. Seedance clips should normally be 5-8 seconds each.
4. Generate clips with stable product, environment, and style anchors.
5. Register every generated image/video into the Beav media library.
6. Use OpenMontage editing/compose tools, or Beav video project commands when available, to assemble final MP4.
7. Return asset ids, media relative paths, and the final project/session id.

## Boundary

Keep this context out of the main operations chat. The main agent should only see the delegation surface and
high-level result summaries.
