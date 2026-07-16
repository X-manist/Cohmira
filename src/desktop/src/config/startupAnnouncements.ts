import type { ViewType } from '../App';

export interface StartupAnnouncementStep {
  id: string;
  selector: string;
  title: string;
  description: string;
  placement: 'top' | 'top-start' | 'top-end' | 'bottom' | 'bottom-start' | 'bottom-end' | 'left' | 'left-start' | 'left-end' | 'right' | 'right-start' | 'right-end';
  view?: ViewType;
}

export interface StartupAnnouncementShortcut {
  id: string;
  label: string;
  view: ViewType;
}

export interface StartupAnnouncementFeature {
  id: string;
  label: string;
  icon: 'knowledge' | 'wander' | 'draft' | 'generate' | 'automation';
}

export interface StartupAnnouncement {
  id: string;
  version: string;
  badge: string;
  title: string;
  summary: string;
  highlights: string[];
  hero: StartupAnnouncementFeature[];
  shortcuts?: StartupAnnouncementShortcut[];
  steps?: StartupAnnouncementStep[];
}

const ANNOUNCEMENT_STORAGE_PREFIX = 'redbox:startup-announcement:v1:';

// 每次发新版本时，在这里追加一条新配置。
// 只要 `id` 或 `version` 变化，弹窗就会对该版本重新展示一次。
export const STARTUP_ANNOUNCEMENTS: StartupAnnouncement[] = [
  {
    id: 'release-1.9.4-product-workflow',
    version: '1.9.4',
    badge: 'v1.9.4 新功能',
    title: '启动弹窗现在可以按版本独立管理',
    summary: '这次更新把首个弹窗改成了版本化内容位。每个版本都能有自己的标题、摘要、按钮和可选引导。',
    highlights: [
      '默认只展示简短摘要，不再堆很多说明文字。',
      '需要时可以给当前版本挂 3 个以内的快捷入口按钮。',
      '如果某个版本需要讲解导航，再单独配置引导步骤。',
    ],
    hero: [
      { id: 'draft', label: '版本弹窗', icon: 'draft' },
      { id: 'generate', label: '快捷入口', icon: 'generate' },
      { id: 'automation', label: '可选引导', icon: 'automation' },
    ],
    shortcuts: [
      { id: 'manuscripts', label: '去稿件', view: 'manuscripts' },
      { id: 'redclaw', label: '去运营中枢', view: 'redclaw' },
      { id: 'media-library', label: '去素材库', view: 'manuscripts' },
    ],
    steps: [
      {
        id: 'manuscripts',
        selector: '[data-guide-id="nav-manuscripts"]',
        title: '1/3 稿件是默认工作台',
        description: '启动后先回到稿件，继续处理正在生产的内容。',
        placement: 'right',
        view: 'manuscripts',
      },
      {
        id: 'redclaw',
        selector: '[data-guide-id="nav-redclaw"]',
        title: '2/2 运营中枢统一对话和创作',
        description: '对话、工具串联、图片视频生成和值守任务都在这里触发。',
        placement: 'right',
        view: 'redclaw',
      },
    ],
  },
];

export function getStartupAnnouncementByVersion(version: string): StartupAnnouncement | null {
  const normalized = String(version || '').trim();
  if (!normalized) return null;
  return STARTUP_ANNOUNCEMENTS.find((item) => item.version === normalized) || null;
}

export function getStartupAnnouncementSeenKey(id: string): string {
  return `${ANNOUNCEMENT_STORAGE_PREFIX}${id}`;
}
