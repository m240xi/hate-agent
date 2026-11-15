// Deliberately exclude SVG/HTML and remote URLs: model output must not execute
// scripts or silently make network requests just by appearing in the transcript.
export function isInlineImage(source: string): boolean {
  return /^data:image\/(?:png|jpeg|webp|gif|bmp);base64,[a-z0-9+/=\s]+$/i.test(source);
}

export function imageSources(args: unknown, preview?: string, images: string[] = []): string[] {
  const found = new Set(images);
  const visit = (value: unknown, depth = 0): void => {
    if (!value || depth > 8) return;
    if (Array.isArray(value)) {
      value.forEach((v) => visit(v, depth + 1));
    } else if (typeof value === 'object') {
      const item = value as Record<string, unknown>;
      if (item.type === 'image' && typeof item.mimeType === 'string' && typeof item.data === 'string') {
        const source = `data:${item.mimeType};base64,${item.data}`;
        if (isInlineImage(source)) found.add(source);
      }
      for (const [key, v] of Object.entries(value)) {
        if (/^image_?paths$/i.test(key) && Array.isArray(v)) {
          v.filter((p): p is string => typeof p === 'string').forEach((p) => found.add(p));
        }
        if (typeof v === 'string') {
          if (/^(image_?path|image_?paths)$/i.test(key)) found.add(v);
          else if (/^(path|file|url)$/i.test(key) && /\.(png|jpe?g|gif|webp|bmp)$/i.test(v)) found.add(v);
          else if (key === 'data' && isInlineImage(v)) found.add(v);
          else if (key === 'text') { try { visit(JSON.parse(v), depth + 1); } catch { /* plain text */ } }
        } else visit(v, depth + 1);
      }
    }
  };
  visit(args);
  if (preview) {
    try { visit(JSON.parse(preview)); } catch {
      // ACP previews may contain multiple JSON/text blocks rather than one object.
      for (const match of preview.matchAll(/"image_?path"\s*:\s*("(?:[^"\\]|\\.)*")/gi)) {
        try { found.add(JSON.parse(match[1] ?? '')); } catch { /* incomplete streamed JSON */ }
      }
    }
  }
  return [...found].filter((s) => isInlineImage(s) || !/^[a-z][a-z0-9+.-]*:/i.test(s) || /^[a-z]:[\\/]/i.test(s));
}
