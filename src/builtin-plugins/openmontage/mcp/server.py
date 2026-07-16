#!/usr/bin/env python3
"""Python-native MCP server for the JiubanAI OpenMontage plugin.

The server preserves the original low-level OpenMontage tool bridge while
adding a versioned AI short-drama workspace and one MCP App resource. It uses
only stdio JSON-RPC framing so the app-bundled ``uv`` can launch it directly.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import sys
import traceback
from typing import Any


PLUGIN_ROOT = Path(__file__).resolve().parent.parent
if str(PLUGIN_ROOT) not in sys.path:
    sys.path.insert(0, str(PLUGIN_ROOT))

from lib.short_drama import ShortDramaError, ShortDramaService  # noqa: E402
from mcp.src import openmontage_runner as runner  # noqa: E402


JSONRPC_VERSION = "2.0"
MCP_PROTOCOL_VERSION = "2024-11-05"
SERVER_VERSION = "0.3.2"
APP_RESOURCE_URI = "ui://openmontage/short-drama"
APP_MIME_TYPE = "text/html;profile=mcp-app"
APP_HTML_PATH = PLUGIN_ROOT / "mcp" / "apps" / "short_drama.html"

DEFAULT_LANGUAGE = str(os.environ.get("OPENMONTAGE_LANGUAGE") or "zh-CN").strip()

if DEFAULT_LANGUAGE.lower().startswith("en"):
    SERVER_INSTRUCTIONS = " ".join([
        "OpenMontage is JiubanAI's built-in AI video and short-drama production plugin.",
        "Use English for titles, generation prompts, project fields, progress updates, and delivery notes unless the user requests another language.",
        "Before the first generation call in a multi-asset task, choose one stable project_id and pass it to every image, clip, and video_stitch call. JiubanAI stores them under generated/<project-id>/images, clips, output, and audio.",
        "For videos longer than 12 seconds, generate multiple 5-12 second clips and combine them with video_stitch. Keep intermediate clips in the media library and surface only requested final deliverables in chat.",
        "Generated video must use JiubanAI app_cli video generate, backed by the app-managed Volcengine service.",
        "Real media generation through run_tool requires dryRun=false and confirm=true.",
    ])
else:
    SERVER_INSTRUCTIONS = " ".join([
        "OpenMontage 是商媒运营助手内置的 AI 视频与短剧生产插件。",
        "默认面向中文用户：除非用户明确指定其他语言，所有标题、生成提示词、项目字段、进度说明和最终交付说明都必须使用简体中文，不要为了调用模型而擅自翻译成英文。",
        "多素材任务在第一次生图前就必须确定一个稳定的 project_id，并在首图、每个视频片段和 video_stitch 调用中复用。商媒运营助手会按 generated/<project-id>/images、clips、output、audio 归档，不得让同一任务的素材散落在 generated 根目录。",
        "普通视频超过 12 秒时，应生成多个 5-12 秒片段并使用 video_stitch 合成为成片。中间片段只写入素材库，不在聊天中逐个展示；最终回复只声明首图和最终成片等用户要求的交付物。",
        "多段连续视频的第一段可使用原始首图；从第二段开始必须使用 seedance_video 的 first_clip 传入上一段素材库视频并采用 continuation 续写，禁止每段重复使用同一张原始首图。每段提示词只描述尚未发生的后续动作。",
        "调用 video_stitch 前必须保留 continuity_check=strict；若工具报告重复开场，应重新生成对应后续片段，不要把有明显重播的结果交付给用户。",
        "最终回复必须使用 run_tool 返回的商媒运营助手素材库持久化路径，禁止向用户展示 /tmp、/private/tmp 或其他工作目录路径。",
        "短剧任务先读取 pipeline_defs/ai-short-drama.yaml 和对应阶段的 director Skill，再开始创作。",
        "宿主 AI 负责创意内容；Python 工具负责版本化保存、结构校验、媒体执行和人工确认门禁。",
        "短剧流程使用 drama_project_create、drama_stage_context、drama_artifact_save 和 drama_open_review。",
        "角色、分镜、资产、配音和审批选择以交互式 MCP App 中的状态为准。",
        "视频生成统一通过商媒运营助手 app_cli video generate 调用应用托管的火山引擎服务。",
        "run_tool 真实执行仍要求 dryRun=false 且 confirm=true。",
    ])


def is_plain_object(value: Any) -> bool:
    return isinstance(value, dict)


def object_schema(
    properties: dict[str, Any],
    *,
    required: list[str] | None = None,
    additional_properties: bool = False,
) -> dict[str, Any]:
    schema: dict[str, Any] = {
        "type": "object",
        "properties": properties,
        "additionalProperties": additional_properties,
    }
    if required:
        schema["required"] = required
    return schema


def ui_meta(visibility: list[str] | None = None) -> dict[str, Any]:
    return {
        "ui": {
            "resourceUri": APP_RESOURCE_URI,
            "visibility": visibility or ["model", "app"],
        }
    }


def tool_definitions() -> list[dict[str, Any]]:
    revision = {"type": "integer", "minimum": 1}
    project_id = {"type": "string", "minLength": 3, "maxLength": 64}
    timeout = {"type": "integer", "minimum": 10000, "maximum": 1800000}
    definitions: list[dict[str, Any]] = [
        {
            "name": "env_check",
            "description": "检查 OpenMontage 目录、Python 运行时、供应商映射，并可选执行工具发现。",
            "inputSchema": object_schema({
                "discover": {"type": "boolean", "default": False},
                "timeoutMs": timeout,
            }),
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "list_pipelines",
            "description": "列出 OpenMontage 的流程定义 YAML 文件。",
            "inputSchema": object_schema({}),
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "get_pipeline",
            "description": "读取一个 OpenMontage 流程定义 YAML，用于任务规划。",
            "inputSchema": object_schema({"name": {"type": "string"}}, required=["name"]),
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "list_tools",
            "description": "发现 OpenMontage 工具并返回精简契约，可按能力或供应商筛选。普通视频可使用 capability=video。",
            "inputSchema": object_schema({
                "capability": {"type": "string"},
                "provider": {"type": "string"},
                "includeSchemas": {"type": "boolean", "default": False},
                "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 200},
                "timeoutMs": timeout,
            }),
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "tool_info",
            "description": "返回一个媒体工具的完整 OpenMontage 契约。",
            "inputSchema": object_schema({"name": {"type": "string"}, "timeoutMs": timeout}, required=["name"]),
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "provider_menu",
            "description": "返回 OpenMontage 供应商可用性，用于执行前检查和规划。",
            "inputSchema": object_schema({
                "summary": {"type": "boolean", "default": True},
                "timeoutMs": timeout,
            }),
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "run_tool",
            "description": "预演或执行一个 OpenMontage 媒体工具。真实执行要求 dryRun=false 且 confirm=true。中文用户场景下，inputs 中的 title 和 prompt 默认使用简体中文。",
            "inputSchema": object_schema({
                "name": {"type": "string"},
                "inputs": {"type": "object", "additionalProperties": True},
                "dryRun": {"type": "boolean", "default": True},
                "confirm": {"type": "boolean", "default": False},
                "timeoutMs": timeout,
            }, required=["name"]),
            "annotations": {"destructiveHint": True},
        },
        {
            "name": "register_asset",
            "description": "把已有的本地 OpenMontage 产物登记到商媒运营助手素材库。",
            "inputSchema": object_schema({
                "path": {"type": "string"},
                "tool": {"type": "string"},
                "model": {"type": "string"},
                "inputs": {"type": "object", "additionalProperties": True},
                "data": {"type": "object", "additionalProperties": True},
                "timeoutMs": timeout,
            }, required=["path"]),
        },
        {
            "name": "drama_project_create",
            "description": "创建带版本控制的 AI 短剧项目；提供 sourceText 时会保存为 source_document 并打开工作台。",
            "inputSchema": object_schema({
                "projectId": project_id,
                "title": {"type": "string", "minLength": 1},
                "sourceTitle": {"type": "string"},
                "sourceType": {"type": "string", "enum": ["text", "markdown", "transcript", "url", "upload", "outline"]},
                "sourceText": {"type": "string"},
                "episodeCount": {"type": "integer", "minimum": 1, "maximum": 200, "default": 1},
                "episodeDurationSeconds": {"type": "integer", "minimum": 15, "maximum": 3600, "default": 90},
                "aspectRatio": {"type": "string", "enum": ["9:16", "16:9", "1:1", "4:3", "3:4"], "default": "9:16"},
                "genre": {"type": "string"},
                "targetAudience": {"type": "string"},
                "language": {"type": "string", "default": "zh-CN"},
                "adaptationMode": {"type": "string", "enum": ["faithful", "balanced", "inspired"], "default": "faithful"},
                "visualStyle": {"type": "string"},
            }, required=["title"]),
            "_meta": ui_meta(),
        },
        {
            "name": "drama_project_list",
            "description": "列出本地 AI 短剧项目，不加载完整制品内容。",
            "inputSchema": object_schema({}),
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "drama_project_get",
            "description": "加载短剧项目的权威状态及当前制品。",
            "inputSchema": object_schema({"projectId": project_id}, required=["projectId"]),
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "drama_source_set",
            "description": "使用乐观版本控制创建或替换短剧项目的 source_document。",
            "inputSchema": object_schema({
                "projectId": project_id,
                "expectedRevision": revision,
                "title": {"type": "string"},
                "sourceType": {"type": "string", "enum": ["text", "markdown", "transcript", "url", "upload", "outline"]},
                "language": {"type": "string"},
                "content": {"type": "string", "minLength": 1},
                "metadata": {"type": "object", "additionalProperties": True},
            }, required=["projectId", "content"]),
        },
        {
            "name": "drama_stage_context",
            "description": "加载一个短剧阶段的上下文、必需输入、输出契约和阶段 Skill 路径；生成制品前必须先读取该 Skill。",
            "inputSchema": object_schema({
                "projectId": project_id,
                "stage": {"type": "string", "enum": ["research", "proposal", "script", "scene_plan", "assets", "edit", "compose", "publish"]},
            }, required=["projectId", "stage"]),
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "drama_artifact_save",
            "description": "校验并持久化 AI 生成的短剧制品；宿主 AI 负责内容，本工具只负责校验、版本化和保存。",
            "inputSchema": object_schema({
                "projectId": project_id,
                "expectedRevision": revision,
                "artifactType": {
                    "type": "string",
                    "enum": [
                        "source_document", "research_brief", "episode_plan", "drama_bible", "screenplay",
                        "drama_storyboard", "continuity_report", "selection_manifest", "generation_manifest",
                        "asset_manifest", "voice_plan", "edit_decisions", "render_report", "final_review", "publish_log"
                    ],
                },
                "artifact": {"type": "object", "additionalProperties": True},
            }, required=["projectId", "artifactType", "artifact"]),
        },
        {
            "name": "drama_open_review",
            "description": "基于当前权威状态，在指定审核视图打开交互式短剧 MCP App。",
            "inputSchema": object_schema({
                "projectId": project_id,
                "view": {"type": "string", "enum": ["overview", "bible", "cast", "screenplay", "storyboard", "assets", "voice", "render"], "default": "overview"},
            }, required=["projectId"]),
            "_meta": ui_meta(),
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "drama_selection_commit",
            "description": "使用乐观版本控制提交用户对角色、分镜、资产、配音或成片的选择。",
            "inputSchema": object_schema({
                "projectId": project_id,
                "expectedRevision": revision,
                "scope": {"type": "string", "minLength": 2},
                "selectedIds": {"type": "array", "items": {"type": "string"}, "uniqueItems": True},
                "settings": {"type": "object", "additionalProperties": True},
            }, required=["projectId", "scope", "selectedIds"]),
            "_meta": ui_meta(["model", "app"]),
        },
        {
            "name": "drama_stage_decide",
            "description": "记录短剧阶段具有约束力的人工决定：批准或要求修改。",
            "inputSchema": object_schema({
                "projectId": project_id,
                "expectedRevision": revision,
                "stage": {"type": "string", "enum": ["research", "proposal", "script", "scene_plan", "assets", "edit", "compose", "publish"]},
                "decision": {"type": "string", "enum": ["approve", "revise"], "default": "approve"},
                "note": {"type": "string"},
            }, required=["projectId", "stage", "decision"]),
            "_meta": ui_meta(["model", "app"]),
        },
        {
            "name": "drama_ui_refresh",
            "description": "在 MCP App 内刷新短剧项目的权威状态。",
            "inputSchema": object_schema({"projectId": project_id}, required=["projectId"]),
            "_meta": ui_meta(["app"]),
            "annotations": {"readOnlyHint": True},
        },
    ]
    return definitions


class MessageParser:
    """Parse either Content-Length framed MCP messages or JSONL."""

    def __init__(self) -> None:
        self.buffer = b""
        self.last_framing = "header"

    def push(self, chunk: bytes) -> list[dict[str, Any]]:
        self.buffer += chunk
        messages: list[dict[str, Any]] = []
        while self.buffer:
            header_match = re.match(br"Content-Length:\s*(\d+)[^\r\n]*(?:\r?\n[^\r\n]*)*\r?\n\r?\n", self.buffer, re.IGNORECASE)
            if header_match:
                content_length = int(header_match.group(1))
                body_start = header_match.end()
                body_end = body_start + content_length
                if len(self.buffer) < body_end:
                    break
                body = self.buffer[body_start:body_end]
                self.buffer = self.buffer[body_end:]
                parsed = self._decode(body)
                if parsed is not None:
                    self.last_framing = "header"
                    messages.append(parsed)
                continue

            if self.buffer.lower().startswith(b"content-length:"):
                break

            newline = self.buffer.find(b"\n")
            if newline < 0:
                break
            line = self.buffer[:newline].strip()
            self.buffer = self.buffer[newline + 1:]
            if not line:
                continue
            parsed = self._decode(line)
            if parsed is not None:
                self.last_framing = "jsonl"
                messages.append(parsed)
        return messages

    def frame(self, message: dict[str, Any]) -> bytes:
        body = json.dumps(message, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        if self.last_framing == "jsonl":
            return body + b"\n"
        return f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body

    @staticmethod
    def _decode(body: bytes) -> dict[str, Any] | None:
        try:
            value = json.loads(body.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            return None
        return value if isinstance(value, dict) else None


class OpenMontageMcpServer:
    def __init__(self, *, projects_root: Path | None = None) -> None:
        self.repo_root, self.openmontage_root, self.loaded_env = runner.bootstrap()
        self.drama = ShortDramaService(projects_root)

    def handle_request(self, message: dict[str, Any]) -> dict[str, Any] | None:
        if not is_plain_object(message):
            return None
        request_id = message.get("id") if "id" in message else None
        method = str(message.get("method") or "")
        params = message.get("params") if is_plain_object(message.get("params")) else {}

        if "id" not in message and method.startswith("notifications/"):
            return None

        try:
            if method == "initialize":
                return self.rpc_result(request_id, {
                    "protocolVersion": str(params.get("protocolVersion") or MCP_PROTOCOL_VERSION),
                    "capabilities": {
                        "tools": {},
                        "resources": {"subscribe": False, "listChanged": False},
                    },
                    "serverInfo": {"name": "openmontage-mcp", "version": SERVER_VERSION},
                    "instructions": SERVER_INSTRUCTIONS,
                })
            if method == "ping":
                return self.rpc_result(request_id, {})
            if method == "tools/list":
                return self.rpc_result(request_id, {"tools": tool_definitions()})
            if method == "tools/call":
                name = str(params.get("name") or "").strip()
                if not name:
                    return self.rpc_error(request_id, -32602, "tools/call requires params.name")
                arguments = params.get("arguments") if is_plain_object(params.get("arguments")) else {}
                return self.rpc_result(request_id, self.call_tool(name, arguments))
            if method == "resources/list":
                return self.rpc_result(request_id, {"resources": [self.app_resource_definition()]})
            if method == "resources/read":
                uri = str(params.get("uri") or "")
                if uri != APP_RESOURCE_URI:
                    return self.rpc_error(request_id, -32002, f"Resource not found: {uri}")
                return self.rpc_result(request_id, {"contents": [self.read_app_resource()]})
            return self.rpc_error(request_id, -32601, f"Method not found: {method}")
        except Exception as exc:
            print(f"[openmontage-mcp] request failed: {exc}", file=sys.stderr)
            return self.rpc_error(request_id, -32603, str(exc))

    def call_tool(self, name: str, args: dict[str, Any]) -> dict[str, Any]:
        try:
            payload, show_ui = self._dispatch_tool(name, args)
            is_error = payload.get("ok") is False if isinstance(payload, dict) else False
            return self.tool_result(payload, is_error=is_error, show_ui=show_ui)
        except ShortDramaError as exc:
            payload = {"ok": False, "error": exc.to_dict()}
            if exc.code == "DRAMA_REVISION_CONFLICT" and exc.details.get("currentState"):
                payload["kind"] = "openmontage.short_drama_conflict"
                payload["project"] = exc.details["currentState"]
            return self.tool_result(payload, is_error=True, show_ui=name == "drama_open_review")
        except Exception as exc:
            print(traceback.format_exc(limit=12), file=sys.stderr)
            return self.tool_result({
                "ok": False,
                "error": {"code": "OPENMONTAGE_TOOL_ERROR", "message": str(exc)},
            }, is_error=True)

    def _dispatch_tool(self, name: str, args: dict[str, Any]) -> tuple[dict[str, Any], bool]:
        if name == "env_check":
            return runner.command_env_check(args, self.repo_root, self.openmontage_root, self.loaded_env), False
        if name == "list_pipelines":
            return self.list_pipelines(), False
        if name == "get_pipeline":
            return self.get_pipeline(args), False
        if name == "list_tools":
            return runner.command_list_tools(args), False
        if name == "tool_info":
            return runner.command_tool_info(args), False
        if name == "provider_menu":
            return runner.command_provider_menu(args), False
        if name == "run_tool":
            enriched = {**args, "_repoRoot": str(self.repo_root), "_openMontageRoot": str(self.openmontage_root)}
            return runner.command_run_tool(enriched), False
        if name == "register_asset":
            enriched = {**args, "_repoRoot": str(self.repo_root), "_openMontageRoot": str(self.openmontage_root)}
            return runner.command_register_asset(enriched), False
        if name == "drama_project_create":
            return self.drama.create_project(args), True
        if name == "drama_project_list":
            return self.drama.list_projects(args), False
        if name == "drama_project_get":
            return self.drama.get_project(args), False
        if name == "drama_source_set":
            return self.drama.save_source(args), False
        if name == "drama_stage_context":
            return self.drama.stage_context(args), False
        if name == "drama_artifact_save":
            return self.drama.save_artifact(args), False
        if name == "drama_open_review":
            return self.drama.open_review(args), True
        if name == "drama_selection_commit":
            return self.drama.commit_selection(args), False
        if name == "drama_stage_decide":
            return self.drama.approve_stage(args), False
        if name == "drama_ui_refresh":
            return self.drama.get_project(args), False
        return ({
            "ok": False,
            "error": {"code": "UNKNOWN_TOOL", "message": f"Unknown OpenMontage MCP tool: {name}"},
        }, False)

    def list_pipelines(self) -> dict[str, Any]:
        definitions = self.openmontage_root / "pipeline_defs"
        if not definitions.is_dir():
            return {
                "ok": False,
                "error": {"code": "PIPELINE_DIR_MISSING", "message": f"pipeline_defs not found: {definitions}"},
            }
        pipelines = [
            {"name": path.name, "path": str(path)}
            for path in sorted(definitions.glob("*.y*ml"))
            if path.is_file()
        ]
        return {"ok": True, "openmontageRoot": str(self.openmontage_root), "pipelines": pipelines}

    def get_pipeline(self, args: dict[str, Any]) -> dict[str, Any]:
        name = str(args.get("name") or args.get("pipeline") or "").strip()
        if not re.fullmatch(r"[A-Za-z0-9._-]+\.ya?ml", name):
            return {
                "ok": False,
                "error": {"code": "PIPELINE_NAME_REQUIRED", "message": "Use a pipeline file name such as ai-short-drama.yaml."},
            }
        path = (self.openmontage_root / "pipeline_defs" / name).resolve()
        definitions = (self.openmontage_root / "pipeline_defs").resolve()
        if path.parent != definitions:
            return {"ok": False, "error": {"code": "PIPELINE_PATH_FORBIDDEN", "message": "Invalid pipeline path."}}
        if not path.is_file():
            return {"ok": False, "error": {"code": "PIPELINE_NOT_FOUND", "message": f"{name} not found."}}
        return {"ok": True, "name": name, "path": str(path), "yaml": path.read_text(encoding="utf-8")}

    @staticmethod
    def app_resource_definition() -> dict[str, Any]:
        return {
            "uri": APP_RESOURCE_URI,
            "name": "OpenMontage AI 短剧工作台",
            "description": "用于角色、剧本、分镜、资产、配音和审批的交互式工作台。",
            "mimeType": APP_MIME_TYPE,
        }

    @staticmethod
    def read_app_resource() -> dict[str, Any]:
        if not APP_HTML_PATH.is_file():
            raise FileNotFoundError(f"MCP App HTML not found: {APP_HTML_PATH}")
        return {
            "uri": APP_RESOURCE_URI,
            "mimeType": APP_MIME_TYPE,
            "text": APP_HTML_PATH.read_text(encoding="utf-8"),
            "_meta": {
                "ui": {
                    "csp": {
                        "connectDomains": [],
                        "resourceDomains": [],
                        "frameDomains": [],
                        "baseUriDomains": [],
                    },
                    "prefersBorder": True,
                }
            },
        }

    @staticmethod
    def tool_result(payload: dict[str, Any], *, is_error: bool = False, show_ui: bool = False) -> dict[str, Any]:
        message = str(payload.get("message") or "").strip()
        if not message:
            message = json.dumps(payload, ensure_ascii=False, indent=2)
        result: dict[str, Any] = {
            "content": [{"type": "text", "text": message}],
            "structuredContent": payload,
        }
        if is_error:
            result["isError"] = True
        if show_ui:
            result["_meta"] = ui_meta()
        return result

    @staticmethod
    def rpc_result(request_id: Any, result: dict[str, Any]) -> dict[str, Any]:
        return {"jsonrpc": JSONRPC_VERSION, "id": request_id, "result": result}

    @staticmethod
    def rpc_error(request_id: Any, code: int, message: str, data: Any = None) -> dict[str, Any]:
        error: dict[str, Any] = {"code": code, "message": message}
        if data is not None:
            error["data"] = data
        return {"jsonrpc": JSONRPC_VERSION, "id": request_id, "error": error}


def main() -> int:
    parser = MessageParser()
    server = OpenMontageMcpServer()
    input_stream = sys.stdin.buffer
    output_stream = sys.stdout.buffer
    while True:
        chunk = input_stream.read1(65536)
        if not chunk:
            break
        for message in parser.push(chunk):
            response = server.handle_request(message)
            if response is not None:
                output_stream.write(parser.frame(response))
                output_stream.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
