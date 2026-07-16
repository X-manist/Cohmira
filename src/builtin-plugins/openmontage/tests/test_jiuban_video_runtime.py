from pathlib import Path
from subprocess import CompletedProcess

import pytest

from mcp.src.openmontage_runner import (
    command_list_tools,
    promote_registered_media_outputs,
    read_media_catalog,
    register_result_media_assets,
)
from tools.video.seedance_video import SeedanceVideo
from tools.video.video_stitch import VideoStitch


def test_video_capability_alias_lists_generation_and_post_tools():
    result = command_list_tools({"capability": "video", "includeSchemas": False})
    names = {tool["name"] for tool in result["tools"]}

    assert "seedance_video" in names
    assert "video_stitch" in names


def test_seedance_schema_exposes_single_clip_duration_limit():
    duration = SeedanceVideo.input_schema["properties"]["duration"]

    assert duration["type"] == "integer"
    assert duration["maximum"] == 12
    assert "video_stitch" in duration["description"]
    assert "简体中文" in SeedanceVideo.input_schema["properties"]["prompt"]["description"]
    assert "上一段" in SeedanceVideo.input_schema["properties"]["first_clip"]["description"]


def test_seedance_bridge_configuration_fails_closed(monkeypatch):
    monkeypatch.delenv("BEAV_BRIDGE_URL", raising=False)
    monkeypatch.delenv("BEAV_BRIDGE_TOKEN", raising=False)

    with pytest.raises(RuntimeError, match="BEAV_BRIDGE_URL"):
        SeedanceVideo._bridge_endpoint()

    monkeypatch.setenv("BEAV_BRIDGE_URL", "http://127.0.0.1:32100")
    with pytest.raises(RuntimeError, match="BEAV_BRIDGE_TOKEN"):
        SeedanceVideo._bridge_headers()


def test_seedance_continuation_uses_previous_clip_without_original_image(monkeypatch):
    captured = {}

    class FakeResponse:
        def raise_for_status(self):
            return None

        def json(self):
            return {
                "result": {
                    "success": True,
                    "data": {
                        "generationMode": "continuation",
                        "assets": [{"absolutePath": "/managed/clip-02.mp4"}],
                    },
                },
            }

    def fake_post(url, headers, json, timeout):
        captured["url"] = url
        captured["headers"] = headers
        captured["json"] = json
        captured["timeout"] = timeout
        return FakeResponse()

    monkeypatch.setenv("BEAV_BRIDGE_URL", "http://127.0.0.1:32100")
    monkeypatch.setenv("BEAV_BRIDGE_TOKEN", "test-local-token")
    monkeypatch.setattr("tools.video.seedance_video.requests.post", fake_post)

    result = SeedanceVideo().execute({
        "prompt": "妈妈继续把婴儿抱起。",
        "operation": "image_to_video",
        "image_path": "/managed/original.png",
        "first_clip": "/managed/clip-01.mp4",
        "duration": 8,
    })

    payload = captured["json"]["payload"]["payload"]
    assert result.success is True
    assert captured["headers"] == {"Authorization": "Bearer test-local-token"}
    assert payload["generationMode"] == "continuation"
    assert payload["firstClip"] == "/managed/clip-01.mp4"
    assert "referenceImages" not in payload
    assert payload["prompt"].startswith("紧接输入视频的最后一帧继续")
    assert result.data["reference_images_suppressed_for_continuation"] is True


def test_registered_media_path_replaces_temporary_public_output():
    result = {
        "success": True,
        "data": {
            "output": "/tmp/final.mp4",
            "output_path": "/tmp/final.mp4",
        },
        "artifacts": ["/tmp/final.mp4"],
    }
    managed = "/Users/demo/.redconvert/spaces/default/media/generated/media_final.mp4"

    promoted = promote_registered_media_outputs(result, [{"absolutePath": managed}])

    assert promoted["data"]["output"] == managed
    assert promoted["data"]["output_path"] == managed
    assert promoted["data"]["storage"] == "jiuban_media_library"
    assert promoted["artifacts"] == [managed]


def test_registered_media_assets_use_project_type_directories(tmp_path: Path, monkeypatch):
    media_root = tmp_path / "media"
    render_root = tmp_path / "render"
    render_root.mkdir()
    monkeypatch.setenv("BEAV_MEDIA_ROOT", str(media_root))

    cover = render_root / "cover.png"
    cover.write_bytes(b"image")
    cover_assets = register_result_media_assets(
        root=tmp_path,
        om_root=tmp_path,
        inputs={"project_id": "Learning Desk Ad", "title": "学习桌首图"},
        result={
            "success": True,
            "data": {"output_path": str(cover), "delivery_role": "cover"},
            "artifacts": [str(cover)],
        },
        tool_name="openai_image",
    )

    final_video = render_root / "final.mp4"
    final_video.write_bytes(b"video")
    final_assets = register_result_media_assets(
        root=tmp_path,
        om_root=tmp_path,
        inputs={"project_id": "Learning Desk Ad", "title": "学习桌广告最终成片"},
        result={
            "success": True,
            "data": {"output_path": str(final_video), "delivery_role": "final_video"},
            "artifacts": [str(final_video)],
        },
        tool_name="video_stitch",
    )

    cover_relative = Path(cover_assets[0]["relativePath"])
    final_relative = Path(final_assets[0]["relativePath"])
    assert cover_relative.parts[:3] == ("generated", "learning-desk-ad", "images")
    assert final_relative.parts[:3] == ("generated", "learning-desk-ad", "output")
    assert final_assets[0]["deliveryRole"] == "final_video"
    assert Path(cover_assets[0]["absolutePath"]).is_file()
    assert Path(final_assets[0]["absolutePath"]).is_file()

    catalog = read_media_catalog(media_root)
    assert {asset.get("projectId") for asset in catalog["assets"]} == {"Learning Desk Ad"}


def test_video_stitch_schema_exposes_project_metadata():
    properties = VideoStitch.input_schema["properties"]

    assert properties["project_id"]["type"] == "string"
    assert properties["title"]["type"] == "string"


class BundledFfmpegStitch(VideoStitch):
    def _ffmpeg_has_filter(self, name: str) -> bool:
        return False

    def _probe_clip(self, path: str):
        return {
            "path": path,
            "width": 720,
            "height": 1280,
            "fps": 24.0,
            "video_codec": "h264",
            "pixel_format": "yuv420p",
            "audio_codec": None,
            "sample_rate": None,
            "duration": 12.0,
        }

    def _stitch_cut(self, clips, output_path, temp_dir, temp_files):
        output_path.write_bytes(b"stitched")
        return {"method": "test-cut"}


def test_missing_crossfade_filters_fall_back_to_cut(tmp_path: Path):
    clips = [tmp_path / "one.mp4", tmp_path / "two.mp4"]
    clips[0].write_bytes(b"clip-one")
    clips[1].write_bytes(b"clip-two")
    output = tmp_path / "output.mp4"

    result = BundledFfmpegStitch().execute(
        {
            "operation": "stitch",
            "clips": [str(clip) for clip in clips],
            "output_path": str(output),
            "transition": "crossfade",
            "auto_normalize": True,
            "target_resolution": "720x1280",
            "target_fps": 24,
        }
    )

    assert result.success is True
    assert result.data["requested_transition"] == "crossfade"
    assert result.data["transition"] == "cut"
    assert result.data["auto_normalized"] is False
    assert result.data["transition_fallback_reason"]
    assert result.data["delivery_role"] == "final_video"
    assert result.data["chat_visibility"] == "final_only"


def test_exact_duplicate_clip_content_is_rejected(tmp_path: Path):
    clips = [tmp_path / "one.mp4", tmp_path / "two.mp4"]
    for clip in clips:
        clip.write_bytes(b"same-clip")

    result = BundledFfmpegStitch().execute({
        "operation": "stitch",
        "clips": [str(clip) for clip in clips],
        "output_path": str(tmp_path / "output.mp4"),
    })

    assert result.success is False
    assert "重复视频片段" in result.error
    assert result.data["exact_duplicates"][0]["kind"] == "same_content"


class RepeatedIntroStitch(BundledFfmpegStitch):
    def _detect_repeated_intros(self, clips, probes):
        return [{
            "previous_clip_index": 0,
            "current_clip_index": 1,
            "start_similarity": 0.88,
            "continuity_similarity": 0.24,
            "sample_duration_seconds": 3.0,
            "reason": "test",
        }]


def test_repeated_intro_strict_mode_blocks_bad_final(tmp_path: Path):
    clips = [tmp_path / "one.mp4", tmp_path / "two.mp4"]
    clips[0].write_bytes(b"clip-one")
    clips[1].write_bytes(b"clip-two")

    result = RepeatedIntroStitch().execute({
        "operation": "stitch",
        "clips": [str(clip) for clip in clips],
        "output_path": str(tmp_path / "output.mp4"),
        "continuity_check": "strict",
    })

    assert result.success is False
    assert "重复开场" in result.error
    assert "first_clip" in result.error


class CaptureNormalizeCommand(VideoStitch):
    def __init__(self):
        self.command = None

    def _ffmpeg_has_filter(self, name: str) -> bool:
        return False

    def run_command(self, cmd, **kwargs):
        self.command = cmd
        return CompletedProcess(cmd, 0, "", "")


def test_normalization_avoids_unavailable_pad_filter(tmp_path: Path):
    stitch = CaptureNormalizeCommand()
    stitch._normalize_clip(
        "input.mp4",
        tmp_path / "output.mp4",
        720,
        1280,
        24,
        "libx264",
        "aac",
        20,
        "medium",
    )

    filter_value = stitch.command[stitch.command.index("-vf") + 1]
    assert filter_value == "scale=720:1280"
    assert "pad=" not in filter_value
