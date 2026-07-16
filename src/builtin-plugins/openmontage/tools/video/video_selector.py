"""Single-provider selector for JiubanAI-managed video generation."""

from __future__ import annotations

from typing import Any

from tools.base_tool import BaseTool, ToolResult, ToolRuntime, ToolStability, ToolStatus, ToolTier


class VideoSelector(BaseTool):
    name = "video_selector"
    version = "1.0.0"
    tier = ToolTier.GENERATE
    capability = "video_generation"
    provider = "selector"
    stability = ToolStability.PRODUCTION
    runtime = ToolRuntime.HYBRID
    agent_skills = ["ai-video-gen", "create-video", "ark-plan-video-director"]
    capabilities = ["text_to_video", "image_to_video", "reference_to_video", "provider_selection"]
    supports = {
        "user_preference_routing": False,
        "reference_image": True,
        "multiple_reference_images": True,
    }
    best_for = ["把视频生成请求统一路由到商媒运营助手桌面端视频服务"]

    input_schema = {
        "type": "object",
        "required": ["prompt"],
        "properties": {
            "prompt": {"type": "string", "description": "视频生成提示词；中文场景默认使用简体中文。"},
            "preferred_provider": {
                "type": "string",
                "enum": ["auto", "jiuban-video", "seedance"],
                "default": "auto",
            },
            "operation": {
                "type": "string",
                "enum": ["text_to_video", "image_to_video", "reference_to_video", "rank"],
                "default": "text_to_video",
            },
            "target_operation": {
                "type": "string",
                "enum": ["text_to_video", "image_to_video", "reference_to_video"],
                "default": "text_to_video",
            },
            "aspect_ratio": {"type": "string", "enum": ["16:9", "9:16"], "default": "16:9"},
            "duration": {
                "type": "integer",
                "minimum": 5,
                "maximum": 12,
                "default": 8,
                "description": "单个片段时长为 5-12 秒；更长视频需要生成多个片段并使用 video_stitch 合成。",
            },
            "resolution": {"type": "string", "enum": ["720p", "1080p"], "default": "720p"},
            "reference_image_path": {"type": "string"},
            "reference_image_url": {"type": "string"},
            "reference_image_paths": {"type": "array", "items": {"type": "string"}},
            "reference_image_urls": {"type": "array", "items": {"type": "string"}},
            "output_path": {"type": "string"},
            "generate_audio": {"type": "boolean", "default": True},
        },
    }

    @staticmethod
    def _provider() -> BaseTool | None:
        from tools.tool_registry import registry

        registry.ensure_discovered()
        return registry.get("seedance_video")

    @property
    def fallback_tools(self) -> list[str]:
        return ["seedance_video"]

    @property
    def provider_matrix(self) -> dict[str, dict[str, str]]:
        return {
            "jiuban-video": {
                "tool": "seedance_video",
                "strength": "商媒运营助手托管的火山引擎视频生成与素材库存储",
            }
        }

    def get_status(self) -> ToolStatus:
        provider = self._provider()
        return provider.get_status() if provider else ToolStatus.UNAVAILABLE

    def estimate_cost(self, inputs: dict[str, Any]) -> float:
        provider = self._provider()
        return provider.estimate_cost(inputs) if provider else 0.0

    def estimate_runtime(self, inputs: dict[str, Any]) -> float:
        provider = self._provider()
        return provider.estimate_runtime(inputs) if provider else 0.0

    @staticmethod
    def _adapt(inputs: dict[str, Any]) -> dict[str, Any]:
        adapted = dict(inputs)
        adapted.pop("preferred_provider", None)
        adapted.pop("target_operation", None)
        reference_path = str(adapted.pop("reference_image_path", "") or "").strip()
        reference_url = str(adapted.pop("reference_image_url", "") or "").strip()
        if reference_path and not adapted.get("image_path"):
            adapted["image_path"] = reference_path
        if reference_url and not adapted.get("image_url"):
            adapted["image_url"] = reference_url
        return adapted

    def dry_run(self, inputs: dict[str, Any]) -> dict[str, Any]:
        provider = self._provider()
        if provider is None:
            return {"status": "unavailable", "error": "seedance_video 尚未注册"}
        operation = inputs.get("target_operation", "text_to_video") if inputs.get("operation") == "rank" else inputs.get("operation", "text_to_video")
        adapted = self._adapt({**inputs, "operation": operation})
        preview = provider.dry_run(adapted)
        preview["selected_tool"] = provider.name
        preview["selected_provider"] = provider.provider
        return preview

    def execute(self, inputs: dict[str, Any]) -> ToolResult:
        provider = self._provider()
        if provider is None:
            return ToolResult(success=False, error="商媒运营助手视频供应商当前不可用")
        if inputs.get("operation") == "rank":
            return ToolResult(
                success=True,
                data={
                    "rankings": [{
                        "provider": provider.provider,
                        "tool_name": provider.name,
                        "status": provider.get_status().value,
                        "reason": "商媒运营助手是当前唯一启用的视频生成供应商",
                    }],
                    "selected_tool": provider.name,
                    "selected_provider": provider.provider,
                },
            )
        result = provider.execute(self._adapt(inputs))
        if result.success:
            result.data.setdefault("selected_tool", provider.name)
            result.data.setdefault("selected_provider", provider.provider)
            result.data.setdefault("selection_reason", "商媒运营助手是当前配置的视频生成服务")
            result.data.setdefault("alternatives_considered", [])
        return result
