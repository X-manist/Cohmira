import type { CompositionInputProps } from '@/types/export';
import type { ItemKeyframes } from '@/types/keyframe';
import type { TimelineItem, TimelineTrack } from '@/types/timeline';
import type { Transition } from '@/types/transition';

type RenderSingleFrameOptions = {
  composition: CompositionInputProps;
  frame: number;
  width: number;
  height: number;
  quality?: number;
  format?: string;
};

/**
 * RedBox does not embed FreeCut's renderer.  Produce a valid background frame
 * for project thumbnails so persistence remains functional in the employee app.
 */
export async function renderSingleFrame(options: RenderSingleFrameOptions): Promise<Blob> {
  const format = options.format ?? 'image/png';
  if (typeof OffscreenCanvas !== 'undefined') {
    const canvas = new OffscreenCanvas(options.width, options.height);
    const context = canvas.getContext('2d');
    if (context) {
      context.fillStyle = options.composition.backgroundColor ?? '#000000';
      context.fillRect(0, 0, options.width, options.height);
      return canvas.convertToBlob({ type: format, quality: options.quality });
    }
  }
  return new Blob([], { type: format });
}

export function convertTimelineToComposition(
  tracks: TimelineTrack[],
  items: TimelineItem[],
  transitions: Transition[],
  fps: number,
  width?: number,
  height?: number,
  _inPoint?: number | null,
  _outPoint?: number | null,
  keyframes?: ItemKeyframes[],
  backgroundColor?: string,
): CompositionInputProps {
  const tracksWithItems = tracks.map((track) => ({
    ...track,
    items: items.filter((item) => item.trackId === track.id),
  }));
  const durationInFrames = items.reduce(
    (maximum, item) => Math.max(maximum, item.from + item.durationInFrames),
    0,
  );

  return {
    fps,
    width,
    height,
    durationInFrames,
    tracks: tracksWithItems,
    transitions,
    keyframes,
    backgroundColor,
  };
}
