<script lang="ts">
  // Stroke icon set — 24x24, 1.8px stroke, currentColor. Decorative by
  // default; pass `label` when the icon carries meaning on its own.
  const PATHS: Record<string, string[]> = {
    folder: ['M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z'],
    file: ['M6 2.5h7l5 5V21H6z', 'M13 2.5V8h5'],
    search: ['M11 4a7 7 0 1 1 0 14 7 7 0 0 1 0-14z', 'm20 20-3.8-3.8'],
    'chevron-right': ['m9 6 6 6-6 6'],
    'chevron-down': ['m6 9 6 6 6-6'],
    'chevron-left': ['m15 6-6 6 6 6'],
    terminal: ['m5 7 4.5 5L5 17', 'M12.5 17H19'],
    edit: ['M4 20h4L19.5 8.5a2.1 2.1 0 0 0-3-3L5 17z', 'm14.5 6.5 3 3'],
    alert: ['M12 3.5 2.5 20h19z', 'M12 10v4.2', 'M12 17.2h.01'],
    check: ['m5 12.5 4.5 4.5L19 7.5'],
    x: ['M6 6l12 12', 'M18 6 6 18'],
    send: ['M21 3 10.5 13.5', 'M21 3l-6.8 18-3.7-7.5L3 9.8z'],
    sliders: ['M4 7h9', 'M17 7h3', 'M4 17h3', 'M11 17h9', 'M15 4.5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5z', 'M9 14.5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5z'],
    list: ['M9 6.5h11', 'M9 12h11', 'M9 17.5h11', 'M4.5 6.5h.01', 'M4.5 12h.01', 'M4.5 17.5h.01'],
    clock: ['M12 3.5a8.5 8.5 0 1 1 0 17 8.5 8.5 0 0 1 0-17z', 'M12 7.5V12l3 2'],
    diff: ['M8 3.5v17', 'M16 3.5v17', 'm11 7-3 3 3 3', 'm13 14 3 3-3 3'],
    'panel-left': ['M4 4h16v16H4z', 'M9.5 4v16'],
    'panel-right': ['M4 4h16v16H4z', 'M14.5 4v16'],
    bolt: ['M13 2.5 4.5 13.5H11l-1 8L18.5 10.5H12z'],
    branch: ['M6 3.5v5a3 3 0 0 0 3 3h6a3 3 0 0 1 3 3v5', 'M6 7a2 2 0 1 0 0-.01', 'M18 17a2 2 0 1 0 0-.01'],
  };

  interface Props {
    name: keyof typeof PATHS | string;
    size?: number;
    label?: string;
  }

  let { name, size = 14, label }: Props = $props();

  const paths = $derived(PATHS[name] ?? []);
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.8"
  stroke-linecap="round"
  stroke-linejoin="round"
  role={label ? 'img' : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : 'true'}
>
  {#each paths as d}
    <path {d} />
  {/each}
</svg>
