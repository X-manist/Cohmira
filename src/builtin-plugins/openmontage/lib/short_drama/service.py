"""Application service for the OpenMontage AI short-drama workflow."""

from __future__ import annotations

from copy import deepcopy
from datetime import datetime, timezone
from pathlib import Path
import re
import secrets
from typing import Any

from .store import ProjectStore, ShortDramaError, utc_now


STAGE_ORDER = [
    "research",
    "proposal",
    "script",
    "scene_plan",
    "assets",
    "edit",
    "compose",
    "publish",
]

STAGE_LABELS = {
    "research": "原作与改编目标",
    "proposal": "短剧圣经与角色设定",
    "script": "分集剧本",
    "scene_plan": "导演分镜",
    "assets": "角色与场景资产",
    "edit": "配音与剪辑",
    "compose": "合成与质检",
    "publish": "交付",
}

ARTIFACT_STAGE = {
    "source_document": "research",
    "research_brief": "research",
    "episode_plan": "proposal",
    "drama_bible": "proposal",
    "screenplay": "script",
    "drama_storyboard": "scene_plan",
    "continuity_report": "scene_plan",
    "selection_manifest": "assets",
    "generation_manifest": "assets",
    "asset_manifest": "assets",
    "voice_plan": "edit",
    "edit_decisions": "edit",
    "render_report": "compose",
    "final_review": "compose",
    "publish_log": "publish",
}

STAGE_OUTPUTS = {
    "research": ["source_document", "research_brief"],
    "proposal": ["episode_plan", "drama_bible"],
    "script": ["screenplay"],
    "scene_plan": ["drama_storyboard", "continuity_report"],
    "assets": ["selection_manifest", "generation_manifest", "asset_manifest"],
    "edit": ["voice_plan", "edit_decisions"],
    "compose": ["render_report", "final_review"],
    "publish": ["publish_log"],
}

STAGE_REQUIRED_INPUTS = {
    "research": ["source_document"],
    "proposal": ["source_document"],
    "script": ["source_document", "episode_plan", "drama_bible"],
    "scene_plan": ["drama_bible", "screenplay"],
    "assets": ["drama_bible", "drama_storyboard"],
    "edit": ["screenplay", "drama_storyboard", "asset_manifest"],
    "compose": ["edit_decisions", "asset_manifest"],
    "publish": ["render_report", "final_review"],
}

PRIMARY_STAGE_ARTIFACTS = {
    "research": {"source_document"},
    "proposal": {"drama_bible", "episode_plan"},
    "script": {"screenplay"},
    "scene_plan": {"drama_storyboard"},
    "assets": {"asset_manifest", "generation_manifest", "selection_manifest"},
    "edit": {"edit_decisions", "voice_plan"},
    "compose": {"render_report", "final_review"},
    "publish": {"publish_log"},
}

REVIEW_GATES = {"proposal", "script", "scene_plan", "assets", "compose"}
APP_VIEWS = {"overview", "bible", "cast", "screenplay", "storyboard", "assets", "voice", "render"}


class ShortDramaService:
    """Deterministic project operations exposed to both the agent and MCP App."""

    def __init__(self, projects_root: Path | None = None):
        self.store = ProjectStore(projects_root)

    @staticmethod
    def create_project_id(title: str) -> str:
        latin = re.sub(r"[^a-z0-9]+", "-", str(title or "").lower()).strip("-")
        prefix = (latin[:28] or "drama").strip("-")
        stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
        return f"{prefix}-{stamp}-{secrets.token_hex(2)}"

    def create_project(self, args: dict[str, Any]) -> dict[str, Any]:
        title = str(args.get("title") or "").strip()
        if not title:
            raise ShortDramaError("DRAMA_TITLE_REQUIRED", "title is required.")
        project_id = str(args.get("projectId") or "").strip().lower() or self.create_project_id(title)
        project_id = self.store.validate_project_id(project_id)

        episode_count = self._bounded_int(args.get("episodeCount"), default=1, minimum=1, maximum=200)
        episode_duration = self._bounded_int(
            args.get("episodeDurationSeconds"), default=90, minimum=15, maximum=3600
        )
        aspect_ratio = str(args.get("aspectRatio") or "9:16").strip()
        if aspect_ratio not in {"9:16", "16:9", "1:1", "4:3", "3:4"}:
            raise ShortDramaError("DRAMA_ASPECT_RATIO_INVALID", f"Unsupported aspect ratio: {aspect_ratio}")

        timestamp = utc_now()
        state: dict[str, Any] = {
            "version": "1.0",
            "kind": "openmontage.short_drama_project",
            "projectId": project_id,
            "title": title,
            "revision": 1,
            "status": "active",
            "currentStage": "research",
            "createdAt": timestamp,
            "updatedAt": timestamp,
            "settings": {
                "episodeCount": episode_count,
                "episodeDurationSeconds": episode_duration,
                "aspectRatio": aspect_ratio,
                "genre": str(args.get("genre") or "").strip(),
                "targetAudience": str(args.get("targetAudience") or "").strip(),
                "language": str(args.get("language") or "zh-CN").strip() or "zh-CN",
                "adaptationMode": str(args.get("adaptationMode") or "faithful").strip(),
                "visualStyle": str(args.get("visualStyle") or "").strip(),
            },
            "stages": {
                stage: {
                    "name": stage,
                    "label": STAGE_LABELS[stage],
                    "status": "awaiting_input" if stage == "research" else "blocked",
                    "updatedAt": timestamp,
                }
                for stage in STAGE_ORDER
            },
            "artifacts": {},
            "selections": {},
            "approvals": [],
        }
        created = self.store.create(state)

        source_text = str(args.get("sourceText") or "")
        if source_text.strip():
            created = self.save_source({
                "projectId": project_id,
                "expectedRevision": 1,
                "title": str(args.get("sourceTitle") or title),
                "sourceType": str(args.get("sourceType") or "text"),
                "language": state["settings"]["language"],
                "content": source_text,
            })["project"]
        return self._result(created, message="短剧项目已创建。下一步由 AI 读取 research 阶段 Skill，完成原作分析。")

    def list_projects(self, _args: dict[str, Any] | None = None) -> dict[str, Any]:
        projects = self.store.list()
        return {
            "ok": True,
            "kind": "openmontage.short_drama_project_list",
            "projects": projects,
            "total": len(projects),
        }

    def get_project(self, args: dict[str, Any], *, include_artifacts: bool = True) -> dict[str, Any]:
        project_id = self._project_id(args)
        state = self.store.load(project_id)
        hydrated = self._hydrate(state) if include_artifacts else deepcopy(state)
        return self._result(hydrated)

    def save_source(self, args: dict[str, Any]) -> dict[str, Any]:
        content = str(args.get("content") or args.get("sourceText") or "")
        if not content.strip():
            raise ShortDramaError("DRAMA_SOURCE_REQUIRED", "Source content cannot be empty.")
        if len(content) > 2_000_000:
            raise ShortDramaError("DRAMA_SOURCE_TOO_LARGE", "Source content exceeds 2,000,000 characters.")
        artifact = {
            "version": "1.0",
            "title": str(args.get("title") or args.get("sourceTitle") or "原作").strip() or "原作",
            "source_type": str(args.get("sourceType") or "text").strip() or "text",
            "language": str(args.get("language") or "zh-CN").strip() or "zh-CN",
            "content": content,
            "metadata": args.get("metadata") if isinstance(args.get("metadata"), dict) else {},
        }
        return self.save_artifact({
            "projectId": self._project_id(args),
            "expectedRevision": args.get("expectedRevision"),
            "artifactType": "source_document",
            "artifact": artifact,
        })

    def save_artifact(self, args: dict[str, Any]) -> dict[str, Any]:
        project_id = self._project_id(args)
        artifact_type = str(args.get("artifactType") or "").strip()
        artifact = args.get("artifact")
        if artifact_type not in ARTIFACT_STAGE:
            raise ShortDramaError(
                "DRAMA_ARTIFACT_TYPE_INVALID",
                f"Unsupported artifactType '{artifact_type}'.",
                details={"supported": sorted(ARTIFACT_STAGE)},
            )
        if not isinstance(artifact, dict):
            raise ShortDramaError("DRAMA_ARTIFACT_REQUIRED", "artifact must be a JSON object.")
        self._validate_artifact(artifact_type, artifact)
        stage = ARTIFACT_STAGE[artifact_type]
        expected_revision = self._optional_revision(args.get("expectedRevision"))

        def change(state: dict[str, Any], revision: int, timestamp: str) -> dict[str, Any]:
            relative_path = f"artifacts/{artifact_type}.json"
            self.store.write_artifact(project_id, relative_path, artifact)
            state.setdefault("artifacts", {})[artifact_type] = {
                "type": artifact_type,
                "stage": stage,
                "path": relative_path,
                "revision": revision,
                "updatedAt": timestamp,
            }
            stage_state = state.setdefault("stages", {}).setdefault(stage, {"name": stage})
            if stage in REVIEW_GATES and artifact_type in PRIMARY_STAGE_ARTIFACTS[stage]:
                stage_state["status"] = "awaiting_review"
                state["status"] = "awaiting_human"
            else:
                stage_state["status"] = "ready"
                state["status"] = "active"
            stage_state["updatedAt"] = timestamp
            stage_state["latestArtifact"] = artifact_type
            state["currentStage"] = stage
            return state

        state = self.store.mutate(
            project_id,
            expected_revision=expected_revision,
            event="artifact_saved",
            event_data={"artifactType": artifact_type, "stage": stage},
            change=change,
        )
        message = (
            f"{artifact_type} 已保存，等待用户在短剧工作台确认。"
            if stage in REVIEW_GATES and artifact_type in PRIMARY_STAGE_ARTIFACTS[stage]
            else f"{artifact_type} 已保存。"
        )
        return self._result(self._hydrate(state), message=message, view=self._view_for_artifact(artifact_type))

    def stage_context(self, args: dict[str, Any]) -> dict[str, Any]:
        project_id = self._project_id(args)
        stage = str(args.get("stage") or "").strip()
        if stage not in STAGE_ORDER:
            raise ShortDramaError(
                "DRAMA_STAGE_INVALID",
                f"Unknown stage '{stage}'.",
                details={"supported": STAGE_ORDER},
            )
        state = self._hydrate(self.store.load(project_id))
        required = STAGE_REQUIRED_INPUTS[stage]
        missing = [name for name in required if name not in state.get("artifactData", {})]
        return {
            "ok": True,
            "kind": "openmontage.short_drama_stage_context",
            "projectId": project_id,
            "revision": state.get("revision"),
            "stage": stage,
            "stageLabel": STAGE_LABELS[stage],
            "skillPath": f"skills/pipelines/ai-short-drama/{self._director_name(stage)}-director.md",
            "referencePaths": [
                "skills/references/ai-short-drama/artifact-contracts.md",
                "skills/references/ai-short-drama/continuity.md",
            ],
            "requiredInputs": required,
            "missingInputs": missing,
            "outputArtifacts": STAGE_OUTPUTS[stage],
            "project": state,
            "instruction": (
                "Read the stage Skill before creative work. Use the host AI to produce schema-valid artifacts, "
                "then persist each artifact with drama_artifact_save. Python must not invent creative content."
            ),
        }

    def open_review(self, args: dict[str, Any]) -> dict[str, Any]:
        project_id = self._project_id(args)
        view = str(args.get("view") or "overview").strip().lower()
        if view not in APP_VIEWS:
            raise ShortDramaError(
                "DRAMA_VIEW_INVALID",
                f"Unknown review view '{view}'.",
                details={"supported": sorted(APP_VIEWS)},
            )
        state = self._hydrate(self.store.load(project_id))
        return self._result(state, view=view, message="已打开 AI 短剧工作台。")

    def commit_selection(self, args: dict[str, Any]) -> dict[str, Any]:
        project_id = self._project_id(args)
        scope = str(args.get("scope") or "").strip().lower()
        if not re.fullmatch(r"[a-z][a-z0-9_-]{1,63}", scope):
            raise ShortDramaError("DRAMA_SELECTION_SCOPE_INVALID", "scope must be a stable lowercase identifier.")
        selected_ids = args.get("selectedIds")
        if not isinstance(selected_ids, list) or not all(isinstance(item, str) and item.strip() for item in selected_ids):
            raise ShortDramaError("DRAMA_SELECTION_INVALID", "selectedIds must be a list of non-empty strings.")
        selected = list(dict.fromkeys(item.strip() for item in selected_ids))
        settings = args.get("settings") if isinstance(args.get("settings"), dict) else {}
        expected_revision = self._optional_revision(args.get("expectedRevision"))

        def change(state: dict[str, Any], revision: int, timestamp: str) -> dict[str, Any]:
            state.setdefault("selections", {})[scope] = {
                "scope": scope,
                "selectedIds": selected,
                "settings": settings,
                "revision": revision,
                "updatedAt": timestamp,
            }
            return state

        state = self.store.mutate(
            project_id,
            expected_revision=expected_revision,
            event="selection_committed",
            event_data={"scope": scope, "selectedIds": selected},
            change=change,
        )
        return self._result(
            self._hydrate(state),
            view=self._view_for_scope(scope),
            message=f"已保存 {scope} 选择（{len(selected)} 项）。",
        )

    def approve_stage(self, args: dict[str, Any]) -> dict[str, Any]:
        project_id = self._project_id(args)
        stage = str(args.get("stage") or "").strip()
        decision = str(args.get("decision") or "approve").strip().lower()
        if stage not in STAGE_ORDER:
            raise ShortDramaError("DRAMA_STAGE_INVALID", f"Unknown stage '{stage}'.")
        if decision not in {"approve", "revise"}:
            raise ShortDramaError("DRAMA_DECISION_INVALID", "decision must be 'approve' or 'revise'.")
        note = str(args.get("note") or "").strip()
        expected_revision = self._optional_revision(args.get("expectedRevision"))

        def change(state: dict[str, Any], revision: int, timestamp: str) -> dict[str, Any]:
            if decision == "approve" and not self._stage_has_output(state, stage):
                raise ShortDramaError(
                    "DRAMA_STAGE_OUTPUT_MISSING",
                    f"Stage '{stage}' has no primary artifact to approve.",
                    details={"expected": sorted(PRIMARY_STAGE_ARTIFACTS[stage])},
                )
            stage_state = state.setdefault("stages", {}).setdefault(stage, {"name": stage})
            approval = {
                "id": f"approval-{revision}",
                "stage": stage,
                "decision": decision,
                "note": note,
                "revision": revision,
                "timestamp": timestamp,
            }
            state.setdefault("approvals", []).append(approval)
            stage_state["status"] = "completed" if decision == "approve" else "revision_requested"
            stage_state["updatedAt"] = timestamp
            if decision == "approve":
                next_stage = self._next_stage(stage)
                if next_stage:
                    next_state = state.setdefault("stages", {}).setdefault(next_stage, {"name": next_stage})
                    if next_state.get("status") == "blocked":
                        next_state["status"] = "ready"
                    next_state["updatedAt"] = timestamp
                    state["currentStage"] = next_stage
                    state["status"] = "active"
                else:
                    state["status"] = "completed"
            else:
                state["currentStage"] = stage
                state["status"] = "active"
            return state

        state = self.store.mutate(
            project_id,
            expected_revision=expected_revision,
            event="stage_decision",
            event_data={"stage": stage, "decision": decision},
            change=change,
        )
        action = "已批准" if decision == "approve" else "已要求修改"
        return self._result(
            self._hydrate(state),
            view=self._view_for_stage(stage),
            message=f"{STAGE_LABELS[stage]}{action}。",
        )

    def _hydrate(self, state: dict[str, Any]) -> dict[str, Any]:
        hydrated = deepcopy(state)
        artifact_data: dict[str, Any] = {}
        for name, descriptor in hydrated.get("artifacts", {}).items():
            if not isinstance(descriptor, dict):
                continue
            relative_path = descriptor.get("path")
            if isinstance(relative_path, str):
                value = self.store.load_artifact(str(hydrated.get("projectId")), relative_path)
                if value is not None:
                    artifact_data[name] = value
        hydrated["artifactData"] = artifact_data
        return hydrated

    @staticmethod
    def _result(project: dict[str, Any], *, message: str = "", view: str = "overview") -> dict[str, Any]:
        return {
            "ok": True,
            "kind": "openmontage.short_drama_workspace",
            "projectId": project.get("projectId"),
            "revision": project.get("revision"),
            "view": view,
            "message": message,
            "project": project,
        }

    @staticmethod
    def _project_id(args: dict[str, Any]) -> str:
        value = str(args.get("projectId") or "").strip().lower()
        if not value:
            raise ShortDramaError("DRAMA_PROJECT_ID_REQUIRED", "projectId is required.")
        return value

    @staticmethod
    def _optional_revision(value: Any) -> int | None:
        if value is None or value == "":
            return None
        try:
            revision = int(value)
        except (TypeError, ValueError) as exc:
            raise ShortDramaError("DRAMA_REVISION_INVALID", "expectedRevision must be an integer.") from exc
        if revision < 1:
            raise ShortDramaError("DRAMA_REVISION_INVALID", "expectedRevision must be at least 1.")
        return revision

    @staticmethod
    def _bounded_int(value: Any, *, default: int, minimum: int, maximum: int) -> int:
        if value in (None, ""):
            return default
        try:
            parsed = int(value)
        except (TypeError, ValueError) as exc:
            raise ShortDramaError("DRAMA_NUMBER_INVALID", f"Expected an integer, got {value!r}.") from exc
        if parsed < minimum or parsed > maximum:
            raise ShortDramaError(
                "DRAMA_NUMBER_OUT_OF_RANGE",
                f"Value must be between {minimum} and {maximum}.",
            )
        return parsed

    @staticmethod
    def _validate_artifact(artifact_type: str, artifact: dict[str, Any]) -> None:
        try:
            from schemas.artifacts import list_schemas, validate_artifact

            if artifact_type in list_schemas():
                validate_artifact(artifact_type, artifact)
        except ShortDramaError:
            raise
        except Exception as exc:
            raise ShortDramaError(
                "DRAMA_ARTIFACT_INVALID",
                f"{artifact_type} does not match its artifact schema: {exc}",
            ) from exc

    @staticmethod
    def _next_stage(stage: str) -> str | None:
        index = STAGE_ORDER.index(stage)
        return STAGE_ORDER[index + 1] if index + 1 < len(STAGE_ORDER) else None

    @staticmethod
    def _stage_has_output(state: dict[str, Any], stage: str) -> bool:
        artifacts = state.get("artifacts", {})
        return any(name in artifacts for name in PRIMARY_STAGE_ARTIFACTS[stage])

    @staticmethod
    def _director_name(stage: str) -> str:
        return {
            "research": "research",
            "proposal": "proposal",
            "script": "script",
            "scene_plan": "scene",
            "assets": "asset",
            "edit": "edit",
            "compose": "compose",
            "publish": "publish",
        }[stage]

    @staticmethod
    def _view_for_artifact(artifact_type: str) -> str:
        return {
            "source_document": "overview",
            "research_brief": "overview",
            "episode_plan": "bible",
            "drama_bible": "bible",
            "screenplay": "screenplay",
            "drama_storyboard": "storyboard",
            "continuity_report": "storyboard",
            "selection_manifest": "assets",
            "generation_manifest": "assets",
            "asset_manifest": "assets",
            "voice_plan": "voice",
            "edit_decisions": "voice",
            "render_report": "render",
            "final_review": "render",
            "publish_log": "render",
        }.get(artifact_type, "overview")

    @staticmethod
    def _view_for_scope(scope: str) -> str:
        if "character" in scope or "cast" in scope:
            return "cast"
        if "story" in scope or "shot" in scope:
            return "storyboard"
        if "voice" in scope:
            return "voice"
        if "render" in scope:
            return "render"
        return "assets"

    @staticmethod
    def _view_for_stage(stage: str) -> str:
        return {
            "research": "overview",
            "proposal": "bible",
            "script": "screenplay",
            "scene_plan": "storyboard",
            "assets": "assets",
            "edit": "voice",
            "compose": "render",
            "publish": "render",
        }[stage]

