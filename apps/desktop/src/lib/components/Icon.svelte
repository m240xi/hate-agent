<script lang="ts">
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
    code: ['m8 6-6 6 6 6', 'm16 6 6 6-6 6'],
    chat: ['M21 11.5a8.5 8.5 0 0 1-8.5 8.5c-1.4 0-2.8-.3-4-.9L3 21l1.9-5.5a8.5 8.5 0 1 1 16.1-4z'],
    gear: [
      'M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7z',
      'M19.4 13.5a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1 1.55V21a2 2 0 1 1-4 0v-.09a1.7 1.7 0 0 0-1-1.55 1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.87 1.7 1.7 0 0 0-1.55-1H3a2 2 0 1 1 0-4h.09a1.7 1.7 0 0 0 1.55-1 1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34h.01a1.7 1.7 0 0 0 1-1.55V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1 1.55h.01a1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87v.01a1.7 1.7 0 0 0 1.55 1H21a2 2 0 1 1 0 4h-.09a1.7 1.7 0 0 0-1.55 1z',
    ],
    home: ['m3 11 9-8 9 8', 'M5 9.5V21h14V9.5'],
    link: ['M10 14a5 5 0 0 0 7.07 0l3-3a5 5 0 0 0-7.07-7.07l-1.5 1.5', 'M14 10a5 5 0 0 0-7.07 0l-3 3a5 5 0 0 0 7.07 7.07l1.5-1.5'],
    key: ['M14 10a4 4 0 1 0-4.9 3.9L12 15h2v2h2v2h4v-4.1A4 4 0 0 0 14 10z', 'M8.5 10.5h.01'],
    min: ['M5 12h14'],
    max: ['M5.5 5.5h13v13h-13z'],
    plus: ['M12 5v14', 'M5 12h14'],
    trash: ['M4 7h16', 'M9 7V4.5h6V7', 'M6.5 7l1 13h9l1-13', 'M10 11v5', 'M14 11v5'],
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
