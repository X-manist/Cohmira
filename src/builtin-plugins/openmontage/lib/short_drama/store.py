"""Versioned, filesystem-backed state for AI short-drama projects.

The AI agent remains the creative intelligence. This module is deliberately
deterministic: it owns persistence, optimistic concurrency, revision history,
and safe project paths so both chat tools and the MCP App see one truth.
"""

from __future__ import annotations

from contextlib import contextmanager
from copy import deepcopy
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import time
import uuid
from typing import Any, Callable, Iterator

from lib.paths import PROJECTS_DIR


PROJECT_ID_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{2,63}$")


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


class ShortDramaError(RuntimeError):
    """Structured domain error returned through MCP tool results."""

    def __init__(self, code: str, message: str, *, details: dict[str, Any] | None = None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details or {}

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {"code": self.code, "message": self.message}
        if self.details:
            payload["details"] = self.details
        return payload


class ProjectStore:
    """Read and mutate short-drama projects under ``projects/<project_id>``."""

    def __init__(self, projects_root: Path | None = None):
        resolved_root = projects_root
        if resolved_root is None:
            explicit = str(os.environ.get("OPENMONTAGE_PROJECTS_DIR") or "").strip()
            workspace = str(os.environ.get("YUNYINGAGENT_ROOT") or "").strip()
            if explicit:
                resolved_root = Path(explicit)
            elif workspace:
                resolved_root = Path(workspace) / "openmontage-projects"
            else:
                resolved_root = PROJECTS_DIR
        self.projects_root = Path(resolved_root).expanduser().resolve()

    def validate_project_id(self, project_id: str) -> str:
        value = str(project_id or "").strip().lower()
        if not PROJECT_ID_RE.fullmatch(value):
            raise ShortDramaError(
                "DRAMA_PROJECT_ID_INVALID",
                "projectId must be 3-64 characters using lowercase letters, numbers, '_' or '-'.",
            )
        return value

    def project_dir(self, project_id: str) -> Path:
        value = self.validate_project_id(project_id)
        candidate = (self.projects_root / value).resolve()
        if candidate.parent != self.projects_root:
            raise ShortDramaError("DRAMA_PROJECT_PATH_FORBIDDEN", "Project path escapes the projects root.")
        return candidate

    def state_path(self, project_id: str) -> Path:
        return self.project_dir(project_id) / "short_drama" / "state.json"

    def create(self, state: dict[str, Any]) -> dict[str, Any]:
        project_id = self.validate_project_id(str(state.get("projectId") or ""))
        project_dir = self.project_dir(project_id)
        state_path = self.state_path(project_id)
        if state_path.exists():
            raise ShortDramaError(
                "DRAMA_PROJECT_EXISTS",
                f"Short-drama project '{project_id}' already exists.",
                details={"projectId": project_id},
            )

        for relative in (
            "short_drama/history",
            "artifacts",
            "assets/images",
            "assets/video",
            "assets/audio",
            "assets/music",
            "renders",
        ):
            (project_dir / relative).mkdir(parents=True, exist_ok=True)

        marker = {
            "version": "1.0",
            "project_id": project_id,
            "title": state.get("title") or project_id,
            "pipeline_type": "ai-short-drama",
            "created_at": state.get("createdAt") or utc_now(),
        }
        self._atomic_write_json(project_dir / "project.json", marker)
        self._atomic_write_json(state_path, state)
        self.append_event(project_id, "project_created", {"revision": state.get("revision", 1)})
        return deepcopy(state)

    def load(self, project_id: str) -> dict[str, Any]:
        path = self.state_path(project_id)
        if not path.is_file():
            raise ShortDramaError(
                "DRAMA_PROJECT_NOT_FOUND",
                f"Short-drama project '{project_id}' was not found.",
                details={"projectId": project_id},
            )
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ShortDramaError(
                "DRAMA_STATE_INVALID",
                f"Unable to read project state for '{project_id}': {exc}",
            ) from exc
        if not isinstance(data, dict):
            raise ShortDramaError("DRAMA_STATE_INVALID", "Project state must be a JSON object.")
        return data

    def list(self) -> list[dict[str, Any]]:
        if not self.projects_root.is_dir():
            return []
        projects: list[dict[str, Any]] = []
        for path in self.projects_root.glob("*/short_drama/state.json"):
            try:
                state = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if not isinstance(state, dict):
                continue
            projects.append({
                "projectId": state.get("projectId"),
                "title": state.get("title"),
                "revision": state.get("revision"),
                "status": state.get("status"),
                "currentStage": state.get("currentStage"),
                "updatedAt": state.get("updatedAt"),
                "settings": state.get("settings", {}),
            })
        projects.sort(key=lambda item: str(item.get("updatedAt") or ""), reverse=True)
        return projects

    def load_artifact(self, project_id: str, relative_path: str) -> dict[str, Any] | None:
        project_dir = self.project_dir(project_id)
        candidate = (project_dir / str(relative_path or "")).resolve()
        if candidate != project_dir and project_dir not in candidate.parents:
            raise ShortDramaError("DRAMA_ARTIFACT_PATH_FORBIDDEN", "Artifact path escapes the project.")
        if not candidate.is_file():
            return None
        try:
            value = json.loads(candidate.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return None
        return value if isinstance(value, dict) else None

    def write_artifact(self, project_id: str, relative_path: str, value: dict[str, Any]) -> Path:
        project_dir = self.project_dir(project_id)
        candidate = (project_dir / relative_path).resolve()
        if project_dir not in candidate.parents:
            raise ShortDramaError("DRAMA_ARTIFACT_PATH_FORBIDDEN", "Artifact path escapes the project.")
        candidate.parent.mkdir(parents=True, exist_ok=True)
        self._atomic_write_json(candidate, value)
        return candidate

    def mutate(
        self,
        project_id: str,
        *,
        expected_revision: int | None,
        event: str,
        change: Callable[[dict[str, Any], int, str], dict[str, Any] | None],
        event_data: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        with self.lock(project_id):
            state = self.load(project_id)
            current_revision = int(state.get("revision") or 0)
            if expected_revision is not None and expected_revision != current_revision:
                raise ShortDramaError(
                    "DRAMA_REVISION_CONFLICT",
                    "Project changed after the UI or agent loaded it. Refresh before saving.",
                    details={
                        "projectId": project_id,
                        "expectedRevision": expected_revision,
                        "currentRevision": current_revision,
                        "currentState": state,
                    },
                )

            next_revision = current_revision + 1
            timestamp = utc_now()
            original = deepcopy(state)
            changed = change(state, next_revision, timestamp)
            next_state = changed if isinstance(changed, dict) else state
            next_state["revision"] = next_revision
            next_state["updatedAt"] = timestamp

            history_path = (
                self.project_dir(project_id)
                / "short_drama"
                / "history"
                / f"revision-{current_revision:06d}.json"
            )
            if not history_path.exists():
                self._atomic_write_json(history_path, original)
            self._atomic_write_json(self.state_path(project_id), next_state)
            self.append_event(project_id, event, {"revision": next_revision, **(event_data or {})})
            return deepcopy(next_state)

    @contextmanager
    def lock(self, project_id: str, timeout_seconds: float = 15.0) -> Iterator[None]:
        lock_path = self.project_dir(project_id) / "short_drama" / ".state.lock"
        lock_path.parent.mkdir(parents=True, exist_ok=True)
        deadline = time.monotonic() + timeout_seconds
        descriptor: int | None = None
        while descriptor is None:
            try:
                descriptor = os.open(str(lock_path), os.O_CREAT | os.O_EXCL | os.O_WRONLY)
                os.write(descriptor, f"{os.getpid()} {utc_now()}\n".encode("utf-8"))
            except FileExistsError:
                try:
                    if time.time() - lock_path.stat().st_mtime > 120:
                        lock_path.unlink()
                        continue
                except FileNotFoundError:
                    continue
                if time.monotonic() >= deadline:
                    raise ShortDramaError(
                        "DRAMA_PROJECT_BUSY",
                        "Project is being updated by another action. Try again shortly.",
                    )
                time.sleep(0.05)
        try:
            yield
        finally:
            if descriptor is not None:
                os.close(descriptor)
            try:
                lock_path.unlink()
            except FileNotFoundError:
                pass

    def append_event(self, project_id: str, event: str, data: dict[str, Any]) -> None:
        path = self.project_dir(project_id) / "short_drama" / "events.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        record = {"timestamp": utc_now(), "event": event, **data}
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n")

    @staticmethod
    def _atomic_write_json(path: Path, value: dict[str, Any]) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
        try:
            with open(temporary, "w", encoding="utf-8") as handle:
                json.dump(value, handle, ensure_ascii=False, indent=2)
                handle.write("\n")
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, path)
        finally:
            try:
                temporary.unlink()
            except FileNotFoundError:
                pass
