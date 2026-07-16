import type { LongDraft, LongTemplate, ScheduleDraft, ScheduleTemplate } from './types';

export interface RedClawPromptPreset {
    id: string;
    label: string;
    text: string;
}

export const REDCLAW_CONTEXT_ID = 'redclaw-singleton';
export const REDCLAW_CONTEXT_TYPE = 'redclaw';
export const REDCLAW_CONTEXT = [
    '商媒运营助手是一个面向内容增长、素材管理、账号发布与复盘的 AI 工作台。',
    '工作目标：基于用户目标推进爆品分析、投流调研、内容创作、素材整理、发布排期与复盘，并给出可执行的工作流建议。',
    '创作执行：图片/视频生成在参数明确后调用 app_cli 的 image/video 工具，并返回素材结果或错误。',
    '参数展示：确认后的生成参数必须集中放在一个 text 代码块中，每行使用“字段：值”；不要把比例、数量、时长等值拆成单独的行内代码。',
    '默认输出结构：目标拆解、内容策略、执行步骤、风险提示。',
].join('\n');

export const DEFAULT_REDCLAW_PROMPT_PRESETS: RedClawPromptPreset[] = [
    {
        id: 'image-creation',
        label: '图片创作',
        text: '我要做图片创作：请先确认用途、平台尺寸、风格、数量和参考图；如果我已经上传或粘贴图片，请基于参考图生成方案，并在确认参数后调用图片生成工具。',
    },
    {
        id: 'video-creation',
        label: '视频创作',
        text: '我要做视频创作：请先确认视频模式、比例、时长、分镜、参考图/首尾帧和音频需求；参数明确后调用视频生成工具。',
    },
];

export const REDCLAW_OPERATION_SHORTCUTS = [
    { label: '爆品分析', text: '分析这个账号最近的爆款内容结构，并给出可复制的选题方向', action: 'inject' as const },
    { label: '投流调研', text: '围绕这个产品做一份投流素材和竞品内容调研', action: 'inject' as const },
    { label: '批量发布', text: '把这批内容整理成多平台发布计划，并列出需要补齐的素材', action: 'inject' as const },
    { label: '博主库构建', text: '根据这个行业方向设计一套博主库字段和采集筛选流程', action: 'inject' as const },
];

export const REDCLAW_SHORTCUTS = [
    ...DEFAULT_REDCLAW_PROMPT_PRESETS.map((preset) => ({
        label: preset.label,
        text: preset.text,
        action: 'inject' as const,
        presetPrompt: true,
    })),
    ...REDCLAW_OPERATION_SHORTCUTS,
];

export const REDCLAW_WELCOME_SHORTCUTS = REDCLAW_SHORTCUTS;

export const RUNNER_INTERVAL_OPTIONS = [10, 20, 30, 60];
export const RUNNER_MAX_AUTOMATION_OPTIONS = [1, 2, 3, 5];
export const HEARTBEAT_INTERVAL_OPTIONS = [15, 30, 60, 120];
export const REDCLAW_SIDEBAR_MIN_WIDTH = 300;
export const REDCLAW_SIDEBAR_MAX_WIDTH = 560;
export const REDCLAW_SIDEBAR_DEFAULT_WIDTH = 380;
export const REDCLAW_WELCOME_ICON_SRC = '/cohmira-mark.svg';

export const SCHEDULE_TEMPLATES: ScheduleTemplate[] = [
    {
        id: 'one-time-image',
        label: '单次真实图片生成',
        description: '在指定时间真实调用图片工具并保留回执',
        name: '单次真实图片生成',
        mode: 'once',
        requiredTools: ['generate_image'],
        prompt: '主题与主体：〔请填写〕\n业务用途与平台：〔请填写〕\n画面比例与像素要求：〔请填写〕\n视觉风格与禁用元素：〔请填写〕\n请使用 dry_run=false 实际调用 generate_image 生成 1 张图片并保存素材。不要只输出提示词、计划或文字说明。',
    },
    {
        id: 'one-time-video',
        label: '单次真实视频生成',
        description: '在指定时间真实调用视频工具并保留回执',
        name: '单次真实视频生成',
        mode: 'once',
        requiredTools: ['seedance_video'],
        prompt: '完整脚本与镜头动作：〔请填写〕\n画面比例、时长与分辨率：〔请填写〕\n参考素材绝对路径（没有则写“无”）：〔请填写〕\n风格、声音与禁用元素：〔请填写〕\n请实际调用 seedance_video 生成视频并保存素材。不要只输出分镜或文字说明。',
    },
    {
        id: 'daily-creation',
        label: '每日创作推进',
        description: '每天自动推进当前内容任务的文案与发布计划',
        name: '每日创作推进',
        mode: 'daily',
        time: '09:30',
        requiredTools: ['create_note'],
        prompt: '品牌/产品与目标人群：〔请填写〕\n内容平台与本轮主题：〔请填写〕\n必须遵守的事实与禁用表达：〔请填写〕\n请产出标题候选、正文、标签和发布计划，并调用 create_note 把完整 Markdown 稿件真实保存。',
    },
    {
        id: 'daily-image',
        label: '每日配图计划',
        description: '每天整理封面与配图需求并保存为笔记',
        name: '每日配图计划',
        mode: 'daily',
        time: '14:00',
        requiredTools: ['create_note'],
        prompt: '内容主题与平台：〔请填写〕\n目标受众与视觉基调：〔请填写〕\n已有素材路径或限制：〔请填写〕\n请整理封面和配图清单、每张图的提示词与验收标准，并调用 create_note 保存完整 Markdown 配图计划。本任务不生成图片。',
    },
    {
        id: 'weekly-retro',
        label: '每周复盘',
        description: '固定每周总结执行结果并给出下一步',
        name: '每周复盘',
        mode: 'weekly',
        time: '21:00',
        weekdays: [1, 4],
        requiredTools: ['create_note'],
        prompt: '复盘业务范围：〔请填写〕\n本周不可变数据快照（请直接粘贴正文，不要只填文件路径）：〔请填写〕\n核心目标与指标口径：〔请填写〕\n只依据以上快照输出有效动作、问题、下周假设和优先级动作，并调用 create_note 保存完整 Markdown 复盘；不得声称读取了未授权文件或实时系统。',
    },
    {
        id: 'interval-watch',
        label: '短周期巡检',
        description: '按固定间隔巡检内容卡点与风险',
        name: '内容巡检',
        mode: 'interval',
        intervalMinutes: 60,
        requiredTools: ['create_note'],
        prompt: '要巡检的业务范围：〔请填写〕\n当前任务与状态的不可变快照（请直接粘贴正文，不要只填文件路径）：〔请填写〕\n判断阻塞的规则：〔请填写〕\n只依据以上快照识别卡点并给出最小下一步行动，调用 create_note 保存巡检记录；不得声称读取了未授权文件或实时系统，也不修改其他任务状态。',
    },
];

export const LONG_TEMPLATES: LongTemplate[] = [
    {
        id: 'growth-sprint',
        label: '增长冲刺',
        description: '围绕一个目标持续多轮优化',
        name: '30天增长冲刺',
        objective: '品牌/产品：〔请填写〕。在 30 天内建立稳定的内容运营产出节奏并提升明确指标：〔请填写〕。',
        stepPrompt: '执行一轮增长冲刺：基于上一轮笔记调整选题策略，给出下一轮动作，并调用 create_note 保存本轮 Markdown 记录。不得声称修改未授权的稿件、素材或工作项。',
        intervalMinutes: 720,
        totalRounds: 30,
        requiredTools: ['create_note'],
    },
    {
        id: 'ip-building',
        label: '品牌表达构建',
        description: '持续沉淀表达定位与内容母题',
        name: '品牌表达构建计划',
        objective: '品牌/人物与业务领域：〔请填写〕。建立清晰的表达定位与可复用内容母题，形成稳定输出体系。',
        stepPrompt: '推进一轮表达构建：基于上一轮笔记提炼用户画像、选题母题和表达风格，并调用 create_note 保存本轮 Markdown 结果。',
        intervalMinutes: 1440,
        totalRounds: 21,
        requiredTools: ['create_note'],
    },
    {
        id: 'topic-lab',
        label: '选题实验室',
        description: '持续验证高潜选题',
        name: '选题实验室',
        objective: '业务领域、目标人群与平台：〔请填写〕。持续验证并筛选高潜选题，形成可追踪的选题记录。',
        stepPrompt: '执行一轮选题实验：基于上一轮笔记提出 3 个选题假设并评估优先级，调用 create_note 保存本轮 Markdown 记录。本任务不声称已发布或修改外部工作项。',
        intervalMinutes: 480,
        totalRounds: 20,
        requiredTools: ['create_note'],
    },
];

export const WEEKDAY_OPTIONS = [
    { value: 1, label: '周一' },
    { value: 2, label: '周二' },
    { value: 3, label: '周三' },
    { value: 4, label: '周四' },
    { value: 5, label: '周五' },
    { value: 6, label: '周六' },
    { value: 0, label: '周日' },
];

export function pickScheduleTemplate(templateId: string): ScheduleTemplate {
    return SCHEDULE_TEMPLATES.find((item) => item.id === templateId) || SCHEDULE_TEMPLATES[0];
}

export function pickLongTemplate(templateId: string): LongTemplate {
    return LONG_TEMPLATES.find((item) => item.id === templateId) || LONG_TEMPLATES[0];
}

export function scheduleDraftFromTemplate(template: ScheduleTemplate): ScheduleDraft {
    return {
        templateId: template.id,
        name: template.name,
        mode: template.mode,
        intervalMinutes: template.intervalMinutes || 60,
        time: template.time || '09:00',
        weekdays: template.weekdays || [1],
        runAtLocal: '',
        prompt: template.prompt,
        requiredToolsText: (template.requiredTools || []).join(', '),
        realOperationConfirmed: false,
    };
}

export function longDraftFromTemplate(template: LongTemplate): LongDraft {
    return {
        templateId: template.id,
        name: template.name,
        objective: template.objective,
        stepPrompt: template.stepPrompt,
        intervalMinutes: template.intervalMinutes,
        totalRounds: template.totalRounds,
        requiredToolsText: (template.requiredTools || []).join(', '),
        realOperationConfirmed: false,
    };
}
