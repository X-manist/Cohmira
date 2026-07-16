import { type ReactNode } from 'react';
import type {
  AnimatableProperty,
  EasingType,
  ItemKeyframes,
  Keyframe,
  EasingConfig,
} from '@/types/keyframe';
import type { TimelineItem } from '@/types/timeline';
import type { ResolvedTransform, TransformProperties } from '@/types/transform';

type AddAutoKeyframeOperation = {
  type?: 'add';
  itemId: string;
  property: AnimatableProperty;
  frame: number;
  value: number;
  easing?: EasingType;
  easingConfig?: EasingConfig;
};

type UpdateAutoKeyframeOperation = {
  type: 'update';
  itemId: string;
  property: AnimatableProperty;
  keyframeId: string;
  updates: Partial<Omit<Keyframe, 'id'>>;
};

export type AutoKeyframeOperation = AddAutoKeyframeOperation | UpdateAutoKeyframeOperation;

function keyframesForProperty(
  itemKeyframes: ItemKeyframes | undefined,
  property: AnimatableProperty,
): Keyframe[] {
  return itemKeyframes?.properties.find((entry) => entry.property === property)?.keyframes ?? [];
}

export function resolveAnimatedTransform(
  item: TimelineItem,
  itemKeyframes?: ItemKeyframes,
): ResolvedTransform {
  const base = item.transform || {};
  const readValue = (property: AnimatableProperty, fallback: number) => {
    const keyframes = keyframesForProperty(itemKeyframes, property);
    return keyframes[0]?.value ?? fallback;
  };

  return {
    x: readValue('x', base.x ?? 0),
    y: readValue('y', base.y ?? 0),
    width: readValue('width', base.width ?? 0),
    height: readValue('height', base.height ?? 0),
    rotation: readValue('rotation', base.rotation ?? 0),
    opacity: readValue('opacity', base.opacity ?? 1),
    cornerRadius: readValue('cornerRadius', base.cornerRadius ?? 0),
  };
}

export function interpolatePropertyValue(
  itemKeyframes: ItemKeyframes | Keyframe[] | undefined,
  propertyOrFrame: AnimatableProperty | number,
  frameOrFallback: number,
  fallback = 0,
): number {
  const keyframes = Array.isArray(itemKeyframes)
    ? itemKeyframes
    : keyframesForProperty(itemKeyframes, propertyOrFrame as AnimatableProperty);
  const frame = typeof propertyOrFrame === 'number' ? propertyOrFrame : frameOrFallback;
  const resolvedFallback = typeof propertyOrFrame === 'number' ? frameOrFallback : fallback;
  if (keyframes.length === 0) return resolvedFallback;
  const previous = [...keyframes].reverse().find((entry) => entry.frame <= frame) ?? keyframes[0];
  return previous?.value ?? resolvedFallback;
}

export function getAnimatablePropertiesForItem(item: TimelineItem): AnimatableProperty[] {
  const base: AnimatableProperty[] = ['x', 'y', 'width', 'height', 'rotation', 'opacity', 'cornerRadius'];
  return item.type === 'audio' ? ['volume'] : base;
}

export function getBezierPresetForEasing(easing: EasingType) {
  if (easing === 'ease-in') return { x1: 0.42, y1: 0, x2: 1, y2: 1 };
  if (easing === 'ease-out') return { x1: 0, y1: 0, x2: 0.58, y2: 1 };
  if (easing === 'ease-in-out') return { x1: 0.42, y1: 0, x2: 0.58, y2: 1 };
  return null;
}

export function isFrameInTransitionRegion(
  _frame?: number,
  _itemId?: string,
  _item?: TimelineItem,
  _transitions?: unknown,
): { start: number; end: number } | undefined {
  return undefined;
}

export function getTransitionBlockedRanges(
  _itemId?: string,
  _item?: TimelineItem,
  _transitions?: unknown,
): Array<{ start: number; end: number }> {
  return [];
}

export function ValueGraphEditor(_: { children?: ReactNode; transform?: TransformProperties; [key: string]: unknown }) {
  return null;
}

export function DopesheetEditor(_: { children?: ReactNode; [key: string]: unknown }) {
  return null;
}
