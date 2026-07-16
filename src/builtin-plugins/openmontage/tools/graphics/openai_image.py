"""OpenAI GPT Image generation (gpt-image-2)."""

from __future__ import annotations

import base64
import json
import os
import time
from pathlib import Path
from typing import Any
from urllib.error import HTTPError
from urllib.parse import urlparse, urlunparse
from urllib.request import Request, urlopen

from tools.base_tool import (
    BaseTool,
    Determinism,
    ExecutionMode,
    ResourceProfile,
    RetryPolicy,
    ToolResult,
    ToolRuntime,
    ToolStability,
    ToolStatus,
    ToolTier,
)


def _env_text(*names: str) -> str:
    for name in names:
        value = str(os.environ.get(name) or "").strip()
        if value:
            return value
    return ""


def _normalize_openai_base_url(raw: str) -> str:
    """Return the SDK base URL, not a concrete images/responses route."""
    value = str(raw or "").strip().rstrip("/")
    if not value:
        return ""
    for suffix in ("/images/generations", "/responses", "/chat/completions"):
        if value.endswith(suffix):
            value = value[: -len(suffix)].rstrip("/")
    parsed = urlparse(value)
    if not parsed.scheme or not parsed.netloc:
        return value
    path = (parsed.path or "").rstrip("/")
    if not path or path == "/":
        path = "/v1"
    elif path.endswith("/v1") or path.endswith("/openai") or "/api/plan/v3" in path:
        pass
    else:
        path = f"{path}/v1"
    return urlunparse((parsed.scheme, parsed.netloc, path, "", "", ""))


class OpenAIImage(BaseTool):
    name = "openai_image"
    version = "0.1.0"
    tier = ToolTier.GENERATE
    capability = "image_generation"
    provider = "openai"
    stability = ToolStability.BETA
    execution_mode = ExecutionMode.SYNC
    determinism = Determinism.STOCHASTIC
    runtime = ToolRuntime.API

    dependencies = []  # checked dynamically
    install_instructions = (
        "Set OPENAI_API_KEY to your OpenAI API key.\n"
        "  pip install openai"
    )
    agent_skills = ["flux-best-practices"]  # general image gen knowledge

    capabilities = ["generate_image", "generate_illustration", "text_to_image"]
    supports = {
        "complex_instructions": True,
        "text_in_image": True,
        "multiple_outputs": True,
    }
    best_for = [
        "complex multi-element compositions",
        "images with text/labels",
        "following detailed instructions accurately",
    ]
    not_good_for = ["offline generation", "budget-constrained projects at high quality"]

    input_schema = {
        "type": "object",
        "required": ["prompt"],
        "properties": {
            "prompt": {"type": "string"},
            "model": {
                "type": "string",
                "enum": ["gpt-image-2"],
                "default": "gpt-image-2",
            },
            "size": {
                "type": "string",
                "enum": ["1024x1024", "1536x1024", "1024x1536", "auto"],
                "default": "1024x1024",
            },
            "quality": {
                "type": "string",
                "enum": ["low", "medium", "high", "auto"],
                "default": "high",
            },
            "output_format": {
                "type": "string",
                "enum": ["png", "jpeg", "webp"],
                "default": "png",
            },
            "n": {"type": "integer", "default": 1, "minimum": 1, "maximum": 4},
            "output_path": {"type": "string"},
            "project_id": {
                "type": "string",
                "description": "素材归档项目 ID；同一视频任务的首图、片段和成片必须复用同一个值。",
            },
            "title": {"type": "string", "description": "素材库中的中文标题。"},
            "delivery_role": {
                "type": "string",
                "description": "素材角色，例如 cover、keyframe 或 final_image。",
            },
        },
    }

    resource_profile = ResourceProfile(
        cpu_cores=1, ram_mb=512, vram_mb=0, disk_mb=100, network_required=True
    )
    retry_policy = RetryPolicy(max_retries=2, retryable_errors=["rate_limit", "timeout"])
    idempotency_key_fields = ["prompt", "size", "quality", "model"]
    side_effects = ["writes image file to output_path", "calls OpenAI API"]
    user_visible_verification = ["Inspect generated image for relevance and quality"]

    def get_status(self) -> ToolStatus:
        if _env_text("BEAV_IMAGE_API_KEY", "OPENAI_IMAGE_API_KEY", "OPENAI_API_KEY"):
            return ToolStatus.AVAILABLE
        return ToolStatus.UNAVAILABLE

    def estimate_cost(self, inputs: dict[str, Any]) -> float:
        # gpt-image-2 per-image pricing at 1024x1024 (non-square sizes run
        # slightly cheaper): https://developers.openai.com/api/docs/guides/image-generation
        quality = inputs.get("quality", "high")
        n = inputs.get("n", 1)
        cost_map = {"low": 0.006, "medium": 0.053, "high": 0.211, "auto": 0.053}
        return cost_map.get(quality, 0.053) * n

    def execute(self, inputs: dict[str, Any]) -> ToolResult:
        api_key = _env_text("BEAV_IMAGE_API_KEY", "OPENAI_IMAGE_API_KEY", "OPENAI_API_KEY")
        if not api_key:
            return ToolResult(
                success=False,
                error=(
                    "OPENAI_API_KEY, OPENAI_IMAGE_API_KEY, or BEAV_IMAGE_API_KEY not set. "
                    + self.install_instructions
                ),
            )

        start = time.time()
        base_url = _env_text("OPENAI_IMAGE_ENDPOINT", "BEAV_IMAGE_ENDPOINT", "OPENAI_BASE_URL")
        normalized_base_url = _normalize_openai_base_url(base_url) if base_url else ""
        model = inputs.get("model") or _env_text("BEAV_IMAGE_MODEL", "OPENAI_IMAGE_MODEL") or "gpt-image-2"
        prompt = inputs["prompt"]
        size = inputs.get("size") or _env_text("BEAV_IMAGE_SIZE") or "1024x1024"
        n = inputs.get("n", 1)
        quality = inputs.get("quality") or _env_text("BEAV_IMAGE_QUALITY") or "high"
        output_format = inputs.get("output_format", "png")

        try:
            image_data = self._generate_image_bytes(
                api_key=api_key,
                base_url=normalized_base_url,
                model=model,
                prompt=prompt,
                size=size,
                quality=quality,
                output_format=output_format,
                n=n,
            )
            ext = output_format
            output_path = Path(inputs.get("output_path", f"generated_image.{ext}"))
            output_path.parent.mkdir(parents=True, exist_ok=True)
            output_path.write_bytes(image_data)

        except Exception as e:
            return ToolResult(success=False, error=f"OpenAI image generation failed: {e}")

        return ToolResult(
            success=True,
            data={
                "provider": "openai",
                "model": model,
                "prompt": prompt,
                "output": str(output_path),
            },
            artifacts=[str(output_path)],
            cost_usd=self.estimate_cost(inputs),
            duration_seconds=round(time.time() - start, 2),
            model=model,
        )

    @staticmethod
    def _generate_image_bytes(
        *,
        api_key: str,
        base_url: str,
        model: str,
        prompt: str,
        size: str,
        quality: str,
        output_format: str,
        n: int,
    ) -> bytes:
        try:
            from openai import OpenAI  # type: ignore

            client_kwargs: dict[str, Any] = {"api_key": api_key}
            if base_url:
                client_kwargs["base_url"] = base_url
            response = OpenAI(**client_kwargs).images.generate(
                model=model,
                prompt=prompt,
                size=size,
                quality=quality,
                output_format=output_format,
                n=n,
            )
            item = response.data[0]
            b64_json = getattr(item, "b64_json", None)
            url = getattr(item, "url", None)
        except ModuleNotFoundError:
            b64_json, url = OpenAIImage._generate_image_http(
                api_key=api_key,
                base_url=base_url,
                model=model,
                prompt=prompt,
                size=size,
                quality=quality,
                output_format=output_format,
                n=n,
            )

        if b64_json:
            return base64.b64decode(b64_json)
        if url:
            return OpenAIImage._http_bytes(url)
        raise RuntimeError("image generation response contained neither b64_json nor url")

    @staticmethod
    def _generate_image_http(
        *,
        api_key: str,
        base_url: str,
        model: str,
        prompt: str,
        size: str,
        quality: str,
        output_format: str,
        n: int,
    ) -> tuple[str, str]:
        endpoint = f"{(base_url or 'https://api.openai.com/v1').rstrip('/')}/images/generations"
        payload = {
            "model": model,
            "prompt": prompt,
            "size": size,
            "quality": quality,
            "output_format": output_format,
            "n": n,
        }
        req = Request(
            endpoint,
            data=json.dumps(payload).encode("utf-8"),
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
            },
            method="POST",
        )
        try:
            with urlopen(req, timeout=180) as response:
                raw = response.read().decode("utf-8", errors="replace")
        except HTTPError as exc:
            raw = exc.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"HTTP {exc.code} from {endpoint}: {raw[:1000]}") from exc
        data = json.loads(raw or "{}")
        images = data.get("data") if isinstance(data, dict) else None
        if not images:
            raise RuntimeError(f"image generation response contained no data: {data}")
        first = images[0] if isinstance(images, list) else images
        if not isinstance(first, dict):
            raise RuntimeError(f"unexpected image generation item: {first}")
        return str(first.get("b64_json") or ""), str(first.get("url") or "")

    @staticmethod
    def _http_bytes(url: str) -> bytes:
        req = Request(url, method="GET")
        try:
            with urlopen(req, timeout=180) as response:
                return response.read()
        except HTTPError as exc:
            raw = exc.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"image download failed with HTTP {exc.code}: {raw[:1000]}") from exc
