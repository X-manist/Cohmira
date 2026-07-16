export interface SpringConfig {
  tension: number;
  friction: number;
  mass: number;
}

function clamp(value: number): number {
  return Math.max(0, Math.min(1, value));
}

export function easeIn(value: number): number {
  return clamp(value) ** 2;
}

export function easeOut(value: number): number {
  return 1 - (1 - clamp(value)) ** 2;
}

export function easeInOut(value: number): number {
  const progress = clamp(value);
  return progress < 0.5 ? 2 * progress * progress : 1 - ((-2 * progress + 2) ** 2) / 2;
}

export function cubicBezier(
  value: number,
  points: { x1: number; y1: number; x2: number; y2: number },
): number {
  const x = clamp(value);
  let parameter = x;
  for (let iteration = 0; iteration < 6; iteration += 1) {
    const inverse = 1 - parameter;
    const estimatedX = 3 * inverse * inverse * parameter * points.x1
      + 3 * inverse * parameter * parameter * points.x2
      + parameter ** 3;
    const derivative = 3 * inverse * inverse * points.x1
      + 6 * inverse * parameter * (points.x2 - points.x1)
      + 3 * parameter * parameter * (1 - points.x2);
    if (Math.abs(derivative) < 1e-6) break;
    parameter = clamp(parameter - (estimatedX - x) / derivative);
  }
  const inverse = 1 - parameter;
  return clamp(
    3 * inverse * inverse * parameter * points.y1
      + 3 * inverse * parameter * parameter * points.y2
      + parameter ** 3,
  );
}

export function springEasing(value: number, config: SpringConfig): number {
  const progress = clamp(value);
  const damping = config.friction / (2 * Math.sqrt(config.tension * config.mass));
  const angularFrequency = Math.sqrt(config.tension / config.mass);
  const envelope = Math.exp(-damping * angularFrequency * progress);
  return clamp(1 - envelope * Math.cos(angularFrequency * Math.sqrt(Math.max(0, 1 - damping ** 2)) * progress));
}
