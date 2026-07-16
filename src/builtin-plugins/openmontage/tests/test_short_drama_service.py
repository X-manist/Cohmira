from __future__ import annotations

from pathlib import Path
import tempfile
import unittest

from lib.short_drama.service import ShortDramaService
from lib.short_drama.store import ShortDramaError


def episode_plan() -> dict:
    return {
        "version": "1.0",
        "title": "雨夜来信",
        "series_logline": "失踪多年的姐姐在雨夜寄来一封当天写下的信。",
        "episodes": [
            {
                "id": "ep_001",
                "number": 1,
                "title": "迟到的信",
                "logline": "林夏发现信中预告了十分钟后的事故。",
                "duration_seconds": 60,
                "beats": ["收到信", "验证预告", "冲向车站"],
                "hook": "信上的墨迹还没干，寄信人却失踪了五年。",
                "cliffhanger": "站台监控里出现了姐姐。",
                "source_refs": ["source:chapter-01:p-01"],
            }
        ],
        "metadata": {},
    }


def drama_bible() -> dict:
    return {
        "version": "1.0",
        "title": "雨夜来信",
        "logline": "一封来自失踪者的实时来信迫使妹妹重走旧案。",
        "themes": ["记忆", "选择"],
        "format": {
            "episode_count": 1,
            "episode_duration_seconds": 60,
            "aspect_ratio": "9:16",
            "genre": "悬疑",
            "target_audience": "短剧用户",
        },
        "world": {
            "premise": "当晚写下的信会提前十分钟抵达。",
            "time_period": "当代",
            "rules": ["信只能预告十分钟后的事件"],
            "tone": "克制悬疑",
        },
        "characters": [
            {
                "id": "char_lin_xia",
                "name": "林夏",
                "role": "主角",
                "description": "寻找失踪姐姐的急诊医生。",
                "appearance": "28岁，短黑发，左眉有浅疤。",
                "wardrobe": ["深蓝急诊外套"],
                "personality": ["冷静", "执拗"],
                "motivation": "确认姐姐是否仍活着。",
                "arc": "从否认过去到主动面对真相。",
                "relationships": [],
                "voice_profile": "低声、克制、语速偏快",
                "negative_constraints": ["不得改变眉部疤痕位置"],
            }
        ],
        "locations": [
            {
                "id": "loc_old_station",
                "name": "旧车站",
                "description": "废弃站台，雨水沿铁棚落下。",
                "visual_anchors": ["绿色长椅", "停摆时钟"],
                "lighting": "冷色顶灯",
                "time_variants": ["雨夜"],
            }
        ],
        "props": [
            {
                "id": "prop_letter",
                "name": "来信",
                "description": "米白信纸，蓝黑墨水未干。",
                "story_function": "预告未来",
            }
        ],
        "visual_language": {
            "style": "现实主义悬疑",
            "palette": ["冷蓝", "钨丝暖黄"],
            "camera_language": "手持近景与稳定对称远景对比",
            "lighting": "低调光",
            "texture": "潮湿颗粒",
        },
        "continuity_rules": ["林夏左眉浅疤始终可见", "信纸在阅读后出现雨渍"],
        "source_refs": ["source:chapter-01:p-01"],
        "metadata": {},
    }


class ShortDramaServiceTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.projects_root = Path(self.temp.name)
        self.service = ShortDramaService(self.projects_root)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_versioned_project_flow_and_human_gate(self) -> None:
        created = self.service.create_project({
            "projectId": "test-drama-001",
            "title": "雨夜来信",
            "sourceText": "第一章：林夏收到一封来自失踪姐姐的信。",
            "episodeDurationSeconds": 60,
        })
        self.assertEqual(created["revision"], 2)
        self.assertIn("source_document", created["project"]["artifactData"])

        plan_result = self.service.save_artifact({
            "projectId": "test-drama-001",
            "expectedRevision": 2,
            "artifactType": "episode_plan",
            "artifact": episode_plan(),
        })
        self.assertEqual(plan_result["revision"], 3)

        bible_result = self.service.save_artifact({
            "projectId": "test-drama-001",
            "expectedRevision": 3,
            "artifactType": "drama_bible",
            "artifact": drama_bible(),
        })
        self.assertEqual(bible_result["project"]["stages"]["proposal"]["status"], "awaiting_review")
        self.assertEqual(bible_result["project"]["status"], "awaiting_human")

        selected = self.service.commit_selection({
            "projectId": "test-drama-001",
            "expectedRevision": 4,
            "scope": "characters",
            "selectedIds": ["char_lin_xia"],
        })
        self.assertEqual(selected["revision"], 5)
        self.assertEqual(
            selected["project"]["selections"]["characters"]["selectedIds"],
            ["char_lin_xia"],
        )

        approved = self.service.approve_stage({
            "projectId": "test-drama-001",
            "expectedRevision": 5,
            "stage": "proposal",
            "decision": "approve",
        })
        self.assertEqual(approved["revision"], 6)
        self.assertEqual(approved["project"]["stages"]["proposal"]["status"], "completed")
        self.assertEqual(approved["project"]["currentStage"], "script")

        history = self.projects_root / "test-drama-001" / "short_drama" / "history"
        self.assertTrue((history / "revision-000005.json").is_file())

    def test_stale_revision_is_rejected_without_overwrite(self) -> None:
        self.service.create_project({"projectId": "test-drama-002", "title": "测试项目"})
        self.service.save_source({
            "projectId": "test-drama-002",
            "expectedRevision": 1,
            "content": "原作 A",
        })

        with self.assertRaises(ShortDramaError) as caught:
            self.service.save_source({
                "projectId": "test-drama-002",
                "expectedRevision": 1,
                "content": "过期写入",
            })

        self.assertEqual(caught.exception.code, "DRAMA_REVISION_CONFLICT")
        current = self.service.get_project({"projectId": "test-drama-002"})
        self.assertEqual(current["project"]["artifactData"]["source_document"]["content"], "原作 A")


if __name__ == "__main__":
    unittest.main()

