import './style.css';
import { fmtTime, srtToVtt } from './formats';
import {
  trackFromFile, trackFromUrl, subtitleLine, isVideoFile, sortByName,
  type Track,
} from './playlist';

/* ------------------------------------------------------------------ *
 * Element lookups
 * ------------------------------------------------------------------ */
const $ = <T extends HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`Missing #${id}`);
  return el as T;
};

const video = $<HTMLVideoElement>('video');
const stage = $<HTMLDivElement>('stage');
const playlistEl = $<HTMLUListElement>('playlist');
const nowPlaying = $<HTMLDivElement>('now-playing');
const perfEl = $<HTMLDivElement>('perf');
const toastEl = $<HTMLDivElement>('toast');

const seek = $<HTMLDivElement>('seek');
const seekFill = $<HTMLDivElement>('seek-fill');
const seekBuffer = $<HTMLDivElement>('seek-buffer');
const seekKnob = $<HTMLDivElement>('seek-knob');
const seekTip = $<HTMLDivElement>('seek-tip');
const timeCur = $<HTMLSpanElement>('time-cur');
const timeDur = $<HTMLSpanElement>('time-dur');

const btnPlay = $<HTMLButtonElement>('btn-play');
const btnPrev = $<HTMLButtonElement>('btn-prev');
const btnNext = $<HTMLButtonElement>('btn-next');
const btnMute = $<HTMLButtonElement>('btn-mute');
const volume = $<HTMLInputElement>('volume');
const speed = $<HTMLSelectElement>('speed');

const audioWrap = $<HTMLLabelElement>('audio-wrap');
const audioSel = $<HTMLSelectElement>('audio-track');
const subWrap = $<HTMLLabelElement>('sub-wrap');
const subSel = $<HTMLSelectElement>('sub-track');

const fileInput = $<HTMLInputElement>('file-input');
const folderInput = $<HTMLInputElement>('folder-input');
const subInput = $<HTMLInputElement>('sub-input');

const btnShuffle = $<HTMLButtonElement>('btn-shuffle');
const btnRepeat = $<HTMLButtonElement>('btn-repeat');

/* ------------------------------------------------------------------ *
 * State
 * ------------------------------------------------------------------ */
type RepeatMode = 'off' | 'all' | 'one';

const tracks: Track[] = [];
let current = -1;
let shuffle = false;
let repeat: RepeatMode = 'off';
let scrubbing = false;

const RESUME_KEY = 'larrez.resume.v1';
const PREFS_KEY = 'larrez.prefs.v1';

/* ------------------------------------------------------------------ *
 * Small helpers
 * ------------------------------------------------------------------ */
let toastTimer = 0;
function toast(msg: string, warn = false, ms = 3200): void {
  toastEl.textContent = msg;
  toastEl.classList.toggle('warn', warn);
  toastEl.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => { toastEl.hidden = true; }, ms);
}

function loadJSON<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? { ...fallback, ...(JSON.parse(raw) as object) } as T : fallback;
  } catch { return fallback; }
}

function saveJSON(key: string, value: unknown): void {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* quota */ }
}

/** Resume positions keyed by "name|size" so the same file re-opens where you left it. */
function resumeKeyFor(t: Track): string {
  return `${t.name}|${t.size}`;
}

function readResume(): Record<string, number> {
  return loadJSON<Record<string, number>>(RESUME_KEY, {});
}

function writeResume(t: Track, seconds: number): void {
  const all = readResume();
  const dur = video.duration;
  // Don't remember trivial starts or near-finished playback.
  if (!isFinite(dur) || seconds < 15 || seconds > dur - 20) {
    delete all[resumeKeyFor(t)];
  } else {
    all[resumeKeyFor(t)] = seconds;
  }
  const keys = Object.keys(all);
  if (keys.length > 200) delete all[keys[0]];
  saveJSON(RESUME_KEY, all);
}

/* ------------------------------------------------------------------ *
 * Playlist rendering
 * ------------------------------------------------------------------ */
let dragFrom = -1;

function renderPlaylist(): void {
  playlistEl.replaceChildren();

  if (tracks.length === 0) {
    const li = document.createElement('li');
    li.className = 'pl-empty';
    li.textContent = 'Playlist is empty. Drop files onto the player to get started.';
    playlistEl.append(li);
    updateNavButtons();
    return;
  }

  const frag = document.createDocumentFragment();

  tracks.forEach((t, i) => {
    const li = document.createElement('li');
    li.className = i === current ? 'active' : '';
    li.draggable = true;
    li.dataset.index = String(i);
    li.title = `${t.name}\n${t.info.note}`;

    const idx = document.createElement('span');
    idx.className = 'pl-idx';
    idx.textContent = String(i + 1);

    const body = document.createElement('div');
    body.className = 'pl-body';

    const name = document.createElement('span');
    name.className = 'pl-name';
    name.textContent = t.name;

    const meta = document.createElement('span');
    meta.className = 'pl-meta';
    meta.textContent = subtitleLine(t) + (t.duration ? ` · ${fmtTime(t.duration)}` : '');

    body.append(name, meta);

    const x = document.createElement('button');
    x.className = 'pl-x';
    x.textContent = '✕';
    x.title = 'Remove from playlist';
    x.addEventListener('click', (e) => { e.stopPropagation(); removeAt(i); });

    li.append(idx, body, x);
    li.addEventListener('click', () => play(i));

    li.addEventListener('dragstart', () => { dragFrom = i; });
    li.addEventListener('dragover', (e) => { e.preventDefault(); li.classList.add('dragover'); });
    li.addEventListener('dragleave', () => li.classList.remove('dragover'));
    li.addEventListener('drop', (e) => {
      e.preventDefault();
      e.stopPropagation();
      li.classList.remove('dragover');
      reorder(dragFrom, i);
    });

    frag.append(li);
  });

  playlistEl.append(frag);
  updateNavButtons();
}

function reorder(from: number, to: number): void {
  if (from < 0 || to < 0 || from === to || from >= tracks.length) return;
  const [moved] = tracks.splice(from, 1);
  tracks.splice(to, 0, moved);
  if (current === from) current = to;
  else if (from < current && to >= current) current--;
  else if (from > current && to <= current) current++;
  dragFrom = -1;
  renderPlaylist();
}

function removeAt(i: number): void {
  const [gone] = tracks.splice(i, 1);
  if (gone?.file) URL.revokeObjectURL(gone.src);

  if (i === current) {
    if (tracks.length === 0) {
      current = -1;
      unload();
    } else {
      current = Math.min(i, tracks.length - 1);
      play(current);
    }
  } else if (i < current) {
    current--;
  }
  renderPlaylist();
}

function updateNavButtons(): void {
  btnPrev.disabled = tracks.length < 2;
  btnNext.disabled = tracks.length < 2;
}

/* ------------------------------------------------------------------ *
 * Adding media
 * ------------------------------------------------------------------ */
function addFiles(files: File[]): void {
  const vids = files.filter(isVideoFile);
  const skipped = files.length - vids.length;
  if (vids.length === 0) {
    toast(skipped ? 'No playable video files in that selection.' : 'Nothing to add.', true);
    return;
  }

  const startEmpty = tracks.length === 0;
  const added = vids.map(trackFromFile).sort(sortByName);
  tracks.push(...added);
  renderPlaylist();

  const bad = added.filter((t) => t.info.support === 'unsupported').length;
  let msg = `Added ${added.length} file${added.length > 1 ? 's' : ''}`;
  if (skipped) msg += ` (skipped ${skipped} non-video)`;
  if (bad) msg += ` — ${bad} need the native build`;
  toast(msg, bad > 0);

  if (startEmpty) play(0);
}

function addUrl(url: string): void {
  const t = trackFromUrl(url);
  const startEmpty = tracks.length === 0;
  tracks.push(t);
  renderPlaylist();
  if (startEmpty) play(tracks.length - 1);
  else toast(`Added ${t.name}`);
}

/* ------------------------------------------------------------------ *
 * Playback
 * ------------------------------------------------------------------ */
function unload(): void {
  video.removeAttribute('src');
  video.load();
  stage.classList.remove('has-media');
  nowPlaying.textContent = 'Nothing loaded';
  timeCur.textContent = timeDur.textContent = '0:00';
  seekFill.style.width = seekBuffer.style.width = '0%';
  seekKnob.style.left = '0%';
  setPlayIcon(false);
  perfEl.textContent = 'idle';
}

function play(i: number): void {
  const t = tracks[i];
  if (!t) return;

  // Persist the outgoing track's position before switching away.
  const prev = tracks[current];
  if (prev && video.currentTime > 0) writeResume(prev, video.currentTime);

  current = i;
  clearSubtitleTracks();

  video.src = t.src;
  video.load();
  stage.classList.add('has-media');
  nowPlaying.textContent = t.name;
  document.title = `${t.name} — Larrez Player`;
  renderPlaylist();

  if (t.info.support === 'unsupported') toast(t.info.note, true, 5200);

  video.play().catch(() => {
    // Autoplay policy or an undecodable file; the error handler covers the latter.
    setPlayIcon(false);
  });
}

function next(auto = false): void {
  if (tracks.length === 0) return;

  if (auto && repeat === 'one') {
    video.currentTime = 0;
    void video.play();
    return;
  }

  if (shuffle && tracks.length > 1) {
    let n = current;
    while (n === current) n = Math.floor(Math.random() * tracks.length);
    play(n);
    return;
  }

  const last = current >= tracks.length - 1;
  if (last && auto && repeat === 'off') {
    setPlayIcon(false);
    toast('End of playlist');
    return;
  }
  play(last ? 0 : current + 1);
}

function prev(): void {
  if (tracks.length === 0) return;
  // Standard behaviour: restart the track if we're past the 3s mark.
  if (video.currentTime > 3) { video.currentTime = 0; return; }
  play(current <= 0 ? tracks.length - 1 : current - 1);
}

function togglePlay(): void {
  if (!video.src) { fileInput.click(); return; }
  if (video.paused) void video.play(); else video.pause();
}

function setPlayIcon(playing: boolean): void {
  btnPlay.textContent = playing ? '❚❚' : '▶';
}

function nudge(sec: number): void {
  if (!video.src || !isFinite(video.duration)) return;
  video.currentTime = Math.min(Math.max(0, video.currentTime + sec), video.duration);
  toast(`${sec > 0 ? '+' : ''}${sec}s`, false, 700);
}

/* ------------------------------------------------------------------ *
 * Seek bar
 * ------------------------------------------------------------------ */
function ratioFromEvent(e: PointerEvent | MouseEvent): number {
  const r = seek.getBoundingClientRect();
  return Math.min(Math.max((e.clientX - r.left) / r.width, 0), 1);
}

function paintProgress(): void {
  const d = video.duration;
  if (!isFinite(d) || d <= 0) return;
  const pct = (video.currentTime / d) * 100;
  seekFill.style.width = `${pct}%`;
  seekKnob.style.left = `${pct}%`;
  timeCur.textContent = fmtTime(video.currentTime);
  seek.setAttribute('aria-valuenow', String(Math.round(pct)));

  if (video.buffered.length) {
    const end = video.buffered.end(video.buffered.length - 1);
    seekBuffer.style.width = `${Math.min((end / d) * 100, 100)}%`;
  }
}

seek.addEventListener('pointerdown', (e) => {
  if (!isFinite(video.duration)) return;
  scrubbing = true;
  seek.classList.add('scrub');
  seek.setPointerCapture(e.pointerId);
  video.currentTime = ratioFromEvent(e) * video.duration;
  paintProgress();
});

seek.addEventListener('pointermove', (e) => {
  if (!isFinite(video.duration)) return;
  const r = ratioFromEvent(e);

  seekTip.hidden = false;
  seekTip.textContent = fmtTime(r * video.duration);
  seekTip.style.left = `${r * 100}%`;

  if (scrubbing) {
    video.currentTime = r * video.duration;
    paintProgress();
  }
});

seek.addEventListener('pointerleave', () => { seekTip.hidden = true; });

seek.addEventListener('pointerup', (e) => {
  scrubbing = false;
  seek.classList.remove('scrub');
  seek.releasePointerCapture(e.pointerId);
});

seek.addEventListener('keydown', (e) => {
  if (e.key === 'ArrowLeft') { nudge(-5); e.preventDefault(); }
  if (e.key === 'ArrowRight') { nudge(5); e.preventDefault(); }
});

/* ------------------------------------------------------------------ *
 * Frame-synced UI updates
 *
 * requestVideoFrameCallback fires only when a new frame is actually
 * presented, so a paused or offscreen video costs us nothing. We fall
 * back to timeupdate (~4Hz) where it isn't available.
 * ------------------------------------------------------------------ */
interface VideoFrameMeta {
  presentedFrames: number;
  width: number;
  height: number;
}
type RVFCVideo = HTMLVideoElement & {
  requestVideoFrameCallback?: (cb: (now: number, meta: VideoFrameMeta) => void) => number;
};

const rvfcVideo = video as RVFCVideo;
const hasRVFC = typeof rvfcVideo.requestVideoFrameCallback === 'function';

let lastFrames = 0;
let lastStamp = performance.now();
let fps = 0;

function onFrame(_now: number, meta: VideoFrameMeta): void {
  if (!scrubbing) paintProgress();

  const t = performance.now();
  if (t - lastStamp >= 1000) {
    fps = ((meta.presentedFrames - lastFrames) * 1000) / (t - lastStamp);
    lastFrames = meta.presentedFrames;
    lastStamp = t;
    paintPerf(meta.width, meta.height);
  }
  rvfcVideo.requestVideoFrameCallback?.(onFrame);
}

function paintPerf(w: number, h: number): void {
  const q = video.getVideoPlaybackQuality?.();
  const dropped = q ? q.droppedVideoFrames : 0;
  perfEl.textContent =
    `${w}×${h} · ${fps.toFixed(0)} fps\n` +
    `dropped ${dropped} · buffer ${bufferAhead().toFixed(1)}s`;
}

function bufferAhead(): number {
  const b = video.buffered;
  for (let i = 0; i < b.length; i++) {
    if (video.currentTime >= b.start(i) && video.currentTime <= b.end(i)) {
      return b.end(i) - video.currentTime;
    }
  }
  return 0;
}

if (hasRVFC) {
  rvfcVideo.requestVideoFrameCallback?.(onFrame);
} else {
  video.addEventListener('timeupdate', () => { if (!scrubbing) paintProgress(); });
}

/* ------------------------------------------------------------------ *
 * Track (audio / subtitle) menus
 * ------------------------------------------------------------------ */
interface AudioTrackLike { id: string; label: string; language: string; enabled: boolean; }
interface AudioTrackListLike { length: number; [i: number]: AudioTrackLike; onchange: (() => void) | null; }

function refreshAudioTracks(): void {
  const list = (video as unknown as { audioTracks?: AudioTrackListLike }).audioTracks;
  if (!list || list.length < 2) { audioWrap.hidden = true; return; }

  audioSel.replaceChildren();
  for (let i = 0; i < list.length; i++) {
    const tr = list[i];
    const o = document.createElement('option');
    o.value = String(i);
    o.textContent = tr.label || tr.language || `Track ${i + 1}`;
    if (tr.enabled) o.selected = true;
    audioSel.append(o);
  }
  audioWrap.hidden = false;
}

audioSel.addEventListener('change', () => {
  const list = (video as unknown as { audioTracks?: AudioTrackListLike }).audioTracks;
  if (!list) return;
  const pick = Number(audioSel.value);
  for (let i = 0; i < list.length; i++) list[i].enabled = i === pick;
});

const addedSubUrls: string[] = [];

function clearSubtitleTracks(): void {
  video.querySelectorAll('track').forEach((el) => el.remove());
  addedSubUrls.splice(0).forEach(URL.revokeObjectURL);
  subWrap.hidden = true;
}

function refreshSubtitleMenu(): void {
  const tt = video.textTracks;
  if (tt.length === 0) { subWrap.hidden = true; return; }

  subSel.replaceChildren();
  const off = document.createElement('option');
  off.value = '-1';
  off.textContent = 'Off';
  subSel.append(off);

  for (let i = 0; i < tt.length; i++) {
    const o = document.createElement('option');
    o.value = String(i);
    o.textContent = tt[i].label || tt[i].language || `Subtitle ${i + 1}`;
    if (tt[i].mode === 'showing') o.selected = true;
    subSel.append(o);
  }
  subWrap.hidden = false;
}

subSel.addEventListener('change', () => {
  const pick = Number(subSel.value);
  const tt = video.textTracks;
  for (let i = 0; i < tt.length; i++) tt[i].mode = i === pick ? 'showing' : 'disabled';
});

async function loadSubtitleFile(file: File): Promise<void> {
  const text = await file.text();
  const isSrt = /\.srt$/i.test(file.name);
  const vtt = isSrt ? srtToVtt(text) : text;
  const url = URL.createObjectURL(new Blob([vtt], { type: 'text/vtt' }));
  addedSubUrls.push(url);

  const el = document.createElement('track');
  el.kind = 'subtitles';
  el.label = file.name.replace(/\.(vtt|srt)$/i, '');
  el.src = url;
  el.default = true;
  video.append(el);

  el.addEventListener('load', () => {
    const tt = video.textTracks;
    for (let i = 0; i < tt.length; i++) tt[i].mode = i === tt.length - 1 ? 'showing' : 'disabled';
    refreshSubtitleMenu();
  });

  toast(`Subtitles loaded: ${el.label}`);
}

/* ------------------------------------------------------------------ *
 * Video element events
 * ------------------------------------------------------------------ */
video.addEventListener('loadedmetadata', () => {
  timeDur.textContent = fmtTime(video.duration);
  const t = tracks[current];
  if (t) {
    t.duration = video.duration;
    const saved = readResume()[resumeKeyFor(t)];
    if (saved && saved < video.duration - 20) {
      video.currentTime = saved;
      toast(`Resumed at ${fmtTime(saved)}`);
    }
    renderPlaylist();
  }
  refreshAudioTracks();
  refreshSubtitleMenu();
});

video.addEventListener('play', () => setPlayIcon(true));
video.addEventListener('pause', () => {
  setPlayIcon(false);
  const t = tracks[current];
  if (t) writeResume(t, video.currentTime);
});

video.addEventListener('ended', () => {
  const t = tracks[current];
  if (t) {
    const all = readResume();
    delete all[resumeKeyFor(t)];
    saveJSON(RESUME_KEY, all);
  }
  next(true);
});

video.addEventListener('error', () => {
  const t = tracks[current];
  if (!t) return;
  const detail = t.info.support === 'native'
    ? 'The file may be corrupt, or it uses a codec this browser lacks (e.g. HEVC).'
    : t.info.note;
  toast(`Can't decode "${t.name}". ${detail}`, true, 6000);
  setPlayIcon(false);
});

video.addEventListener('volumechange', () => {
  btnMute.textContent = video.muted || video.volume === 0 ? '🔇' : video.volume < 0.5 ? '🔉' : '🔊';
  volume.value = String(video.muted ? 0 : video.volume);
  savePrefs();
});

video.addEventListener('click', togglePlay);
video.addEventListener('dblclick', () => void toggleFullscreen());

/* ------------------------------------------------------------------ *
 * Controls wiring
 * ------------------------------------------------------------------ */
btnPlay.addEventListener('click', togglePlay);
btnPrev.addEventListener('click', prev);
btnNext.addEventListener('click', () => next(false));
$('btn-back').addEventListener('click', () => nudge(-10));
$('btn-fwd').addEventListener('click', () => nudge(10));
btnMute.addEventListener('click', () => { video.muted = !video.muted; });

volume.addEventListener('input', () => {
  video.volume = Number(volume.value);
  video.muted = video.volume === 0;
});

speed.addEventListener('change', () => {
  video.playbackRate = Number(speed.value);
  savePrefs();
});

$('btn-open').addEventListener('click', () => fileInput.click());
$('btn-open-folder').addEventListener('click', () => folderInput.click());
$('btn-sub-add').addEventListener('click', () => subInput.click());

$('btn-open-url').addEventListener('click', () => {
  const url = prompt('Video URL (direct file, HLS .m3u8, or DASH .mpd):');
  if (url?.trim()) addUrl(url.trim());
});

fileInput.addEventListener('change', () => {
  addFiles([...(fileInput.files ?? [])]);
  fileInput.value = '';
});

folderInput.addEventListener('change', () => {
  addFiles([...(folderInput.files ?? [])]);
  folderInput.value = '';
});

subInput.addEventListener('change', () => {
  const f = subInput.files?.[0];
  if (f) void loadSubtitleFile(f);
  subInput.value = '';
});

btnShuffle.addEventListener('click', () => {
  shuffle = !shuffle;
  btnShuffle.classList.toggle('on', shuffle);
  toast(`Shuffle ${shuffle ? 'on' : 'off'}`, false, 1200);
  savePrefs();
});

btnRepeat.addEventListener('click', () => {
  repeat = repeat === 'off' ? 'all' : repeat === 'all' ? 'one' : 'off';
  btnRepeat.textContent = `Repeat: ${repeat}`;
  btnRepeat.classList.toggle('on', repeat !== 'off');
  savePrefs();
});

$('btn-clear').addEventListener('click', () => {
  tracks.forEach((t) => { if (t.file) URL.revokeObjectURL(t.src); });
  tracks.length = 0;
  current = -1;
  unload();
  renderPlaylist();
});

/* ---- Fullscreen / PiP / snapshot ---- */
async function toggleFullscreen(): Promise<void> {
  try {
    if (document.fullscreenElement) await document.exitFullscreen();
    else await stage.requestFullscreen();
  } catch { toast('Fullscreen was blocked by the browser.', true); }
}
$('btn-full').addEventListener('click', () => void toggleFullscreen());

$('btn-pip').addEventListener('click', async () => {
  try {
    if (document.pictureInPictureElement) await document.exitPictureInPicture();
    else if (video.src) await video.requestPictureInPicture();
  } catch { toast('Picture-in-picture is unavailable for this video.', true); }
});

function snapshot(): void {
  if (!video.videoWidth) { toast('Nothing to capture yet.', true); return; }
  const c = document.createElement('canvas');
  c.width = video.videoWidth;
  c.height = video.videoHeight;
  c.getContext('2d')?.drawImage(video, 0, 0);
  c.toBlob((blob) => {
    if (!blob) { toast('Snapshot failed (the source may be cross-origin).', true); return; }
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    const base = (tracks[current]?.name ?? 'larrez').replace(/\.[^.]+$/, '');
    a.download = `${base}_${Math.floor(video.currentTime)}s.png`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 5000);
    toast('Snapshot saved');
  }, 'image/png');
}
$('btn-snap').addEventListener('click', snapshot);

/* ------------------------------------------------------------------ *
 * Drag & drop
 * ------------------------------------------------------------------ */
['dragenter', 'dragover'].forEach((ev) =>
  stage.addEventListener(ev, (e) => {
    e.preventDefault();
    stage.classList.add('dragging');
  }),
);

['dragleave', 'drop'].forEach((ev) =>
  stage.addEventListener(ev, (e) => {
    e.preventDefault();
    if (ev === 'drop' || (e as DragEvent).relatedTarget === null) {
      stage.classList.remove('dragging');
    }
  }),
);

stage.addEventListener('drop', async (e) => {
  const dt = (e as DragEvent).dataTransfer;
  if (!dt) return;

  const subs = [...dt.files].filter((f) => /\.(srt|vtt)$/i.test(f.name));
  if (subs.length && video.src) { await loadSubtitleFile(subs[0]); }

  const files = await collectFiles(dt);
  if (files.length) addFiles(files);
});

/** Walk dropped directory entries so dropping a folder works, not just files. */
async function collectFiles(dt: DataTransfer): Promise<File[]> {
  const entries = [...dt.items]
    .map((it) => (it.webkitGetAsEntry?.() ?? null))
    .filter((e): e is FileSystemEntry => e !== null);

  if (entries.length === 0) return [...dt.files];

  const out: File[] = [];
  const walk = async (entry: FileSystemEntry): Promise<void> => {
    if (entry.isFile) {
      const file = await new Promise<File | null>((res) =>
        (entry as FileSystemFileEntry).file(res, () => res(null)),
      );
      if (file) out.push(file);
      return;
    }
    if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      // readEntries returns at most 100 per call, so drain it.
      for (;;) {
        const batch = await new Promise<FileSystemEntry[]>((res) =>
          reader.readEntries(res, () => res([])),
        );
        if (batch.length === 0) break;
        for (const child of batch) await walk(child);
      }
    }
  };

  await Promise.all(entries.map(walk));
  return out;
}

/* ------------------------------------------------------------------ *
 * Keyboard shortcuts
 * ------------------------------------------------------------------ */
document.addEventListener('keydown', (e) => {
  const target = e.target as HTMLElement | null;
  if (target && /^(INPUT|SELECT|TEXTAREA)$/.test(target.tagName)) return;
  if (e.ctrlKey || e.altKey || e.metaKey) return;

  switch (e.key.toLowerCase()) {
    case ' ': case 'k': togglePlay(); break;
    case 'arrowleft': nudge(-5); break;
    case 'arrowright': nudge(5); break;
    case 'j': nudge(-10); break;
    case 'l': nudge(10); break;
    case 'arrowup': video.volume = Math.min(1, video.volume + 0.05); video.muted = false; break;
    case 'arrowdown': video.volume = Math.max(0, video.volume - 0.05); break;
    case 'm': video.muted = !video.muted; break;
    case 'f': void toggleFullscreen(); break;
    case 'i': $('btn-pip').click(); break;
    case 'c': snapshot(); break;
    case 'n': next(false); break;
    case 'p': prev(); break;
    case 's': btnShuffle.click(); break;
    case 'r': btnRepeat.click(); break;
    case 'o': fileInput.click(); break;
    default: return;
  }
  e.preventDefault();
});

/* ------------------------------------------------------------------ *
 * Idle-hide the controls (and the cursor) during playback
 * ------------------------------------------------------------------ */
let idleTimer = 0;
function wake(): void {
  stage.classList.remove('idle', 'hide-cursor');
  clearTimeout(idleTimer);
  idleTimer = window.setTimeout(() => {
    if (!video.paused && video.src) stage.classList.add('idle', 'hide-cursor');
  }, 2600);
}
stage.addEventListener('pointermove', wake);
stage.addEventListener('pointerdown', wake);
video.addEventListener('pause', wake);

/* ------------------------------------------------------------------ *
 * Preferences
 * ------------------------------------------------------------------ */
interface Prefs { volume: number; muted: boolean; rate: number; shuffle: boolean; repeat: RepeatMode; }
const DEFAULT_PREFS: Prefs = { volume: 1, muted: false, rate: 1, shuffle: false, repeat: 'off' };

function savePrefs(): void {
  saveJSON(PREFS_KEY, {
    volume: video.volume,
    muted: video.muted,
    rate: video.playbackRate,
    shuffle,
    repeat,
  } satisfies Prefs);
}

function restorePrefs(): void {
  const p = loadJSON<Prefs>(PREFS_KEY, DEFAULT_PREFS);
  video.volume = p.volume;
  video.muted = p.muted;
  video.playbackRate = p.rate;
  speed.value = String(p.rate);
  shuffle = p.shuffle;
  repeat = p.repeat;
  btnShuffle.classList.toggle('on', shuffle);
  btnRepeat.textContent = `Repeat: ${repeat}`;
  btnRepeat.classList.toggle('on', repeat !== 'off');
}

window.addEventListener('beforeunload', () => {
  const t = tracks[current];
  if (t && video.currentTime > 0) writeResume(t, video.currentTime);
});

/* ------------------------------------------------------------------ *
 * Boot
 * ------------------------------------------------------------------ */
restorePrefs();
renderPlaylist();
unload();
wake();
