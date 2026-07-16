#!/usr/bin/env python3
"""OpenMontage tool runner for the Goose MCP bridge.

The Node MCP server keeps protocol/framing simple. This runner owns the Python
side: load the workspace .env, map Beav/Goose env names to OpenMontage provider
envs, discover tools, and execute one requested operation.
"""

from __future__ import annotations

import dataclasses
from contextlib import contextmanager
from datetime import datetime, timezone
import json
import mimetypes
import os
import re
import shutil
import sys
import time
import traceback
import uuid
from pathlib import Path
from typing import Any, Iterator


sys.dont_write_bytecode = True

SECRET_KEY_RE = re.compile(r"(KEY|TOKEN|SECRET|COOKIE|PASSWORD)", re.IGNORECASE)
MEDIA_EXTENSIONS = {
    ".png",
    ".jpg",
    ".jpeg",
    ".webp",
    ".gif",
    ".bmp",
    ".svg",
    ".mp4",
    ".mov",
    ".webm",
    ".m4v",
    ".avi",
    ".mkv",
    ".mp3",
    ".wav",
    ".m4a",
    ".aac",
    ".ogg",
    ".flac",
}


def parse_dotenv_line(line: str) -> tuple[str, str] | None:
    line = line.strip()
    if not line or line.startswith("#") or "=" not in line:
        return None
    key, _, value = line.partition("=")
    key = key.strip()
    value = value.strip()
    if not key:
        return None
    if value[:1] in ("'", '"'):
        quote = value[0]
        end = value.find(quote, 1)
        value = value[1:end] if end != -1 else value[1:]
    else:
        match = re.search(r"(^|\s)#", value)
        if match:
            value = value[: match.start()]
        value = value.strip()
    return key, value


def load_dotenv(path: Path, *, override: bool = False) -> list[str]:
    loaded: list[str] = []
    if not path.is_file():
        return loaded
    for raw in path.read_text(encoding="utf-8", errors="ignore").splitlines():
        parsed = parse_dotenv_line(raw)
        if parsed is None:
            continue
        key, value = parsed
        if override or key not in os.environ:
            os.environ[key] = value
            loaded.append(key)
    return loaded


def repo_root() -> Path:
    env_root = os.environ.get("YUNYINGAGENT_ROOT") or os.environ.get("REPO_ROOT")
    if env_root:
        return Path(env_root).expanduser().resolve()
    for start in (Path(__file__).resolve(), Path.cwd()):
        for current in [start, *start.parents]:
            has_repo_openmontage = (current / "Beav" / "OpenMontage").is_dir()
            has_mcp_source = (current / "mcps" / "openmontage-mcp").is_dir()
            has_dotenv = (current / ".env").is_file()
            if (has_repo_openmontage or has_mcp_source) and has_dotenv:
                return current.resolve()
    # <plugin>/mcp/src/openmontage_runner.py -> plugin root
    return Path(__file__).resolve().parents[2]


def openmontage_root(root: Path) -> Path:
    env_root = os.environ.get("OPENMONTAGE_ROOT")
    if env_root and Path(env_root).expanduser().is_dir():
        return Path(env_root).expanduser().resolve()
    plugin_root = Path(__file__).resolve().parents[2]
    if (plugin_root / "tools").is_dir():
        return plugin_root.resolve()
    return root.resolve()


def first_env(*keys: str) -> str:
    for key in keys:
        value = str(os.environ.get(key) or "").strip()
        if value:
            return value
    return ""


def set_if_missing(key: str, value: str) -> None:
    if value and not str(os.environ.get(key) or "").strip():
        os.environ[key] = value


def normalize_plan_base_url(value: str) -> str:
    raw = str(value or "").strip().rstrip("/")
    if not raw:
        return "https://ark.cn-beijing.volces.com/api/plan/v3"
    for suffix in (
        "/contents/generations/tasks",
        "/images/generations",
        "/contents/generations/tasks/{id}",
    ):
        if raw.endswith(suffix):
            raw = raw[: -len(suffix)].rstrip("/")
    return raw


def apply_env_aliases() -> None:
    # OpenAI-compatible image generation.
    set_if_missing("OPENAI_API_KEY", first_env("OPENAI_API_KEY", "OPENAI_IMAGE_API_KEY", "BEAV_IMAGE_API_KEY"))
    set_if_missing("OPENAI_IMAGE_MODEL", first_env("OPENAI_IMAGE_MODEL", "BEAV_IMAGE_MODEL", "GOOSE_IMAGE_MODEL"))
    set_if_missing("OPENAI_IMAGE_ENDPOINT", first_env("OPENAI_IMAGE_ENDPOINT", "BEAV_IMAGE_ENDPOINT"))

    # Volcengine Ark Plan video/image generation.
    ark_key = first_env("ARK_API_KEY", "SEEDANCE_API_KEY", "BEAV_VIDEO_API_KEY")
    set_if_missing("ARK_API_KEY", ark_key)
    set_if_missing("SEEDANCE_API_KEY", first_env("SEEDANCE_API_KEY", "BEAV_VIDEO_API_KEY", "ARK_API_KEY"))
    set_if_missing("SEEDANCE_VIDEO_MODEL", first_env("SEEDANCE_VIDEO_MODEL", "BEAV_VIDEO_MODEL", "ARK_SEEDANCE_MODEL"))
    set_if_missing("ARK_SEEDANCE_MODEL", first_env("ARK_SEEDANCE_MODEL", "SEEDANCE_VIDEO_MODEL", "BEAV_VIDEO_MODEL", "doubao-seedance-1.5-pro"))
    set_if_missing("ARK_SEEDREAM_MODEL", first_env("ARK_SEEDREAM_MODEL", "SEEDREAM_IMAGE_MODEL", "doubao-seedream-5.0-lite"))

    plan_base = normalize_plan_base_url(first_env("ARK_BASE_URL", "SEEDANCE_BASE_URL", "BEAV_VIDEO_ENDPOINT"))
    set_if_missing("ARK_BASE_URL", plan_base)
    set_if_missing("SEEDANCE_BASE_URL", plan_base)

    os.environ.setdefault("PYTHONIOENCODING", "utf-8")


def bootstrap() -> tuple[Path, Path, list[str]]:
    root = repo_root()
    om_root = openmontage_root(root)
    loaded: list[str] = []
    loaded.extend(load_dotenv(root / ".env", override=False))
    loaded.extend(load_dotenv(om_root / ".env", override=False))
    apply_env_aliases()
    if str(om_root) not in sys.path:
        sys.path.insert(0, str(om_root))
    os.chdir(om_root)
    return root, om_root, loaded


def scrub(value: Any) -> Any:
    if isinstance(value, dict):
        next_value: dict[str, Any] = {}
        for k, v in value.items():
            if SECRET_KEY_RE.search(str(k)) and isinstance(v, str) and v:
                next_value[k] = "***"
            else:
                next_value[k] = scrub(v)
        return next_value
    if isinstance(value, list):
        return [scrub(item) for item in value]
    if isinstance(value, tuple):
        return [scrub(item) for item in value]
    if dataclasses.is_dataclass(value):
        return scrub(dataclasses.asdict(value))
    return value


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def compact_record(value: dict[str, Any]) -> dict[str, Any]:
    return {k: v for k, v in value.items() if v not in (None, "")}


def normalize_store_path(value: Path | str) -> str:
    return str(value).replace("\\", "/")


def media_root(root: Path) -> Path:
    explicit = first_env("BEAV_MEDIA_ROOT")
    if explicit:
        return Path(explicit).expanduser().resolve()
    return (root / "media").resolve()


def guess_mime_type(path_value: Path) -> str:
    guessed, _ = mimetypes.guess_type(str(path_value))
    if guessed:
        return guessed.lower()
    suffix = path_value.suffix.lower()
    if suffix in {".jpg", ".jpeg"}:
        return "image/jpeg"
    if suffix == ".png":
        return "image/png"
    if suffix == ".webp":
        return "image/webp"
    if suffix == ".gif":
        return "image/gif"
    if suffix == ".mp4":
        return "video/mp4"
    if suffix == ".mov":
        return "video/quicktime"
    if suffix == ".webm":
        return "video/webm"
    if suffix == ".mp3":
        return "audio/mpeg"
    if suffix == ".wav":
        return "audio/wav"
    return "application/octet-stream"


@contextmanager
def catalog_lock(root: Path) -> Iterator[None]:
    root.mkdir(parents=True, exist_ok=True)
    lock_path = root / "catalog.json.lock"
    deadline = time.monotonic() + 30
    fd: int | None = None
    while fd is None:
        try:
            fd = os.open(str(lock_path), os.O_CREAT | os.O_EXCL | os.O_WRONLY)
            os.write(fd, f"{os.getpid()} {now_iso()}\n".encode("utf-8"))
            break
        except FileExistsError:
            if time.monotonic() >= deadline:
                try:
                    if time.time() - lock_path.stat().st_mtime > 120:
                        lock_path.unlink()
                        continue
                except FileNotFoundError:
                    continue
                raise TimeoutError(f"Timed out waiting for media catalog lock: {lock_path}")
            time.sleep(0.1)
    try:
        yield
    finally:
        if fd is not None:
            os.close(fd)
        try:
            lock_path.unlink()
        except FileNotFoundError:
            pass


def read_media_catalog(root: Path) -> dict[str, Any]:
    catalog_path = root / "catalog.json"
    if not catalog_path.is_file():
        return {"version": 1, "assets": []}
    try:
        parsed = json.loads(catalog_path.read_text(encoding="utf-8"))
        if isinstance(parsed, dict) and isinstance(parsed.get("assets"), list):
            return {"version": 1, "assets": parsed["assets"]}
    except Exception:
        pass
    return {"version": 1, "assets": []}


def write_media_catalog(root: Path, catalog: dict[str, Any]) -> None:
    root.mkdir(parents=True, exist_ok=True)
    (root / "generated").mkdir(parents=True, exist_ok=True)
    catalog_path = root / "catalog.json"
    tmp_path = root / f".catalog.{os.getpid()}.{uuid.uuid4().hex}.tmp"
    tmp_path.write_text(json.dumps(catalog, ensure_ascii=False, indent=2), encoding="utf-8")
    os.replace(tmp_path, catalog_path)


def resolve_artifact_path(raw: Any, root: Path, om_root: Path) -> Path | None:
    if raw is None:
        return None
    text = str(raw).strip()
    if not text or re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", text):
        return None
    candidate = Path(text).expanduser()
    candidates = [candidate] if candidate.is_absolute() else [
        (root / candidate),
        (om_root / candidate),
        (Path.cwd() / candidate),
    ]
    for item in candidates:
        try:
            resolved = item.resolve()
            if resolved.is_file() and resolved.suffix.lower() in MEDIA_EXTENSIONS:
                return resolved
        except Exception:
            continue
    return None


def collect_artifact_paths(result: dict[str, Any], root: Path, om_root: Path) -> list[Path]:
    paths: list[Path] = []

    def add(raw: Any) -> None:
        path_value = resolve_artifact_path(raw, root, om_root)
        if path_value and path_value not in paths:
            paths.append(path_value)

    artifacts = result.get("artifacts")
    if isinstance(artifacts, list):
        for artifact in artifacts:
            add(artifact)

    data = result.get("data")
    if isinstance(data, dict):
        for key in ("output_path", "output", "file_path", "path", "file"):
            add(data.get(key))
        outputs = data.get("outputs")
        if isinstance(outputs, list):
            for output in outputs:
                if isinstance(output, dict):
                    for key in ("output_path", "output", "file_path", "path", "file"):
                        add(output.get(key))
                else:
                    add(output)

    return paths


def asset_metadata(inputs: dict[str, Any], result: dict[str, Any], tool_name: str, source_path: Path, relative_path: str) -> dict[str, Any]:
    data = result.get("data") if isinstance(result.get("data"), dict) else {}
    prompt = str(data.get("prompt") or inputs.get("prompt") or "").strip()
    project_id = result_project_id(inputs, result) or None
    delivery_role = result_delivery_role(inputs, result) or None
    model = str(result.get("model") or data.get("model") or inputs.get("model") or "").strip() or None
    provider = str(data.get("provider") or inputs.get("provider") or "openmontage").strip() or None
    provider_template = str(data.get("gateway") or inputs.get("gateway") or data.get("providerTemplate") or "").strip() or None
    aspect_ratio = str(data.get("aspect_ratio") or inputs.get("aspect_ratio") or inputs.get("ratio") or "").strip() or None
    size = str(data.get("size") or inputs.get("size") or data.get("resolution") or inputs.get("resolution") or "").strip() or None
    quality = str(data.get("quality") or inputs.get("quality") or "").strip() or None
    title = str(inputs.get("title") or data.get("title") or f"{tool_name}: {source_path.stem}").strip()
    created = now_iso()
    return compact_record({
        "id": f"media_{int(time.time() * 1000)}_{uuid.uuid4().hex[:8]}",
        "source": "generated",
        "projectId": project_id,
        "deliveryRole": delivery_role,
        "title": title,
        "prompt": re.sub(r"\s+", " ", prompt).strip() if prompt else None,
        "provider": provider,
        "providerTemplate": provider_template,
        "model": model,
        "aspectRatio": aspect_ratio,
        "size": size,
        "quality": quality,
        "mimeType": guess_mime_type(source_path),
        "relativePath": relative_path,
        "createdAt": created,
        "updatedAt": created,
    })


def result_project_id(inputs: dict[str, Any], result: dict[str, Any]) -> str:
    data = result.get("data") if isinstance(result.get("data"), dict) else {}
    return str(
        inputs.get("projectId")
        or inputs.get("project_id")
        or inputs.get("video_project_id")
        or data.get("projectId")
        or data.get("project_id")
        or ""
    ).strip()


def result_delivery_role(inputs: dict[str, Any], result: dict[str, Any]) -> str:
    data = result.get("data") if isinstance(result.get("data"), dict) else {}
    return str(
        data.get("delivery_role")
        or data.get("deliveryRole")
        or inputs.get("delivery_role")
        or inputs.get("deliveryRole")
        or ""
    ).strip()


def media_project_folder(project_id: str) -> str:
    normalized = re.sub(r"[^\w\u4e00-\u9fff]+", "-", str(project_id or "").strip().lower())
    normalized = re.sub(r"-+", "-", normalized).strip("-_")
    return normalized[:80] or "untitled-project"


def media_asset_category(inputs: dict[str, Any], result: dict[str, Any], source_path: Path) -> str:
    role = result_delivery_role(inputs, result).lower()
    if "final" in role or "output" in role:
        return "output"
    mime_type = guess_mime_type(source_path).lower()
    if mime_type.startswith("image/"):
        return "images"
    if mime_type.startswith("video/"):
        return "clips"
    if mime_type.startswith("audio/"):
        return "audio"
    return "files"


def register_result_media_assets(
    *,
    root: Path,
    om_root: Path,
    inputs: dict[str, Any],
    result: dict[str, Any],
    tool_name: str,
) -> list[dict[str, Any]]:
    if not bool(result.get("success")):
        return []

    sources = collect_artifact_paths(result, root, om_root)
    if not sources:
        return []

    mroot = media_root(root)
    project_id = result_project_id(inputs, result)
    delivery_role = result_delivery_role(inputs, result)
    registered: list[dict[str, Any]] = []

    with catalog_lock(mroot):
        catalog = read_media_catalog(mroot)
        assets = catalog.setdefault("assets", [])
        for source_path in sources:
            source_category = media_asset_category(inputs, result, source_path)
            target_dir = mroot / "generated"
            if project_id:
                target_dir = target_dir / media_project_folder(project_id) / source_category
            target_dir.mkdir(parents=True, exist_ok=True)
            try:
                relative_to_media = source_path.relative_to(mroot)
                target_path = source_path
                relative_path = normalize_store_path(relative_to_media)
                existing_source_asset = next((
                    item for item in assets
                    if isinstance(item, dict) and item.get("relativePath") == relative_path
                ), None)
                if project_id:
                    expected_parent = target_dir.resolve()
                    current_parent = source_path.parent.resolve()
                    if current_parent != expected_parent:
                        target_path = target_dir / source_path.name
                        if target_path.exists() and target_path.resolve() != source_path.resolve():
                            target_path = target_dir / f"media_{int(time.time() * 1000)}_{uuid.uuid4().hex[:8]}_{source_path.name}"
                        shutil.move(str(source_path), str(target_path))
                        relative_path = normalize_store_path(target_path.relative_to(mroot))
                        if existing_source_asset:
                            existing_source_asset["relativePath"] = relative_path
                            existing_source_asset["projectId"] = project_id
                            if delivery_role:
                                existing_source_asset["deliveryRole"] = delivery_role
                            existing_source_asset["updatedAt"] = now_iso()
            except ValueError:
                suffix = source_path.suffix.lower() or ".bin"
                target_name = f"media_{int(time.time() * 1000)}_{uuid.uuid4().hex[:8]}_{source_path.name}"
                if not target_name.lower().endswith(suffix):
                    target_name = f"{target_name}{suffix}"
                target_path = target_dir / target_name
                shutil.copy2(source_path, target_path)
                relative_path = normalize_store_path(target_path.relative_to(mroot))

            existing = next((item for item in assets if isinstance(item, dict) and item.get("relativePath") == relative_path), None)
            if existing:
                if project_id:
                    existing["projectId"] = project_id
                if delivery_role:
                    existing["deliveryRole"] = delivery_role
                if project_id or delivery_role:
                    existing["updatedAt"] = now_iso()
                returned = dict(existing)
                returned["absolutePath"] = str(target_path)
                registered.append(returned)
                continue

            asset = asset_metadata(inputs, result, tool_name, target_path, relative_path)
            assets.append(asset)
            returned = dict(asset)
            returned["absolutePath"] = str(target_path)
            registered.append(returned)
        write_media_catalog(mroot, catalog)

    return registered


def promote_registered_media_outputs(
    result: dict[str, Any],
    media_assets: list[dict[str, Any]],
) -> dict[str, Any]:
    """Expose durable media-library paths as the tool's public outputs.

    Tools may render into /tmp or another working directory, but those paths are
    implementation details. Downstream tools and the final assistant response
    should use the registered JiubanAI media-library files.
    """
    managed_paths = list(dict.fromkeys(
        str(asset.get("absolutePath") or "").strip()
        for asset in media_assets
        if isinstance(asset, dict) and str(asset.get("absolutePath") or "").strip()
    ))
    if not managed_paths:
        return result

    data = result.get("data") if isinstance(result.get("data"), dict) else {}
    data["output"] = managed_paths[0]
    data["output_path"] = managed_paths[0]
    data["storage"] = "jiuban_media_library"
    result["data"] = data
    result["artifacts"] = managed_paths
    return result


def safe_tool_info(tool: Any, include_schema: bool = True) -> dict[str, Any]:
    try:
        info = tool.get_info()
    except Exception as exc:
        info = {
            "name": getattr(tool, "name", ""),
            "provider": getattr(tool, "provider", ""),
            "capability": getattr(tool, "capability", ""),
            "status": "error",
            "error": str(exc),
        }
    if not include_schema:
        info.pop("input_schema", None)
        info.pop("output_schema", None)
        info.pop("artifact_schema", None)
    return scrub(info)


def load_registry() -> Any:
    from tools.tool_registry import registry

    registry.clear()
    registry.discover("tools")
    return registry


def discovery_errors(registry: Any) -> list[dict[str, str]]:
    errors = getattr(registry, "discovery_errors", None)
    if callable(errors):
        return scrub(errors())
    return scrub(getattr(registry, "_discovery_errors", []))


def env_snapshot(root: Path, om_root: Path, loaded_keys: list[str]) -> dict[str, Any]:
    keys = [
        "OPENAI_BASE_URL",
        "OPENAI_IMAGE_ENDPOINT",
        "OPENAI_IMAGE_MODEL",
        "OPENAI_API_KEY",
        "OPENMONTAGE_VISION_ANALYZER_PROVIDER",
        "OPENMONTAGE_VISION_ANALYZER_MODEL",
        "BEAV_IMAGE_ENDPOINT",
        "BEAV_IMAGE_MODEL",
        "BEAV_IMAGE_PROVIDER_TEMPLATE",
        "ARK_BASE_URL",
        "ARK_API_KEY",
        "ARK_SEEDANCE_MODEL",
        "ARK_SEEDREAM_MODEL",
        "SEEDANCE_BASE_URL",
        "SEEDANCE_VIDEO_MODEL",
        "SEEDANCE_API_KEY",
        "BEAV_VIDEO_ENDPOINT",
        "BEAV_VIDEO_MODEL",
        "BEAV_VIDEO_API_KEY",
    ]
    return scrub({
        "repoRoot": str(root),
        "openmontageRoot": str(om_root),
        "python": sys.executable,
        "loadedDotenvKeys": sorted(set(loaded_keys)),
        "config": {key: os.environ.get(key, "") for key in keys if os.environ.get(key, "")},
    })


def command_env_check(args: dict[str, Any], root: Path, om_root: Path, loaded: list[str]) -> dict[str, Any]:
    result = {
        "ok": om_root.is_dir(),
        "env": env_snapshot(root, om_root, loaded),
        "paths": {
            "pipelineDefs": str(om_root / "pipeline_defs"),
            "tools": str(om_root / "tools"),
            "skills": str(om_root / "skills"),
        },
    }
    if args.get("discover"):
        registry = load_registry()
        result["toolCount"] = len(registry.list_all())
        result["discoveryErrors"] = discovery_errors(registry)[:20]
    return result


def command_list_tools(args: dict[str, Any]) -> dict[str, Any]:
    registry = load_registry()
    capability = str(args.get("capability") or "").strip()
    capability_aliases = {
        "video": {"video_generation", "video_post", "stock_video"},
        "image": {"image_generation", "enhancement"},
        "audio": {"audio_processing", "music_generation", "music_search", "tts"},
    }
    accepted_capabilities = capability_aliases.get(capability, {capability} if capability else set())
    provider = str(args.get("provider") or "").strip()
    include_schema = bool(args.get("includeSchemas", False))
    limit = int(args.get("limit") or 200)
    tools = []
    for name in registry.list_all():
        tool = registry.get(name)
        if tool is None:
            continue
        if accepted_capabilities and getattr(tool, "capability", "") not in accepted_capabilities:
            continue
        if provider and getattr(tool, "provider", "") != provider:
            continue
        tools.append(safe_tool_info(tool, include_schema=include_schema))
    tools.sort(key=lambda item: (str(item.get("capability", "")), str(item.get("provider", "")), str(item.get("name", ""))))
    return {
        "ok": True,
        "tools": tools[:limit],
        "total": len(tools),
        "discoveryErrors": discovery_errors(registry)[:20],
    }


def command_tool_info(args: dict[str, Any]) -> dict[str, Any]:
    name = str(args.get("name") or "").strip()
    if not name:
        return {"ok": False, "error": {"code": "NAME_REQUIRED", "message": "tool_info requires name"}}
    registry = load_registry()
    tool = registry.get(name)
    if tool is None:
        return {"ok": False, "error": {"code": "TOOL_NOT_FOUND", "message": f"No OpenMontage tool named {name}"}}
    return {"ok": True, "tool": safe_tool_info(tool, include_schema=True), "discoveryErrors": discovery_errors(registry)[:20]}


def command_provider_menu(args: dict[str, Any]) -> dict[str, Any]:
    registry = load_registry()
    summary = registry.provider_menu_summary() if args.get("summary", True) else registry.provider_menu()
    return {"ok": True, "menu": scrub(summary), "discoveryErrors": discovery_errors(registry)[:20]}


def command_run_tool(args: dict[str, Any]) -> dict[str, Any]:
    root = Path(args.get("_repoRoot") or repo_root())
    om_root = Path(args.get("_openMontageRoot") or openmontage_root(root))
    name = str(args.get("name") or "").strip()
    if not name:
        return {"ok": False, "error": {"code": "NAME_REQUIRED", "message": "run_tool requires name"}}
    raw_inputs = args.get("inputs") if isinstance(args.get("inputs"), dict) else {}
    inputs = dict(raw_inputs)
    output_path = str(inputs.get("output_path") or "").strip()
    if output_path and not Path(output_path).expanduser().is_absolute():
        inputs["output_path"] = str((root / output_path).resolve())
    dry_run = bool(args.get("dryRun", True))
    confirm = bool(args.get("confirm", False))
    if not dry_run and not confirm:
        return {
            "ok": False,
            "blocked": True,
            "error": {
                "code": "CONFIRMATION_REQUIRED",
                "message": "Real OpenMontage execution requires dryRun=false and confirm=true.",
            },
        }
    registry = load_registry()
    tool = registry.get(name)
    if tool is None:
        return {"ok": False, "error": {"code": "TOOL_NOT_FOUND", "message": f"No OpenMontage tool named {name}"}}

    if dry_run:
        return {
            "ok": True,
            "dryRun": True,
            "tool": name,
            "result": scrub(tool.dry_run(inputs)),
            "discoveryErrors": discovery_errors(registry)[:20],
        }

    result = tool.execute(inputs)
    scrubbed_result = scrub(result)
    media_assets = register_result_media_assets(
        root=root,
        om_root=om_root,
        inputs=inputs,
        result=scrubbed_result if isinstance(scrubbed_result, dict) else {},
        tool_name=name,
    )
    if isinstance(scrubbed_result, dict):
        scrubbed_result = promote_registered_media_outputs(scrubbed_result, media_assets)
    result_data = (
        scrubbed_result.get("data")
        if isinstance(scrubbed_result, dict) and isinstance(scrubbed_result.get("data"), dict)
        else {}
    )
    chat_visibility = str(result_data.get("chat_visibility") or "").strip()
    delivery_role = str(result_data.get("delivery_role") or "").strip()
    response = {
        "ok": bool(getattr(result, "success", False)),
        "dryRun": False,
        "tool": name,
        "result": scrubbed_result,
        "mediaAssets": scrub(media_assets),
        "discoveryErrors": discovery_errors(registry)[:20],
    }
    if chat_visibility:
        response["chatVisibility"] = chat_visibility
    if delivery_role:
        response["deliveryRole"] = delivery_role
    return response


def command_register_asset(args: dict[str, Any]) -> dict[str, Any]:
    root = Path(args.get("_repoRoot") or repo_root())
    om_root = Path(args.get("_openMontageRoot") or openmontage_root(root))
    raw_path = args.get("path") or args.get("output_path")
    source_path = resolve_artifact_path(raw_path, root, om_root)
    if source_path is None:
        return {
            "ok": False,
            "error": {
                "code": "ASSET_NOT_FOUND",
                "message": f"No supported local media file found at {raw_path}",
            },
        }
    inputs = args.get("inputs") if isinstance(args.get("inputs"), dict) else {}
    data = args.get("data") if isinstance(args.get("data"), dict) else {}
    result = {
        "success": True,
        "data": {
            **data,
            "output_path": str(source_path),
        },
        "artifacts": [str(source_path)],
        "model": data.get("model") or args.get("model"),
    }
    media_assets = register_result_media_assets(
        root=root,
        om_root=om_root,
        inputs=dict(inputs),
        result=result,
        tool_name=str(args.get("tool") or "openmontage_asset"),
    )
    return {
        "ok": True,
        "path": str(source_path),
        "mediaAssets": scrub(media_assets),
    }


def read_json_arg() -> dict[str, Any]:
    if len(sys.argv) < 3:
        return {}
    raw = sys.argv[2]
    if raw == "-":
        raw = sys.stdin.read()
    try:
        data = json.loads(raw or "{}")
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


def main() -> int:
    command = sys.argv[1] if len(sys.argv) > 1 else "env_check"
    args = read_json_arg()
    try:
        root, om_root, loaded = bootstrap()
        if command == "env_check":
            payload = command_env_check(args, root, om_root, loaded)
        elif command == "list_tools":
            payload = command_list_tools(args)
        elif command == "tool_info":
            payload = command_tool_info(args)
        elif command == "provider_menu":
            payload = command_provider_menu(args)
        elif command == "run_tool":
            args["_repoRoot"] = str(root)
            args["_openMontageRoot"] = str(om_root)
            payload = command_run_tool(args)
        elif command == "register_asset":
            args["_repoRoot"] = str(root)
            args["_openMontageRoot"] = str(om_root)
            payload = command_register_asset(args)
        else:
            payload = {"ok": False, "error": {"code": "UNKNOWN_COMMAND", "message": command}}
    except Exception as exc:
        payload = {
            "ok": False,
            "error": {
                "code": "RUNNER_ERROR",
                "message": str(exc),
                "traceback": traceback.format_exc(limit=12),
            },
        }
    print(json.dumps(scrub(payload), ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
