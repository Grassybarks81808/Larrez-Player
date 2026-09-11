/**
 * Format capability detection.
 *
 * The browser <video> element can only demux/decode what the host OS + browser
 * ships codecs for. Rather than silently failing on a .avi, we probe up-front
 * and tell the user exactly what will happen.
 */

export type Support = 'native' | 'partial' | 'unsupported';

export interface FormatInfo {
  ext: string;
  container: string;
  support: Support;
  note: string;
}

const MIME_BY_EXT: Record<string, string> = {
  mp4: 'video/mp4',
  m4v: 'video/mp4',
  mov: 'video/quicktime',
  webm: 'video/webm',
  ogv: 'video/ogg',
  ogg: 'video/ogg',
  mkv: 'video/webm', // Chromium demuxes many MKVs through the WebM path
  avi: 'video/x-msvideo',
  ts: 'video/mp2t',
  m2ts: 'video/mp2t',
  flv: 'video/x-flv',
  wmv: 'video/x-ms-wmv',
  mpg: 'video/mpeg',
  mpeg: 'video/mpeg',
  '3gp': 'video/3gpp',
  m3u8: 'application/vnd.apple.mpegurl',
  mpd: 'application/dash+xml',
};

const CONTAINER: Record<string, string> = {
  mp4: 'MPEG-4', m4v: 'MPEG-4', mov: 'QuickTime', webm: 'WebM',
  ogv: 'Ogg', ogg: 'Ogg', mkv: 'Matroska', avi: 'AVI',
  ts: 'MPEG-TS', m2ts: 'MPEG-TS', flv: 'Flash Video', wmv: 'Windows Media',
  mpg: 'MPEG-PS', mpeg: 'MPEG-PS', '3gp': '3GPP',
  m3u8: 'HLS', mpd: 'DASH',
};

/** Containers the browser will essentially never play without a real demuxer. */
const HARD_UNSUPPORTED = new Set(['avi', 'flv', 'wmv', 'mpg', 'mpeg', 'm2ts']);

/** Containers that play only when the inside codecs happen to be web codecs. */
const CODEC_DEPENDENT = new Set(['mkv', 'mov', 'ts']);

export function extOf(name: string): string {
  const m = /\.([a-z0-9]+)(?:[?#].*)?$/i.exec(name);
  return m ? m[1].toLowerCase() : '';
}

export function mimeFor(name: string): string {
  return MIME_BY_EXT[extOf(name)] ?? '';
}

let probe: HTMLVideoElement | null = null;
function canPlay(mime: string): string {
  if (!mime) return '';
  probe ??= document.createElement('video');
  return probe.canPlayType(mime);
}

export function inspect(name: string): FormatInfo {
  const ext = extOf(name);
  const container = CONTAINER[ext] ?? (ext ? ext.toUpperCase() : 'unknown');

  if (!ext) {
    return { ext, container, support: 'partial', note: 'No file extension — will try to play anyway.' };
  }

  if (HARD_UNSUPPORTED.has(ext)) {
    return {
      ext, container, support: 'unsupported',
      note: `${container} is not decodable in the browser. The native Windows build (libmpv) plays it directly.`,
    };
  }

  if (CODEC_DEPENDENT.has(ext)) {
    return {
      ext, container, support: 'partial',
      note: `${container} plays only if its video codec is web-supported (VP9, AV1, or H.264). HEVC/DTS tracks need the native build.`,
    };
  }

  const verdict = canPlay(MIME_BY_EXT[ext] ?? '');
  if (verdict === 'probably' || verdict === 'maybe') {
    return { ext, container, support: 'native', note: `${container} plays natively.` };
  }

  return {
    ext, container, support: 'partial',
    note: `${container} support is uncertain in this browser — attempting playback.`,
  };
}

/** Human-readable byte size. */
export function humanSize(bytes: number): string {
  if (!bytes) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  let n = bytes;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n < 10 && i > 0 ? n.toFixed(1) : Math.round(n)} ${u[i]}`;
}

/** mm:ss / h:mm:ss */
export function fmtTime(sec: number): string {
  if (!isFinite(sec) || sec < 0) return '0:00';
  const s = Math.floor(sec % 60);
  const m = Math.floor((sec / 60) % 60);
  const h = Math.floor(sec / 3600);
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** Convert SubRip to WebVTT so <track> can consume it. */
export function srtToVtt(srt: string): string {
  const body = srt
    .replace(/\r+/g, '')
    .replace(/^\uFEFF/, '')
    .replace(/(\d{2}:\d{2}:\d{2}),(\d{3})/g, '$1.$2');
  return `WEBVTT\n\n${body}`;
}
