"""JiubanAI-managed Seedance video generation.

OpenMontage owns planning and editing, while the desktop app owns provider
configuration, Volcengine requests, polling, downloads, and media-library writes.
"""

from __future__ import annotations

import os
import shutil
import time
from pathlib import Path
from typing import Any

import requests

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


def _env_int(name: str, default: int) -> int:
    try:
        return int(float(str(os.environ.get(name) or default)))
    except (TypeError, ValueError):
        return default


def _env_bool(name: str, default: bool) -> bool:
    value = str(os.environ.get(name) or "").strip().lower()
    if not value:
        return default
    return value in {"1", "true", "yes", "on"}


DEFAULT_DURATION_SECONDS = max(5, min(12, _env_int("OPENMONTAGE_DEFAULT_DURATION_SECONDS", 8)))
DEFAULT_ASPECT_RATIO = str(os.environ.get("OPENMONTAGE_DEFAULT_ASPECT_RATIO") or "16:9")
if DEFAULT_ASPECT_RATIO not in {"16:9", "9:16"}:
    DEFAULT_ASPECT_RATIO = "16:9"
DEFAULT_RESOLUTION = str(os.environ.get("OPENMONTAGE_DEFAULT_RESOLUTION") or "720p")
if DEFAULT_RESOLUTION not in {"720p", "1080p"}:
    DEFAULT_RESOLUTION = "720p"
DEFAULT_GENERATE_AUDIO = _env_bool("OPENMONTAGE_DEFAULT_GENERATE_AUDIO", True)


class SeedanceVideo(BaseTool):
    name = "seedance_video"
    version = "1.0.0"
    tier = ToolTier.GENERATE
    capability = "video_generation"
    provider = "jiuban-video"
    stability = ToolStability.PRODUCTION
    execution_mode = ExecutionMode.SYNC
    determinism = Determinism.STOCHASTIC
    runtime = ToolRuntime.API

    dependencies = ["requests"]
    install_instructions = (
        "请启动商媒运营助手桌面端。火山引擎端点、API Key、模型选择、任务轮询和素材库存储均由主应用统一管理。"
    )
    agent_skills = ["seedance-2-0", "ai-video-gen", "ark-plan-video-director"]
    capabilities = ["text_to_video", "image_to_video", "reference_to_video"]
    supports = {
        "text_to_video": True,
        "image_to_video": True,
        "reference_to_video": True,
        "multiple_reference_images": True,
        "reference_image": True,
        "native_audio": True,
        "camera_direction": True,
        "multi_shot": True,
        "aspect_ratio": True,
    }
    best_for = [
        "商媒运营助手内的所有 AI 视频生成任务",
        "火山引擎 Seedance 文生视频和参考图生视频",
        "生成后自动写入商媒运营助手素材库",
    ]
    not_good_for = [
        "商媒运营助手桌面端未运行的环境",
        "单段超过 12 秒的视频；应生成多个片段后使用 video_stitch 合成",
    ]
    fallback_tools: list[str] = []
    quality_score = 1.0

    input_schema = {
        "type": "object",
        "required": ["prompt"],
        "properties": {
            "prompt": {
                "type": "string",
                "description": "视频生成提示词。默认使用简体中文，按主体、动作、场景、构图和镜头顺序清楚描述；用户明确指定其他语言时除外。",
            },
            "operation": {
                "type": "string",
                "enum": ["text_to_video", "image_to_video", "reference_to_video"],
                "default": "text_to_video",
            },
            "duration": {
                "type": "integer",
                "minimum": 5,
                "maximum": 12,
                "default": DEFAULT_DURATION_SECONDS,
                "description": "单个生成片段的时长，范围 5-12 秒。目标视频超过 12 秒时必须生成多个片段，再用 video_stitch 合成。",
            },
            "aspect_ratio": {
                "type": "string",
                "enum": ["16:9", "9:16"],
                "default": DEFAULT_ASPECT_RATIO,
            },
            "resolution": {
                "type": "string",
                "enum": ["720p", "1080p"],
                "default": DEFAULT_RESOLUTION,
            },
            "generate_audio": {
                "type": "boolean",
                "default": DEFAULT_GENERATE_AUDIO,
                "description": "是否生成原生音频；中文生活场景默认开启。",
            },
            "image_url": {"type": "string", "description": "图生视频参考图 URL。"},
            "image_path": {
                "type": "string",
                "description": "图生视频本地参考图绝对路径。多段连续视频只在第一段使用原始首图；第二段起应改用 first_clip 续写上一段。",
            },
            "end_image_url": {"type": "string"},
            "end_image_path": {"type": "string"},
            "reference_image_urls": {"type": "array", "items": {"type": "string"}},
            "reference_image_paths": {"type": "array", "items": {"type": "string"}},
            "driving_audio": {"type": "string"},
            "first_clip": {
                "type": "string",
                "description": "续写模式的上一段视频路径或 URL。多段成片从第二段开始必须传入上一段素材库路径，不要再次传原始首图。",
            },
            "output_path": {"type": "string", "description": "可选的本地输出路径；产物始终会同时写入商媒运营助手素材库。"},
            "title": {"type": "string", "description": "素材库标题，中文场景默认使用简体中文。"},
            "project_id": {"type": "string"},
        },
    }

    resource_profile = ResourceProfile(
        cpu_cores=1, ram_mb=256, vram_mb=0, disk_mb=500, network_required=True
    )
    retry_policy = RetryPolicy(max_retries=1, retryable_errors=["timeout", "bridge_unavailable"])
    idempotency_key_fields = ["prompt", "operation", "duration", "aspect_ratio"]
    side_effects = [
        "调用商媒运营助手本地 App Bridge",
        "在商媒运营助手素材库中创建视频资产",
        "可选地把首个生成资产复制到 output_path",
    ]

    @staticmethod
    def _bridge_endpoint() -> str:
        base = str(os.environ.get("BEAV_BRIDGE_URL") or "").strip().rstrip("/")
        if not base:
            raise RuntimeError("缺少 BEAV_BRIDGE_URL，拒绝调用本地主程序桥")
        route = str(os.environ.get("BEAV_BRIDGE_PATH") or "/mcp/beav")
        return f"{base}/{route.lstrip('/')}"

    @staticmethod
    def _bridge_headers() -> dict[str, str]:
        token = str(os.environ.get("BEAV_BRIDGE_TOKEN") or "").strip()
        if not token:
            raise RuntimeError("缺少 BEAV_BRIDGE_TOKEN，拒绝调用本地主程序桥")
        return {"Authorization": f"Bearer {token}"}

    @staticmethod
    def _duration_seconds(value: Any) -> int:
        try:
            parsed = int(float(str(value if value not in (None, "") else DEFAULT_DURATION_SECONDS)))
        except (TypeError, ValueError):
            parsed = DEFAULT_DURATION_SECONDS
        return max(5, min(12, parsed))

    @staticmethod
    def _requested_duration_seconds(value: Any) -> int:
        try:
            parsed = int(float(str(value if value not in (None, "") else DEFAULT_DURATION_SECONDS)))
        except (TypeError, ValueError):
            parsed = DEFAULT_DURATION_SECONDS
        return max(5, parsed)

    @staticmethod
    def _reference_images(inputs: dict[str, Any]) -> list[str]:
        refs: list[str] = []
        for key in ("image_path", "image_url"):
            value = str(inputs.get(key) or "").strip()
            if value:
                refs.append(value)
        for key in ("reference_image_paths", "reference_image_urls"):
            values = inputs.get(key)
            if isinstance(values, list):
                refs.extend(str(value).strip() for value in values if str(value).strip())
        for key in ("end_image_path", "end_image_url"):
            value = str(inputs.get(key) or "").strip()
            if value:
                refs.append(value)
        return list(dict.fromkeys(refs))[:5]

    @classmethod
    def _generation_mode(cls, inputs: dict[str, Any], refs: list[str]) -> str:
        if str(inputs.get("first_clip") or "").strip():
            return "continuation"
        if str(inputs.get("end_image_path") or inputs.get("end_image_url") or "").strip() and len(refs) >= 2:
            return "first-last-frame"
        operation = str(inputs.get("operation") or "text_to_video")
        if operation in {"image_to_video", "reference_to_video"} or refs:
            return "reference-guided"
        return "text-to-video"

    @staticmethod
    def _continuation_prompt(prompt: str) -> str:
        language = str(os.environ.get("OPENMONTAGE_LANGUAGE") or "zh-CN").lower()
        if language.startswith("en"):
            instruction = (
                "Continue directly from the final frame of the supplied video. "
                "Preserve character, environment, lighting, camera direction, and action state. "
                "Do not restart the scene or repeat actions already completed."
            )
        else:
            instruction = (
                "紧接输入视频的最后一帧继续，保持人物、环境、光线、机位和动作状态连续；"
                "不要回到开场画面，也不要重复上一段已经完成的动作。"
            )
        return f"{instruction}\n{prompt}" if prompt else instruction

    def get_status(self) -> ToolStatus:
        disabled = str(os.environ.get("OPENMONTAGE_DISABLE_APP_VIDEO") or "").lower()
        return ToolStatus.UNAVAILABLE if disabled in {"1", "true", "yes"} else ToolStatus.AVAILABLE

    def is_operation_available(self, operation: str) -> bool:
        return operation in {"text_to_video", "image_to_video", "reference_to_video"}

    def estimate_cost(self, inputs: dict[str, Any]) -> float:
        return 0.0

    def estimate_runtime(self, inputs: dict[str, Any]) -> float:
        return 120.0

    def dry_run(self, inputs: dict[str, Any]) -> dict[str, Any]:
        refs = self._reference_images(inputs)
        first_clip = str(inputs.get("first_clip") or "").strip()
        if first_clip:
            refs = []
        requested_duration = self._requested_duration_seconds(inputs.get("duration"))
        clip_duration = self._duration_seconds(inputs.get("duration"))
        return {
            "tool": self.name,
            "provider": self.provider,
            "route": "JiubanAI app_cli video generate",
            "bridge": self._bridge_endpoint(),
            "generation_mode": self._generation_mode(inputs, refs),
            "continuity_source": first_clip or None,
            "reference_image_count": len(refs),
            "requested_duration_seconds": requested_duration,
            "clip_duration_seconds": clip_duration,
            "requires_stitch_for_requested_duration": requested_duration > clip_duration,
            "status": self.get_status().value,
            "would_execute": True,
        }

    def execute(self, inputs: dict[str, Any]) -> ToolResult:
        started = time.time()
        refs = self._reference_images(inputs)
        first_clip = str(inputs.get("first_clip") or "").strip()
        reference_images_suppressed = bool(first_clip and refs)
        if first_clip:
            refs = []
        generation_mode = self._generation_mode(inputs, refs)
        requested_duration = self._requested_duration_seconds(inputs.get("duration"))
        clip_duration = self._duration_seconds(inputs.get("duration"))
        prompt = str(inputs.get("prompt") or "").strip()
        if first_clip:
            prompt = self._continuation_prompt(prompt)
        app_payload = {
            "prompt": prompt,
            "title": str(inputs.get("title") or "").strip() or None,
            "projectId": str(inputs.get("project_id") or "").strip() or None,
            "generationMode": generation_mode,
            "referenceImages": refs,
            "aspectRatio": str(inputs.get("aspect_ratio") or DEFAULT_ASPECT_RATIO),
            "resolution": str(inputs.get("resolution") or DEFAULT_RESOLUTION),
            "durationSeconds": clip_duration,
            "generateAudio": (
                bool(inputs["generate_audio"])
                if "generate_audio" in inputs
                else DEFAULT_GENERATE_AUDIO
            ),
            "drivingAudio": str(inputs.get("driving_audio") or "").strip() or None,
            "firstClip": first_clip or None,
        }
        app_payload = {key: value for key, value in app_payload.items() if value not in (None, "", [])}
        timeout_seconds = max(
            60.0,
            float(os.environ.get("BEAV_BRIDGE_TIMEOUT_MS") or 1_800_000) / 1000.0,
        )

        try:
            response = requests.post(
                self._bridge_endpoint(),
                headers=self._bridge_headers(),
                json={
                    "action": "app_cli",
                    "payload": {"command": "video generate", "payload": app_payload},
                    "source": "openmontage-plugin",
                },
                timeout=timeout_seconds,
            )
            response.raise_for_status()
            envelope = response.json()
            if envelope.get("error"):
                raise RuntimeError(str(envelope["error"].get("message") or envelope["error"]))
            result = envelope.get("result") or {}
            if result.get("success") is False:
                raise RuntimeError(str(result.get("error") or result.get("llmContent") or "视频生成失败"))
            data = result.get("data") if isinstance(result.get("data"), dict) else {}
            assets = data.get("assets") if isinstance(data.get("assets"), list) else []
            managed_artifact_paths = [
                str(asset.get("absolutePath") or "").strip()
                for asset in assets
                if isinstance(asset, dict) and str(asset.get("absolutePath") or "").strip()
            ]
            if not managed_artifact_paths:
                raise RuntimeError("商媒运营助手已完成请求，但没有返回本地视频资产路径")

            requested_output = str(inputs.get("output_path") or "").strip()
            if requested_output:
                output_path = Path(requested_output).expanduser()
                output_path.parent.mkdir(parents=True, exist_ok=True)
                source_path = Path(managed_artifact_paths[0])
                if source_path.resolve() != output_path.resolve():
                    shutil.copy2(source_path, output_path)

            return ToolResult(
                success=True,
                data={
                    "provider": data.get("provider") or self.provider,
                    "model": data.get("model") or os.environ.get("BEAV_VIDEO_MODEL") or "app-managed",
                    "generation_mode": data.get("generationMode") or generation_mode,
                    "continuity_source": first_clip or None,
                    "reference_images_suppressed_for_continuation": reference_images_suppressed,
                    "requested_duration_seconds": requested_duration,
                    "clip_duration_seconds": clip_duration,
                    "requires_stitch_for_requested_duration": requested_duration > clip_duration,
                    "prompt": app_payload["prompt"],
                    "output": managed_artifact_paths[0],
                    "output_path": managed_artifact_paths[0],
                    "assets": assets,
                    "managed_by": "商媒运营助手素材库",
                    "delivery_role": "intermediate_clip",
                    "chat_visibility": "library_only",
                },
                artifacts=list(dict.fromkeys(managed_artifact_paths)),
                cost_usd=0.0,
                duration_seconds=round(time.time() - started, 2),
                model=str(data.get("model") or "app-managed"),
            )
        except Exception as exc:
            return ToolResult(
                success=False,
                error=f"商媒运营助手视频生成桥接失败：{exc}",
                duration_seconds=round(time.time() - started, 2),
            )
