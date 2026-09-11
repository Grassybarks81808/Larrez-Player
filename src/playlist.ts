import { inspect, humanSize, type FormatInfo } from './formats';

export interface Track {
  id: string;
  name: string;
  /** Object URL for local files, or a direct URL for streams. */
  src: string;
  /** Kept so we can revoke the object URL and re-derive metadata. */
  file?: File;
  size: number;
  info: FormatInfo;
  duration?: number;
}

let seq = 0;

export function trackFromFile(file: File): Track {
  return {
    id: `t${++seq}`,
    name: file.name,
    src: URL.createObjectURL(file),
    file,
    size: file.size,
    info: inspect(file.name),
  };
}

export function trackFromUrl(url: string): Track {
  let name = url;
  try {
    const u = new URL(url, location.href);
    name = decodeURIComponent(u.pathname.split('/').filter(Boolean).pop() || u.hostname);
  } catch { /* keep raw string */ }
  return {
    id: `t${++seq}`,
    name,
    src: url,
    size: 0,
    info: inspect(name),
  };
}

export function subtitleLine(t: Track): string {
  const bits = [t.info.container];
  if (t.size) bits.push(humanSize(t.size));
  if (t.info.support === 'unsupported') bits.push('needs native build');
  else if (t.info.support === 'partial') bits.push('codec-dependent');
  return bits.join(' · ');
}

const VIDEO_RE = /\.(mp4|m4v|mov|webm|ogv|ogg|mkv|avi|ts|m2ts|flv|wmv|mpe?g|3gp)$/i;

export function isVideoFile(f: File): boolean {
  return f.type.startsWith('video/') || VIDEO_RE.test(f.name);
}

/** Natural sort so "ep2" comes before "ep10". */
export function sortByName(a: Track, b: Track): number {
  return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' });
}
