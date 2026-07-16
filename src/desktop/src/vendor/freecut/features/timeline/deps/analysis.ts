export type SceneCut = {
  frame: number;
  confidence?: number;
};

export type SceneDetectionProgress = {
  processedFrames?: number;
  totalFrames?: number;
  totalSamples: number;
  sceneCuts: number;
  percent: number;
  stage?: 'optical-flow' | 'loading-model' | 'verifying';
  verificationModel?: string;
};

export type VerificationModel = string;

export async function detectScenes(
  _video?: HTMLVideoElement,
  _fps?: number,
  _options?: {
    method?: string;
    verificationModel?: VerificationModel;
    mediaId?: string;
    signal?: AbortSignal;
    onProgress?: (progress: SceneDetectionProgress) => void;
  },
): Promise<SceneCut[]> {
  return [];
}

export function getSceneVerificationModelLabel(model: string): string {
  return model || 'disabled';
}

export function getSceneVerificationModelOptions(): Array<{ value: VerificationModel; label: string }> {
  return [{ value: 'disabled', label: 'Disabled' }];
}
