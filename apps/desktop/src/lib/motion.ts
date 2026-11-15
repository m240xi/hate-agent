export function ms(duration: number): number {
  if (typeof window === 'undefined') return duration;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : duration;
}
