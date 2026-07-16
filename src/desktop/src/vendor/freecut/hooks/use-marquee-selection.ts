import { useEffect, useRef, useState, type RefObject } from 'react';

export interface MarqueeState {
  active: boolean;
  startX: number;
  startY: number;
  currentX: number;
  currentY: number;
}

interface SelectableItem {
  id: string;
  getBoundingRect: () => Pick<DOMRect, 'left' | 'top' | 'right' | 'bottom' | 'width' | 'height'> | null;
}

interface UseMarqueeSelectionOptions {
  containerRef: RefObject<HTMLElement>;
  items: SelectableItem[];
  onSelectionChange: (ids: string[]) => void;
  onPreviewSelectionChange?: (ids: string[]) => void;
  enabled?: boolean;
  threshold?: number;
  commitSelectionOnMouseUp?: boolean;
  liveCommitThrottleMs?: number;
}

const IDLE_STATE: MarqueeState = {
  active: false,
  startX: 0,
  startY: 0,
  currentX: 0,
  currentY: 0,
};

export function getMarqueeRect(startX: number, startY: number, currentX: number, currentY: number) {
  const left = Math.min(startX, currentX);
  const top = Math.min(startY, currentY);
  const right = Math.max(startX, currentX);
  const bottom = Math.max(startY, currentY);
  return { left, top, right, bottom, width: right - left, height: bottom - top };
}

function intersects(
  selection: ReturnType<typeof getMarqueeRect>,
  item: ReturnType<SelectableItem['getBoundingRect']>,
): boolean {
  return Boolean(item)
    && selection.left <= item!.right
    && selection.right >= item!.left
    && selection.top <= item!.bottom
    && selection.bottom >= item!.top;
}

export function useMarqueeSelection({
  containerRef,
  items,
  onSelectionChange,
  onPreviewSelectionChange,
  enabled = true,
  threshold = 4,
  commitSelectionOnMouseUp = true,
}: UseMarqueeSelectionOptions) {
  const [marqueeState, setMarqueeState] = useState<MarqueeState>(IDLE_STATE);
  const optionsRef = useRef({ items, onSelectionChange, onPreviewSelectionChange });
  optionsRef.current = { items, onSelectionChange, onPreviewSelectionChange };

  useEffect(() => {
    const container = containerRef.current;
    if (!container || !enabled) return undefined;

    const handlePointerDown = (event: PointerEvent) => {
      if (event.button !== 0 || (event.target as Element | null)?.closest('[data-item-id]')) return;
      const startX = event.clientX;
      const startY = event.clientY;
      let active = false;

      const updateSelection = (currentX: number, currentY: number) => {
        const rect = getMarqueeRect(startX, startY, currentX, currentY);
        const selected = optionsRef.current.items
          .filter((item) => intersects(rect, item.getBoundingRect()))
          .map((item) => item.id);
        optionsRef.current.onPreviewSelectionChange?.(selected);
        return selected;
      };

      const handlePointerMove = (moveEvent: PointerEvent) => {
        if (!active && Math.hypot(moveEvent.clientX - startX, moveEvent.clientY - startY) < threshold) return;
        active = true;
        setMarqueeState({
          active: true,
          startX,
          startY,
          currentX: moveEvent.clientX,
          currentY: moveEvent.clientY,
        });
        updateSelection(moveEvent.clientX, moveEvent.clientY);
      };

      const handlePointerUp = (upEvent: PointerEvent) => {
        window.removeEventListener('pointermove', handlePointerMove);
        window.removeEventListener('pointerup', handlePointerUp);
        if (active && commitSelectionOnMouseUp) {
          optionsRef.current.onSelectionChange(updateSelection(upEvent.clientX, upEvent.clientY));
        }
        optionsRef.current.onPreviewSelectionChange?.([]);
        setMarqueeState(IDLE_STATE);
      };

      window.addEventListener('pointermove', handlePointerMove);
      window.addEventListener('pointerup', handlePointerUp, { once: true });
    };

    container.addEventListener('pointerdown', handlePointerDown);
    return () => container.removeEventListener('pointerdown', handlePointerDown);
  }, [commitSelectionOnMouseUp, containerRef, enabled, threshold]);

  return { marqueeState };
}
