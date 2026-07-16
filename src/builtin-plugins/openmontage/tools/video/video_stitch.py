"""Video stitch tool wrapping FFmpeg.

Multi-clip assembly with validation, transitions, and spatial layouts.
Supports sequential concatenation (TikTok-style stitch), crossfade/fade
transitions, and spatial compositions (side-by-side, vertical stack,
picture-in-picture) for duet-style content.
"""

from __future__ import annotations

import hashlib
import json
import re
import time
from pathlib import Path
from typing import Any, Optional

from tools.base_tool import (
    BaseTool,
    Determinism,
    ExecutionMode,
    ResourceProfile,
    RetryPolicy,
    ResumeSupport,
    ToolResult,
    ToolStability,
    ToolTier,
)


class VideoStitch(BaseTool):
    name = "video_stitch"
    version = "0.2.0"
    tier = ToolTier.CORE
    capability = "video_post"
    provider = "ffmpeg"
    stability = ToolStability.EXPERIMENTAL
    execution_mode = ExecutionMode.SYNC
    determinism = Determinism.DETERMINISTIC

    dependencies = ["cmd:ffmpeg", "cmd:ffprobe"]
    install_instructions = "商媒运营助手安装包已内置 FFmpeg；若运行时缺失，请重新安装完整应用。"
    agent_skills = ["ffmpeg", "video-toolkit"]

    capabilities = [
        "validate_clips",
        "stitch",
        "crossfade",
        "fade_through_black",
        "preview_stitch",
        "repeated_intro_qa",
        "spatial_side_by_side",
        "spatial_vertical_stack",
        "spatial_picture_in_picture",
    ]

    input_schema = {
        "type": "object",
        "required": ["operation"],
        "properties": {
            "operation": {
                "type": "string",
                "enum": ["validate", "stitch", "preview_stitch", "spatial"],
            },
            "clips": {
                "type": "array",
                "items": {"type": "string"},
                "description": "按成片顺序排列的输入视频文件路径列表。",
            },
            "output_path": {"type": "string"},
            "project_id": {
                "type": "string",
                "description": "本次视频任务的稳定项目 ID，必须与首图和各分段生成使用同一个值。",
            },
            "title": {
                "type": "string",
                "description": "最终成片在商媒运营助手素材库中的中文标题。",
            },
            "transition": {
                "type": "string",
                "enum": ["cut", "crossfade", "fade"],
                "default": "cut",
                "description": "转场类型。若内置 FFmpeg 不支持 xfade/acrossfade，crossfade 和 fade 会自动降级为稳定的 cut。",
            },
            "transition_duration": {
                "type": "number",
                "minimum": 0.1,
                "maximum": 5.0,
                "default": 0.5,
                "description": "转场时长，单位为秒。",
            },
            "continuity_check": {
                "type": "string",
                "enum": ["strict", "warn", "off"],
                "default": "strict",
                "description": "检测相邻片段是否从同一开场重新播放。strict 会阻止有明显重复开场的成片；warn 仅报告；off 关闭检测。",
            },
            "auto_normalize": {
                "type": "boolean",
                "default": False,
                "description": "片段规格不一致时，是否先转码到统一格式再拼接。",
            },
            "target_resolution": {
                "type": "string",
                "description": "规范化目标分辨率，例如 1920x1080。",
            },
            "target_fps": {
                "type": "integer",
                "description": "规范化目标帧率。",
            },
            "codec": {"type": "string", "default": "libx264"},
            "crf": {"type": "integer", "default": 23},
            "preset": {"type": "string", "default": "medium"},
            "profile": {
                "type": "string",
                "description": "media_profiles.py 中定义的媒体配置名称。",
            },
            "layout": {
                "type": "string",
                "enum": ["side_by_side", "vertical_stack", "picture_in_picture"],
                "description": "空间合成操作使用的布局。",
            },
            "pip_position": {
                "type": "string",
                "enum": ["top_left", "top_right", "bottom_left", "bottom_right"],
                "default": "bottom_right",
                "description": "画中画叠层位置。",
            },
            "pip_scale": {
                "type": "number",
                "minimum": 0.1,
                "maximum": 0.5,
                "default": 0.3,
                "description": "画中画相对底层视频的缩放比例。",
            },
            "pip_margin": {
                "type": "integer",
                "default": 10,
                "description": "画中画距离边缘的像素间距。",
            },
            "dry_run": {
                "type": "boolean",
                "default": False,
                "description": "为 true 时仅返回执行计划，不实际处理文件。",
            },
        },
    }

    resource_profile = ResourceProfile(
        cpu_cores=4, ram_mb=2048, vram_mb=0, disk_mb=5000, network_required=False
    )
    retry_policy = RetryPolicy(max_retries=1, retryable_errors=["Conversion failed"])
    resume_support = ResumeSupport.FROM_START
    idempotency_key_fields = ["operation", "clips", "transition", "layout"]
    side_effects = ["把最终成片写入 output_path"]
    user_visible_verification = [
        "播放最终成片，检查片段顺序、转场和音画同步",
    ]

    def execute(self, inputs: dict[str, Any]) -> ToolResult:
        operation = inputs["operation"]
        start = time.time()

        if inputs.get("dry_run"):
            return ToolResult(
                success=True,
                data=self.dry_run(inputs),
            )

        try:
            if operation == "validate":
                result = self._validate(inputs)
            elif operation == "stitch":
                result = self._stitch(inputs)
            elif operation == "preview_stitch":
                result = self._preview_stitch(inputs)
            elif operation == "spatial":
                result = self._spatial(inputs)
            else:
                return ToolResult(success=False, error=f"Unknown operation: {operation}")
        except Exception as e:
            return ToolResult(success=False, error=str(e))

        result.duration_seconds = round(time.time() - start, 2)
        return result

    def dry_run(self, inputs: dict[str, Any]) -> dict[str, Any]:
        """Preflight check: validate clips and report what would happen."""
        clips = inputs.get("clips", [])
        operation = inputs.get("operation", "stitch")
        info = {
            "tool": self.name,
            "operation": operation,
            "clip_count": len(clips),
            "transition": inputs.get("transition", "cut"),
            "auto_normalize": inputs.get("auto_normalize", False),
            "estimated_cost_usd": self.estimate_cost(inputs),
            "estimated_runtime_seconds": self.estimate_runtime(inputs),
            "status": self.get_status().value,
            "would_execute": True,
        }
        if clips:
            probe_results = []
            for clip in clips:
                if Path(clip).exists():
                    probe = self._probe_clip(clip)
                    if probe:
                        probe_results.append(probe)
            info["clip_info"] = probe_results
        return info

    # ------------------------------------------------------------------
    # Audio-stream detection and silent-audio helpers
    # ------------------------------------------------------------------

    def _clip_has_audio(self, clip_path: str) -> bool:
        """Return True if *clip_path* contains at least one audio stream."""
        cmd = [
            "ffprobe", "-v", "quiet",
            "-select_streams", "a",
            "-show_entries", "stream=codec_type",
            "-of", "json",
            str(clip_path),
        ]
        try:
            proc = self.run_command(cmd)
            data = json.loads(proc.stdout)
            return len(data.get("streams", [])) > 0
        except Exception:
            return False

    def _ensure_audio_for_clips(
        self,
        clips: list[str],
        temp_dir: Path,
        temp_files: list[Path],
    ) -> list[str]:
        """Return a list of clip paths where every clip is guaranteed to have
        an audio stream.  Clips that already contain audio are returned as-is.
        For clips without audio, a silent stereo AAC track is muxed in and the
        path to the new file is returned instead.  All generated temp files are
        appended to *temp_files* so the caller can clean them up.
        """
        result: list[str] = []
        for i, clip in enumerate(clips):
            if self._clip_has_audio(clip):
                result.append(clip)
            else:
                augmented = temp_dir / f"audio_aug_{i:04d}.mp4"
                cmd = [
                    "ffmpeg", "-y",
                    "-i", str(clip),
                    "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo",
                    "-c:v", "copy",
                    "-c:a", "aac",
                    "-shortest",
                    str(augmented),
                ]
                self.run_command(cmd)
                temp_files.append(augmented)
                result.append(str(augmented))
        return result

    # ------------------------------------------------------------------
    # Probe helper
    # ------------------------------------------------------------------

    def _probe_clip(self, clip_path: str) -> Optional[dict[str, Any]]:
        """Probe a single clip with ffprobe and return metadata dict."""
        cmd = [
            "ffprobe", "-v", "quiet",
            "-print_format", "json",
            "-show_streams",
            "-show_format",
            str(clip_path),
        ]
        try:
            proc = self.run_command(cmd)
            data = json.loads(proc.stdout)
        except Exception:
            return None

        info: dict[str, Any] = {"path": str(clip_path)}

        # Extract video stream info
        for stream in data.get("streams", []):
            if stream.get("codec_type") == "video":
                info["width"] = stream.get("width")
                info["height"] = stream.get("height")
                info["video_codec"] = stream.get("codec_name")
                info["pixel_format"] = stream.get("pix_fmt")
                # Parse fps from r_frame_rate (e.g. "30/1")
                rfr = stream.get("r_frame_rate", "0/1")
                try:
                    num, den = rfr.split("/")
                    info["fps"] = round(int(num) / int(den), 2)
                except (ValueError, ZeroDivisionError):
                    info["fps"] = None
                break

        # Extract audio stream info
        for stream in data.get("streams", []):
            if stream.get("codec_type") == "audio":
                info["audio_codec"] = stream.get("codec_name")
                info["sample_rate"] = stream.get("sample_rate")
                info["audio_channels"] = stream.get("channels")
                break

        # Duration from format
        fmt = data.get("format", {})
        try:
            info["duration"] = float(fmt.get("duration", 0))
        except (TypeError, ValueError):
            info["duration"] = 0.0
        try:
            info["file_size_bytes"] = int(fmt.get("size", 0))
        except (TypeError, ValueError):
            info["file_size_bytes"] = 0

        return info

    # ------------------------------------------------------------------
    # validate
    # ------------------------------------------------------------------

    def _validate(self, inputs: dict[str, Any]) -> ToolResult:
        """Check clip compatibility: resolution, fps, codec, audio format.

        Returns a detailed report of mismatches.
        """
        clips = inputs.get("clips", [])
        if not clips:
            return ToolResult(success=False, error="No clips provided")

        # Probe all clips
        probes: list[dict[str, Any]] = []
        missing: list[str] = []
        probe_errors: list[str] = []

        for clip in clips:
            if not Path(clip).exists():
                missing.append(clip)
                continue
            info = self._probe_clip(clip)
            if info is None:
                probe_errors.append(clip)
            else:
                probes.append(info)

        if missing:
            return ToolResult(
                success=False,
                error=f"Clips not found: {', '.join(missing)}",
            )
        if probe_errors:
            return ToolResult(
                success=False,
                error=f"Failed to probe clips: {', '.join(probe_errors)}",
            )

        exact_duplicates = self._find_exact_duplicate_clips(clips)
        repeated_intros = self._detect_repeated_intros(clips, probes)

        # Compare properties across clips
        mismatches: list[dict[str, Any]] = []
        reference = probes[0]
        check_fields = [
            ("width", "resolution width"),
            ("height", "resolution height"),
            ("fps", "frame rate"),
            ("video_codec", "video codec"),
            ("pixel_format", "pixel format"),
            ("audio_codec", "audio codec"),
            ("sample_rate", "audio sample rate"),
            ("audio_channels", "audio channels"),
        ]

        for i, probe in enumerate(probes[1:], start=1):
            clip_mismatches: list[str] = []
            for field_key, label in check_fields:
                ref_val = reference.get(field_key)
                cur_val = probe.get(field_key)
                if ref_val is not None and cur_val is not None and ref_val != cur_val:
                    clip_mismatches.append(
                        f"{label}: clip[0]={ref_val} vs clip[{i}]={cur_val}"
                    )
            if clip_mismatches:
                mismatches.append({
                    "clip_index": i,
                    "clip_path": probe["path"],
                    "differences": clip_mismatches,
                })

        compatible = len(mismatches) == 0 and not exact_duplicates
        total_duration = sum(p.get("duration", 0) for p in probes)

        return ToolResult(
            success=True,
            data={
                "operation": "validate",
                "clip_count": len(clips),
                "compatible": compatible,
                "total_duration": round(total_duration, 2),
                "reference_clip": {
                    "path": reference["path"],
                    "resolution": f"{reference.get('width')}x{reference.get('height')}",
                    "fps": reference.get("fps"),
                    "video_codec": reference.get("video_codec"),
                    "audio_codec": reference.get("audio_codec"),
                },
                "mismatches": mismatches,
                "exact_duplicates": exact_duplicates,
                "repeated_intros": repeated_intros,
                "clips": probes,
            },
        )

    @staticmethod
    def _file_sha256(path: str) -> str:
        digest = hashlib.sha256()
        with open(path, "rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        return digest.hexdigest()

    def _find_exact_duplicate_clips(self, clips: list[str]) -> list[dict[str, Any]]:
        resolved_seen: dict[str, int] = {}
        digest_seen: dict[str, int] = {}
        duplicates: list[dict[str, Any]] = []

        for index, clip in enumerate(clips):
            resolved = str(Path(clip).expanduser().resolve())
            if resolved in resolved_seen:
                duplicates.append({
                    "kind": "same_path",
                    "first_index": resolved_seen[resolved],
                    "duplicate_index": index,
                    "path": resolved,
                })
                continue
            resolved_seen[resolved] = index

            digest = self._file_sha256(resolved)
            if digest in digest_seen:
                duplicates.append({
                    "kind": "same_content",
                    "first_index": digest_seen[digest],
                    "duplicate_index": index,
                    "path": resolved,
                    "sha256": digest,
                })
                continue
            digest_seen[digest] = index

        return duplicates

    def _measure_ssim(
        self,
        left_path: str,
        right_path: str,
        *,
        left_start: float,
        right_start: float,
        duration: float,
    ) -> Optional[float]:
        filter_graph = (
            "[0:v]fps=4,scale=96:96:flags=fast_bilinear,format=yuv420p,"
            "setpts=PTS-STARTPTS[left];"
            "[1:v]fps=4,scale=96:96:flags=fast_bilinear,format=yuv420p,"
            "setpts=PTS-STARTPTS[right];"
            "[left][right]ssim"
        )
        cmd = [
            "ffmpeg", "-hide_banner", "-nostdin",
            "-ss", f"{left_start:.3f}", "-t", f"{duration:.3f}", "-i", left_path,
            "-ss", f"{right_start:.3f}", "-t", f"{duration:.3f}", "-i", right_path,
            "-filter_complex", filter_graph,
            "-an", "-f", "null", "-",
        ]
        try:
            proc = self.run_command(cmd, timeout=60)
        except Exception:
            return None
        matches = re.findall(r"\bAll:([0-9]+(?:\.[0-9]+)?)", f"{proc.stdout}\n{proc.stderr}")
        return float(matches[-1]) if matches else None

    def _detect_repeated_intros(
        self,
        clips: list[str],
        probes: list[dict[str, Any]],
    ) -> list[dict[str, Any]]:
        if len(clips) < 2 or not self._ffmpeg_has_filter("ssim"):
            return []

        repeated: list[dict[str, Any]] = []
        for index in range(1, len(clips)):
            previous_duration = float(probes[index - 1].get("duration") or 0)
            current_duration = float(probes[index].get("duration") or 0)
            sample_duration = min(3.0, previous_duration, current_duration)
            if sample_duration < 1.0:
                continue

            start_similarity = self._measure_ssim(
                clips[index - 1],
                clips[index],
                left_start=0,
                right_start=0,
                duration=sample_duration,
            )
            continuity_similarity = self._measure_ssim(
                clips[index - 1],
                clips[index],
                left_start=max(0.0, previous_duration - sample_duration),
                right_start=0,
                duration=sample_duration,
            )
            if start_similarity is None or continuity_similarity is None:
                continue

            if start_similarity >= 0.79 and start_similarity - continuity_similarity >= 0.18:
                repeated.append({
                    "previous_clip_index": index - 1,
                    "current_clip_index": index,
                    "start_similarity": round(start_similarity, 4),
                    "continuity_similarity": round(continuity_similarity, 4),
                    "sample_duration_seconds": round(sample_duration, 2),
                    "reason": "当前片段开场更像上一片段的开场，而不是上一片段的结尾",
                })

        return repeated

    # ------------------------------------------------------------------
    # Normalization helper
    # ------------------------------------------------------------------

    def _ffmpeg_has_filter(self, name: str) -> bool:
        cache = getattr(self, "_ffmpeg_filter_cache", None)
        if cache is None:
            cache = {}
            self._ffmpeg_filter_cache = cache
        if name in cache:
            return cache[name]
        try:
            proc = self.run_command(["ffmpeg", "-hide_banner", "-filters"])
            output = f"{proc.stdout}\n{proc.stderr}"
            available = any(
                len(parts) > 1 and parts[1] == name
                for parts in (line.split() for line in output.splitlines())
            )
        except Exception:
            available = False
        cache[name] = available
        return available

    def _resolve_normalization_target(
        self, inputs: dict[str, Any], probes: list[dict[str, Any]]
    ) -> tuple[int, int, int, str, str]:
        """Determine the target resolution, fps, and codecs for normalization.

        Returns (width, height, fps, video_codec, audio_codec).
        """
        # If a media profile is specified, use it
        profile_name = inputs.get("profile")
        if profile_name:
            try:
                from lib.media_profiles import get_profile
                profile = get_profile(profile_name)
                return (profile.width, profile.height, profile.fps, profile.codec, profile.audio_codec)
            except (ImportError, ValueError):
                pass

        # Explicit target overrides
        target_w, target_h = None, None
        if inputs.get("target_resolution"):
            parts = inputs["target_resolution"].split("x")
            if len(parts) == 2:
                target_w, target_h = int(parts[0]), int(parts[1])

        target_fps = inputs.get("target_fps")

        # Fall back to first clip as reference
        ref = probes[0] if probes else {}
        width = target_w or ref.get("width", 1920)
        height = target_h or ref.get("height", 1080)
        fps = target_fps or ref.get("fps", 30)
        video_codec = inputs.get("codec", "libx264")
        audio_codec = "aac"

        return (width, height, int(fps), video_codec, audio_codec)

    def _normalize_clip(
        self,
        clip_path: str,
        output_path: Path,
        width: int,
        height: int,
        fps: int,
        video_codec: str,
        audio_codec: str,
        crf: int,
        preset: str,
    ) -> None:
        """Re-encode a clip to the target format."""
        if self._ffmpeg_has_filter("pad"):
            video_filter = (
                f"scale={width}:{height}:force_original_aspect_ratio=decrease,"
                f"pad={width}:{height}:(ow-iw)/2:(oh-ih)/2"
            )
        else:
            video_filter = f"scale={width}:{height}"
        cmd = [
            "ffmpeg", "-y",
            "-i", str(clip_path),
            "-vf", video_filter,
            "-r", str(fps),
            "-c:v", video_codec, "-crf", str(crf), "-preset", preset,
            "-c:a", audio_codec, "-ar", "44100", "-ac", "2",
            "-pix_fmt", "yuv420p",
            str(output_path),
        ]
        self.run_command(cmd)

    def _needs_normalization(self, probes: list[dict[str, Any]]) -> bool:
        """Check whether clips need normalization to be concat-compatible."""
        if len(probes) < 2:
            return False
        ref = probes[0]
        for probe in probes[1:]:
            for key in ("width", "height", "fps", "video_codec", "audio_codec", "sample_rate"):
                if ref.get(key) != probe.get(key) and ref.get(key) is not None:
                    return True
        return False

    # ------------------------------------------------------------------
    # stitch
    # ------------------------------------------------------------------

    def _stitch(self, inputs: dict[str, Any]) -> ToolResult:
        """Concatenate clips sequentially with FFmpeg concat demuxer.

        Supports transitions: cut (default), crossfade, fade-through-black.
        """
        clips = [str(clip) for clip in (inputs.get("clips", []) or []) if str(clip).strip()]
        if not clips:
            return ToolResult(success=False, error="未提供视频片段")
        if len(clips) < 2:
            return ToolResult(success=False, error="视频拼接至少需要 2 个片段")

        output_path = Path(inputs.get("output_path", "stitched_output.mp4"))
        output_path.parent.mkdir(parents=True, exist_ok=True)
        requested_transition = inputs.get("transition", "cut")
        transition = requested_transition
        transition_dur = inputs.get("transition_duration", 0.5)
        transition_fallback_reason = None
        if transition in ("crossfade", "fade") and not (
            self._ffmpeg_has_filter("xfade") and self._ffmpeg_has_filter("acrossfade")
        ):
            transition = "cut"
            transition_dur = 0
            transition_fallback_reason = (
                "内置 FFmpeg 不支持 xfade/acrossfade，已自动改用稳定的直接切换转场。"
            )
        auto_normalize = inputs.get("auto_normalize", False)
        codec = inputs.get("codec", "libx264")
        crf = inputs.get("crf", 23)
        preset = inputs.get("preset", "medium")

        # Verify all clips exist
        for clip in clips:
            if not Path(clip).exists():
                return ToolResult(success=False, error=f"视频片段不存在：{clip}")

        exact_duplicates = self._find_exact_duplicate_clips(clips)
        if exact_duplicates:
            duplicate = exact_duplicates[0]
            return ToolResult(
                success=False,
                error=(
                    f"检测到重复视频片段：第 {duplicate['first_index'] + 1} 段与第 "
                    f"{duplicate['duplicate_index'] + 1} 段是同一路径或完全相同的文件。"
                    "请移除重复项后重新拼接。"
                ),
                data={"exact_duplicates": exact_duplicates},
            )

        # Probe clips for compatibility check
        probes: list[dict[str, Any]] = []
        for clip in clips:
            info = self._probe_clip(clip)
            if info is None:
                return ToolResult(success=False, error=f"无法读取视频片段信息：{clip}")
            probes.append(info)

        continuity_check = str(inputs.get("continuity_check") or "strict").strip().lower()
        repeated_intros = (
            self._detect_repeated_intros(clips, probes)
            if continuity_check != "off"
            else []
        )
        if repeated_intros and continuity_check == "strict":
            repeated = repeated_intros[0]
            return ToolResult(
                success=False,
                error=(
                    f"检测到第 {repeated['current_clip_index'] + 1} 段重复开场：它的前三秒更像"
                    f"第 {repeated['previous_clip_index'] + 1} 段开头，而不是上一段结尾。"
                    "请用 seedance_video 的 continuation 模式重新生成该段，并把上一段素材库路径传给 first_clip；"
                    "不要再次使用原始首图。"
                ),
                data={"repeated_intros": repeated_intros},
            )

        needs_norm = self._needs_normalization(probes)
        target_requires_norm = False
        if auto_normalize and inputs.get("target_resolution"):
            parts = str(inputs["target_resolution"]).split("x")
            if len(parts) == 2 and all(part.isdigit() for part in parts):
                target_width, target_height = int(parts[0]), int(parts[1])
                target_requires_norm = any(
                    probe.get("width") != target_width or probe.get("height") != target_height
                    for probe in probes
                )
        if auto_normalize and inputs.get("target_fps"):
            target_fps = float(inputs["target_fps"])
            target_requires_norm = target_requires_norm or any(
                float(probe.get("fps") or 0) != target_fps for probe in probes
            )

        # If clips are incompatible and auto_normalize is off, fail with advice
        if needs_norm and not auto_normalize and transition == "cut":
            return ToolResult(
                success=False,
                error=(
                    "视频片段的分辨率、帧率或编码不一致。请设置 auto_normalize=true "
                    "统一转码，或改用非 cut 转场。"
                ),
            )

        temp_dir = output_path.parent / ".stitch_tmp"
        temp_dir.mkdir(parents=True, exist_ok=True)
        temp_files: list[Path] = []

        try:
            # Normalize clips if needed
            working_clips: list[str] = []
            normalize_clips = needs_norm or target_requires_norm or transition != "cut"
            if normalize_clips:
                width, height, fps, vid_codec, aud_codec = self._resolve_normalization_target(inputs, probes)
                for i, clip in enumerate(clips):
                    norm_path = temp_dir / f"norm_{i:04d}.mp4"
                    self._normalize_clip(clip, norm_path, width, height, fps, vid_codec, aud_codec, crf, preset)
                    working_clips.append(str(norm_path))
                    temp_files.append(norm_path)
            else:
                working_clips = list(clips)

            # For crossfade/fade transitions, ensure every clip has an audio
            # stream so that the acrossfade filter does not fail.  Image-derived
            # video clips typically lack audio; we add a silent track for those.
            if transition in ("crossfade", "fade"):
                working_clips = self._ensure_audio_for_clips(
                    working_clips, temp_dir, temp_files,
                )

            if transition == "cut":
                result_data = self._stitch_cut(working_clips, output_path, temp_dir, temp_files)
            elif transition == "crossfade":
                result_data = self._stitch_crossfade(working_clips, output_path, transition_dur, probes)
            elif transition == "fade":
                result_data = self._stitch_fade_through_black(working_clips, output_path, transition_dur, probes)
            else:
                return ToolResult(success=False, error=f"未知转场类型：{transition}")

            # Get output file info
            file_size = output_path.stat().st_size if output_path.exists() else 0
            out_probe = self._probe_clip(str(output_path))
            out_duration = out_probe.get("duration", 0) if out_probe else 0

            return ToolResult(
                success=True,
                data={
                    "operation": "stitch",
                    "clip_count": len(clips),
                    "transition": transition,
                    "requested_transition": requested_transition,
                    "transition_duration": transition_dur if transition != "cut" else 0,
                    "transition_fallback_reason": transition_fallback_reason,
                    "continuity_check": continuity_check,
                    "repeated_intros": repeated_intros,
                    "auto_normalized": normalize_clips,
                    "output": str(output_path),
                    "duration": round(out_duration, 2),
                    "file_size_bytes": file_size,
                    "delivery_role": "final_video",
                    "chat_visibility": "final_only",
                    **result_data,
                },
                artifacts=[str(output_path)],
            )
        finally:
            self._cleanup_temp(temp_dir, temp_files)

    def _stitch_cut(
        self,
        clips: list[str],
        output_path: Path,
        temp_dir: Path,
        temp_files: list[Path],
    ) -> dict[str, Any]:
        """Simple concat via FFmpeg concat demuxer (no transition)."""
        concat_list = temp_dir / "concat_list.txt"
        temp_files.append(concat_list)
        with open(concat_list, "w", encoding="utf-8") as f:
            for clip in clips:
                safe_path = str(Path(clip).resolve()).replace("\\", "/")
                f.write(f"file '{safe_path}'\n")

        cmd = [
            "ffmpeg", "-y",
            "-f", "concat", "-safe", "0",
            "-i", str(concat_list),
            "-c", "copy",
            str(output_path),
        ]
        self.run_command(cmd)
        return {"method": "concat_demuxer"}

    def _stitch_crossfade(
        self,
        clips: list[str],
        output_path: Path,
        duration: float,
        probes: list[dict[str, Any]],
    ) -> dict[str, Any]:
        """Crossfade between adjacent clips using xfade filter."""
        if len(clips) == 2:
            # Simple two-clip crossfade
            cmd = [
                "ffmpeg", "-y",
                "-i", clips[0],
                "-i", clips[1],
                "-filter_complex",
                f"[0:v][1:v]xfade=transition=fade:duration={duration}:offset={self._get_xfade_offset(probes, 0, duration)}[v];"
                f"[0:a][1:a]acrossfade=d={duration}[a]",
                "-map", "[v]", "-map", "[a]",
                str(output_path),
            ]
            self.run_command(cmd)
        else:
            # Chain crossfades for N clips
            self._chain_xfade(clips, output_path, duration, probes, transition="fade")
        return {"method": "xfade_crossfade"}

    def _stitch_fade_through_black(
        self,
        clips: list[str],
        output_path: Path,
        duration: float,
        probes: list[dict[str, Any]],
    ) -> dict[str, Any]:
        """Fade-through-black between adjacent clips using xfade fadeblack."""
        if len(clips) == 2:
            cmd = [
                "ffmpeg", "-y",
                "-i", clips[0],
                "-i", clips[1],
                "-filter_complex",
                f"[0:v][1:v]xfade=transition=fadeblack:duration={duration}:offset={self._get_xfade_offset(probes, 0, duration)}[v];"
                f"[0:a][1:a]acrossfade=d={duration}[a]",
                "-map", "[v]", "-map", "[a]",
                str(output_path),
            ]
            self.run_command(cmd)
        else:
            self._chain_xfade(clips, output_path, duration, probes, transition="fadeblack")
        return {"method": "xfade_fadeblack"}

    def _get_xfade_offset(
        self, probes: list[dict[str, Any]], clip_index: int, duration: float
    ) -> float:
        """Calculate xfade offset for a given clip pair.

        The offset is the timestamp in the output where the transition starts,
        which equals the duration of the first clip minus the transition duration.
        """
        clip_dur = probes[clip_index].get("duration", 0) if clip_index < len(probes) else 0
        offset = max(0, clip_dur - duration)
        return round(offset, 3)

    def _chain_xfade(
        self,
        clips: list[str],
        output_path: Path,
        duration: float,
        probes: list[dict[str, Any]],
        transition: str,
    ) -> None:
        """Chain xfade filters for N > 2 clips.

        Builds a complex filtergraph that progressively applies xfade
        between each adjacent pair of clips.
        """
        n = len(clips)
        input_args: list[str] = []
        for clip in clips:
            input_args.extend(["-i", clip])

        # Calculate cumulative offsets
        # Each xfade offset = cumulative duration of all previous segments
        # minus cumulative transition overlaps minus current transition duration
        video_filters: list[str] = []
        audio_filters: list[str] = []
        cumulative_offset = 0.0

        for i in range(n - 1):
            clip_dur = probes[i].get("duration", 0) if i < len(probes) else 0
            offset = round(cumulative_offset + clip_dur - duration, 3)
            offset = max(0, offset)

            if i == 0:
                v_in1 = "[0:v]"
                a_in1 = "[0:a]"
            else:
                v_in1 = f"[vfade{i-1}]"
                a_in1 = f"[afade{i-1}]"

            v_in2 = f"[{i+1}:v]"
            a_in2 = f"[{i+1}:a]"

            if i < n - 2:
                v_out = f"[vfade{i}]"
                a_out = f"[afade{i}]"
            else:
                v_out = "[vout]"
                a_out = "[aout]"

            video_filters.append(
                f"{v_in1}{v_in2}xfade=transition={transition}:duration={duration}:offset={offset}{v_out}"
            )
            audio_filters.append(
                f"{a_in1}{a_in2}acrossfade=d={duration}{a_out}"
            )

            # Cumulative offset advances by clip duration minus overlap
            cumulative_offset = offset

        filter_complex = ";".join(video_filters + audio_filters)

        cmd = ["ffmpeg", "-y"]
        cmd.extend(input_args)
        cmd.extend(["-filter_complex", filter_complex])
        cmd.extend(["-map", "[vout]", "-map", "[aout]"])
        cmd.append(str(output_path))
        self.run_command(cmd)

    # ------------------------------------------------------------------
    # preview_stitch
    # ------------------------------------------------------------------

    def _preview_stitch(self, inputs: dict[str, Any]) -> ToolResult:
        """Generate a low-resolution preview of the stitched result."""
        clips = inputs.get("clips", [])
        if not clips:
            return ToolResult(success=False, error="No clips provided")
        if len(clips) < 2:
            return ToolResult(success=False, error="At least 2 clips required for preview")

        output_path = Path(inputs.get("output_path", "stitch_preview.mp4"))
        output_path.parent.mkdir(parents=True, exist_ok=True)

        # Verify all clips exist
        for clip in clips:
            if not Path(clip).exists():
                return ToolResult(success=False, error=f"Clip not found: {clip}")

        # Build preview by normalizing to low-res and stitching
        preview_inputs = dict(inputs)
        preview_inputs["auto_normalize"] = True
        preview_inputs["target_resolution"] = "640x360"
        preview_inputs["target_fps"] = 24
        preview_inputs["crf"] = 30
        preview_inputs["preset"] = "ultrafast"
        preview_inputs["output_path"] = str(output_path)

        # Delegate to _stitch with preview settings
        result = self._stitch(preview_inputs)

        if result.success:
            result.data["operation"] = "preview_stitch"
            result.data["preview"] = True
            result.data["preview_resolution"] = "640x360"

        return result

    # ------------------------------------------------------------------
    # spatial
    # ------------------------------------------------------------------

    def _spatial(self, inputs: dict[str, Any]) -> ToolResult:
        """Side-by-side, vertical stack, or picture-in-picture layouts.

        Designed for TikTok Stitch/Duet style compositions (D3.5.8).
        """
        clips = inputs.get("clips", [])
        if not clips or len(clips) < 2:
            return ToolResult(
                success=False,
                error="At least 2 clips required for spatial layout",
            )

        layout = inputs.get("layout")
        if not layout:
            return ToolResult(success=False, error="layout is required for spatial operation")

        output_path = Path(inputs.get("output_path", "spatial_output.mp4"))
        output_path.parent.mkdir(parents=True, exist_ok=True)
        codec = inputs.get("codec", "libx264")
        crf = inputs.get("crf", 23)

        # Verify all clips exist
        for clip in clips:
            if not Path(clip).exists():
                return ToolResult(success=False, error=f"Clip not found: {clip}")

        temp_dir = output_path.parent / ".spatial_tmp"
        temp_dir.mkdir(parents=True, exist_ok=True)
        temp_files: list[Path] = []

        try:
            # side_by_side and vertical_stack use amix which requires audio
            # on both inputs.  Ensure silent tracks for audio-less clips.
            working_clips = list(clips)
            if layout in ("side_by_side", "vertical_stack"):
                working_clips = self._ensure_audio_for_clips(
                    working_clips, temp_dir, temp_files,
                )

            if layout == "side_by_side":
                self._spatial_side_by_side(working_clips, output_path, codec, crf)
            elif layout == "vertical_stack":
                self._spatial_vertical_stack(working_clips, output_path, codec, crf)
            elif layout == "picture_in_picture":
                self._spatial_pip(working_clips, output_path, inputs, codec, crf)
            else:
                return ToolResult(success=False, error=f"Unknown layout: {layout}")
        except Exception as e:
            return ToolResult(success=False, error=str(e))
        finally:
            self._cleanup_temp(temp_dir, temp_files)

        file_size = output_path.stat().st_size if output_path.exists() else 0
        out_probe = self._probe_clip(str(output_path))
        out_duration = out_probe.get("duration", 0) if out_probe else 0

        return ToolResult(
            success=True,
            data={
                "operation": "spatial",
                "layout": layout,
                "clip_count": len(clips),
                "output": str(output_path),
                "duration": round(out_duration, 2),
                "file_size_bytes": file_size,
            },
            artifacts=[str(output_path)],
        )

    def _spatial_side_by_side(
        self, clips: list[str], output_path: Path, codec: str, crf: int
    ) -> None:
        """Place clips side by side (horizontal split).

        Both clips are scaled to the same height and placed left-right.
        Uses the first two clips; additional clips are ignored.
        """
        input_args = ["-i", clips[0], "-i", clips[1]]
        filter_complex = (
            "[0:v]scale=-2:480[left];"
            "[1:v]scale=-2:480[right];"
            "[left][right]hstack=inputs=2[v];"
            "[0:a][1:a]amix=inputs=2:duration=shortest[a]"
        )
        cmd = ["ffmpeg", "-y"]
        cmd.extend(input_args)
        cmd.extend([
            "-filter_complex", filter_complex,
            "-map", "[v]", "-map", "[a]",
            "-c:v", codec, "-crf", str(crf),
            "-c:a", "aac",
            "-shortest",
            str(output_path),
        ])
        self.run_command(cmd)

    def _spatial_vertical_stack(
        self, clips: list[str], output_path: Path, codec: str, crf: int
    ) -> None:
        """Place clips in a vertical stack (top-bottom).

        Both clips are scaled to the same width and stacked vertically.
        Ideal for portrait/mobile viewing.
        """
        input_args = ["-i", clips[0], "-i", clips[1]]
        filter_complex = (
            "[0:v]scale=540:-2[top];"
            "[1:v]scale=540:-2[bottom];"
            "[top][bottom]vstack=inputs=2[v];"
            "[0:a][1:a]amix=inputs=2:duration=shortest[a]"
        )
        cmd = ["ffmpeg", "-y"]
        cmd.extend(input_args)
        cmd.extend([
            "-filter_complex", filter_complex,
            "-map", "[v]", "-map", "[a]",
            "-c:v", codec, "-crf", str(crf),
            "-c:a", "aac",
            "-shortest",
            str(output_path),
        ])
        self.run_command(cmd)

    def _spatial_pip(
        self,
        clips: list[str],
        output_path: Path,
        inputs: dict[str, Any],
        codec: str,
        crf: int,
    ) -> None:
        """Picture-in-picture: overlay second clip on first.

        clips[0] is the base (full-screen), clips[1] is the PiP overlay.
        """
        pip_position = inputs.get("pip_position", "bottom_right")
        pip_scale = inputs.get("pip_scale", 0.3)
        pip_margin = inputs.get("pip_margin", 10)

        # Build position expression based on corner
        position_map = {
            "top_left": f"{pip_margin}:{pip_margin}",
            "top_right": f"main_w-overlay_w-{pip_margin}:{pip_margin}",
            "bottom_left": f"{pip_margin}:main_h-overlay_h-{pip_margin}",
            "bottom_right": f"main_w-overlay_w-{pip_margin}:main_h-overlay_h-{pip_margin}",
        }
        position = position_map.get(pip_position, position_map["bottom_right"])

        input_args = ["-i", clips[0], "-i", clips[1]]
        filter_complex = (
            f"[1:v]scale=iw*{pip_scale}:ih*{pip_scale}[pip];"
            f"[0:v][pip]overlay={position}:shortest=1[v]"
        )
        cmd = ["ffmpeg", "-y"]
        cmd.extend(input_args)
        cmd.extend([
            "-filter_complex", filter_complex,
            "-map", "[v]", "-map", "0:a?",
            "-c:v", codec, "-crf", str(crf),
            "-c:a", "aac",
            "-shortest",
            str(output_path),
        ])
        self.run_command(cmd)

    # ------------------------------------------------------------------
    # Cleanup
    # ------------------------------------------------------------------

    @staticmethod
    def _cleanup_temp(temp_dir: Path, temp_files: list[Path]) -> None:
        """Remove temporary files and directory."""
        for f in temp_files:
            if f.exists():
                try:
                    f.unlink()
                except OSError:
                    pass
        if temp_dir.exists():
            try:
                temp_dir.rmdir()
            except OSError:
                pass
