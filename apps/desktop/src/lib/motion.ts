/** True when decorative motion (fades, drift, twinkle) should be settled. */
export function motionReduced(): boolean {
  if (typeof document === 'undefined') return false;
  return document.documentElement.dataset.ambientMotion === 'off';
}

export function ms(duration: number): number {
  if (typeof window === 'undefined') return duration;
  return motionReduced() ? 0 : duration;
}
