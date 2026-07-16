---
name: openmontage-video-production
description: 使用 OpenMontage 完成 AI 短剧和常规视频的策划、角色设定、剧本、导演分镜、素材处理、剪辑、合成、质检与交付；视频生成统一调用商媒运营助手内置火山引擎服务。
---

# OpenMontage 视频制作

本插件负责视频生产编排、剪辑和质量控制。先读取 `INDEX.md` 与目标 pipeline，再按阶段读取
`pipelines/`、`core/`、`creative/` 和 `meta/` 中需要的文档。

## 中文交付规则

- 默认工作语言是简体中文；用户未明确指定其他语言时，标题、生成提示词、素材描述、进度说明和最终答复都使用中文。
- 不要为了调用图像或视频模型而把中文需求翻译成英文。Seedance 和商媒运营助手媒体服务可以直接接收中文提示词。
- 多素材任务在第一次生图前先确定一个稳定的 `project_id`。首图、关键帧、每个视频片段和 `video_stitch` 必须复用同一个值。
- 项目素材统一归档为 `generated/<project-id>/images/`、`clips/`、`output/`、`audio/`；禁止把同一任务的文件散落在 `generated/` 根目录。
- 项目稿件统一保存到 `manuscripts/projects/<project-id>/`，至少包含需求说明、脚本和交付记录；若已创建商媒运营助手视频项目包，则以项目包内的 `brief.md`、`script.md` 为准。
- 多段视频任务中，各分段属于中间素材：生成后写入素材库，不要在聊天中逐个展示或逐个列路径。
- 完成剪辑后，最终答复只声明用户要求的首图、最终成片等交付物，并优先给出商媒运营助手素材库中的持久化路径。
- `/tmp`、`/private/tmp` 和项目临时目录只是工具工作路径，绝不能作为最终交付路径展示给用户。
- 若单段最长时长不足，直接规划多段生成和 `video_stitch`，不要先尝试超出工具约束的时长。
- 多段连续视频的第一段可使用用户首图或核心环境图；从第二段开始必须调用 `seedance_video` 的续写模式，把上一段素材库路径放入 `first_clip`，不要让每一段都从同一张首图重新开始。
- 后续分段提示词只描述上一段尚未发生的动作，并明确“紧接上一段最后画面”；禁止重述开场动作。
- 拼接时保持 `continuity_check=strict`。若检测到相邻片段重复开场，先重生成后续片段，再制作成片。

当请求涉及小说改编、AI 漫剧、AI 短剧、连续角色、分集剧本或故事分镜时：

- 选择 `pipeline_defs/ai-short-drama.yaml`。
- 先读 `pipelines/ai-short-drama/executive-producer.md`，然后逐阶段读取对应 director。
- 用 `drama_project_create` 建项目，用 `drama_stage_context` 取得当前 revision 和输入制品。
- AI 根据 Skill 生成内容，再用 `drama_artifact_save` 保存；Python 不代替 AI 写故事、剧本或分镜。
- 角色、分镜、资产、配音与阶段批准都通过 `drama_open_review` 打开的 MCP App 让用户确认。
- 用户选择和批准是绑定状态；收到 UI 消息后必须重新读取最新 revision，不能沿用旧上下文。

视频生成只有一个受支持入口：

- 调用 `video_selector` 或 `seedance_video`。
- 这两个入口最终都通过商媒运营助手本地 App Bridge 调用 `app_cli video generate`。
- 不要调用 FAL、Kling、Runway、Veo、MiniMax、Replicate、ComfyUI 或本地 GPU 视频生成器。
- 生成结果由商媒运营助手媒体库管理；OpenMontage 继续负责分镜、剪辑、拼接、字幕、音频、QA 和交付。

Python 运行时由应用内置 uv 按需安装。不要要求用户单独安装 Python、pip 或虚拟环境。
