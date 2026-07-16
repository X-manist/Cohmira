import { useCallback, useRef } from 'react';

/** React 18-compatible form of React 19's useEffectEvent. */
export function useEffectEvent<T extends (...args: any[]) => any>(callback: T): T {
  const callbackRef = useRef(callback);
  callbackRef.current = callback;
  return useCallback(((...args: Parameters<T>) => callbackRef.current(...args)) as T, []);
}
