// The page: starts the pond (pond-worker.js) and its player (pond-worklet.js),
// then draws the pond from above, with you on the bank at the bottom looking
// up across the water, in step with what you hear.

// The pond's layout, as in src/scene.rs, and the area drawn, as in src/visual.rs.
const POND_CENTER = { x: 0, y: 24 };
const POND_RADII = { x: 34, y: 21 };
const BANK_MIN = { x: -9, y: -3 };
const BANK_MAX = { x: 9, y: 4.5 };
const VIEW_MIN = { x: -38, y: -6 };
const VIEW_MAX = { x: 38, y: 47 };
const RINGS = [5, 10, 20, 40];
/** Peas land within this bearing of straight ahead, 2 to 7 m out. */
const THROW = (35 * Math.PI) / 180;

const COLORS = {
  hen: '#f5c84c',
  drake: '#93d36c',
  duckling: '#f39a3c',
  pea: '#b8ef6c',
  call: '#ee80d2',
  bill: '#e8893a',
  outline: 'rgba(8, 16, 12, 0.8)',
};
const TAU = 2 * Math.PI;
const HEADPHONES_KEY = 'duck-pond:headphones';

const $ = (id) => document.getElementById(id);
const canvas = $('map');
const g = canvas.getContext('2d');
const peasButton = $('peas');
const headphonesButton = $('headphones');
const pauseButton = $('pause');
const listenButton = $('listen');
const introHeadphones = $('intro-headphones');

let audio = null;
let worker = null;
let headphones = false;
try {
  headphones = localStorage.getItem(HEADPHONES_KEY) === '1';
} catch {}

/** When each block starts being heard, as told by the worklet, and the scene
 *  at the start of each block, as told by the worker. */
const starts = [];
const snapshots = [];
const QUEUE_CAP = 2000;
let heardFrame = -1;
let current = null;
let selected = null;

// --- starting up ------------------------------------------------------------

/** Two 32-bit halves of a seed: `?seed=N` for a repeatable pond, else random. */
function seed() {
  const param = new URLSearchParams(location.search).get('seed');
  if (param !== null && /^\d+$/.test(param)) {
    const s = BigInt.asUintN(64, BigInt(param));
    return [Number(s >> 32n), Number(s & 0xffffffffn)];
  }
  return Array.from(crypto.getRandomValues(new Uint32Array(2)));
}

function showError(message) {
  const error = $('intro-error');
  error.textContent = message;
  error.hidden = false;
  $('intro').hidden = false;
  listenButton.disabled = false;
}

async function start() {
  listenButton.disabled = true;
  try {
    if (!window.isSecureContext || !window.AudioWorkletNode) {
      throw new Error("This browser can't play the pond here: it needs a secure (https://) page with Web Audio worklets.");
    }
    // play even with an iPhone's ringer switched off
    if (navigator.audioSession) navigator.audioSession.type = 'playback';
    // created and resumed straight away, while the tap still counts
    audio = new AudioContext({ latencyHint: 'playback' });
    audio.resume();
    await audio.audioWorklet.addModule(new URL('pond-worklet.js', import.meta.url));
    const node = new AudioWorkletNode(audio, 'pond-player', {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [2],
    });
    node.connect(audio.destination);
    node.port.onmessage = ({ data }) => push(starts, data);

    worker = new Worker(new URL('pond-worker.js', import.meta.url));
    worker.onmessage = ({ data }) => {
      if (data.type === 'snapshot') push(snapshots, data.snapshot);
      else if (data.type === 'error') showError(`The pond couldn't start: ${data.message}`);
    };
    worker.onerror = (e) => showError(`The pond couldn't start: ${e.message}`);
    const feed = new MessageChannel();
    node.port.postMessage(feed.port1, [feed.port1]);
    worker.postMessage(
      { type: 'start', feed: feed.port2, sampleRate: audio.sampleRate, seed: seed(), headphones },
      [feed.port2],
    );

    audio.onstatechange = updatePause;
    updatePause();
    $('intro').hidden = true;
    peasButton.disabled = false;
    pauseButton.disabled = false;
  } catch (err) {
    showError(err.message ?? String(err));
  }
}

function push(queue, item) {
  queue.push(item);
  // nothing drains these while the page is hidden
  if (queue.length > QUEUE_CAP) queue.shift();
}

// --- controls ---------------------------------------------------------------

function throwPeas() {
  worker?.postMessage({ type: 'peas' });
}

function setHeadphones(on) {
  headphones = on;
  try {
    localStorage.setItem(HEADPHONES_KEY, on ? '1' : '0');
  } catch {}
  headphonesButton.setAttribute('aria-pressed', String(on));
  introHeadphones.checked = on;
  worker?.postMessage({ type: 'headphones', on });
}

async function togglePause() {
  if (!audio) return;
  if (audio.state === 'running') await audio.suspend();
  else await audio.resume();
}

function updatePause() {
  const paused = audio.state !== 'running';
  pauseButton.classList.toggle('paused', paused);
  pauseButton.setAttribute('aria-label', paused ? 'Play' : 'Pause');
  pauseButton.title = paused ? 'Play (space)' : 'Pause (space)';
}

listenButton.addEventListener('click', start);
introHeadphones.addEventListener('change', () => setHeadphones(introHeadphones.checked));
peasButton.addEventListener('click', throwPeas);
headphonesButton.addEventListener('click', () => setHeadphones(!headphones));
pauseButton.addEventListener('click', togglePause);
setHeadphones(headphones);

addEventListener('keydown', (e) => {
  if (e.ctrlKey || e.metaKey || e.altKey || e.repeat) return;
  const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLButtonElement;
  if (e.key === 'p' || e.key === 'P') throwPeas();
  else if (e.key === 'h' || e.key === 'H') setHeadphones(!headphones);
  else if (e.key === ' ' && !typing) {
    e.preventDefault();
    togglePause();
  }
});

// --- keeping time with the audio --------------------------------------------

/** Context time of the sound coming out of the speakers right now. */
function heardTime() {
  const stamp = audio.getOutputTimestamp?.();
  if (stamp && stamp.contextTime > 0 && stamp.performanceTime > 0) {
    const since = audio.state === 'running' ? (performance.now() - stamp.performanceTime) / 1000 : 0;
    return stamp.contextTime + Math.min(Math.max(since, 0), 0.1);
  }
  return audio.currentTime - (audio.outputLatency || audio.baseLatency || 0);
}

function catchUp() {
  const now = heardTime();
  while (starts.length && starts[0].time <= now) heardFrame = starts.shift().frame;
  while (snapshots.length && snapshots[0].frame <= heardFrame) current = snapshots.shift();
}

// --- which way the ducks face -----------------------------------------------

/** Per duck: where it was last frame, its smoothed velocity and heading. */
const motion = new Map();

function track(snap, dt) {
  for (const d of snap.ducks) {
    let m = motion.get(d.name);
    if (!m) {
      m = { x: d.x, y: d.y, vx: 0, vy: 0, angle: Math.PI / 2 };
      motion.set(d.name, m);
    }
    if (dt > 0) {
      const k = Math.min(1, dt * 3);
      m.vx += ((d.x - m.x) / dt - m.vx) * k;
      m.vy += ((d.y - m.y) / dt - m.vy) * k;
    }
    m.x = d.x;
    m.y = d.y;
    if (Math.hypot(m.vx, m.vy) > 0.06) {
      let turn = Math.atan2(m.vy, m.vx) - m.angle;
      turn = ((turn + 3 * Math.PI) % TAU) - Math.PI;
      m.angle += turn * Math.min(1, dt * 6);
    }
  }
}

// --- the map ----------------------------------------------------------------

let dpr = 1;
let width = 0;
let height = 0;
let scale = 1;
let background = null;
const mid = { x: (VIEW_MIN.x + VIEW_MAX.x) / 2, y: (VIEW_MIN.y + VIEW_MAX.y) / 2 };

const sx = (x) => width / 2 + (x - mid.x) * scale;
const sy = (y) => height / 2 - (y - mid.y) * scale;

function layout() {
  const rect = canvas.getBoundingClientRect();
  dpr = window.devicePixelRatio || 1;
  width = rect.width;
  height = rect.height;
  canvas.width = Math.max(1, Math.round(width * dpr));
  canvas.height = Math.max(1, Math.round(height * dpr));
  scale = Math.min(width / (VIEW_MAX.x - VIEW_MIN.x), height / (VIEW_MAX.y - VIEW_MIN.y));
  background = paintBackground();
}

/** Small deterministic noise, so the grass and ripples stay put. */
function hash(i, j, k = 0) {
  let h = (i * 374761393 + j * 668265263 + k * 2147483647) | 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
}

function paintBackground() {
  const bg = document.createElement('canvas');
  bg.width = canvas.width;
  bg.height = canvas.height;
  const b = bg.getContext('2d');
  b.setTransform(dpr, 0, 0, dpr, 0, 0);

  // grass, with tufts on a world grid so they stay put as the window changes
  b.fillStyle = '#16201a';
  b.fillRect(0, 0, width, height);
  const step = 2.4;
  const [x0, x1] = [mid.x - width / 2 / scale, mid.x + width / 2 / scale];
  const [y0, y1] = [mid.y - height / 2 / scale, mid.y + height / 2 / scale];
  b.lineWidth = 1;
  b.lineCap = 'round';
  for (let i = Math.floor(x0 / step); i <= x1 / step; i++) {
    for (let j = Math.floor(y0 / step); j <= y1 / step; j++) {
      if (hash(i, j) < 0.45) continue;
      const x = sx((i + hash(i, j, 1)) * step);
      const y = sy((j + hash(i, j, 2)) * step);
      const h = 3 + 3 * hash(i, j, 3);
      b.strokeStyle = hash(i, j, 4) < 0.5 ? '#26372a' : '#2f4430';
      b.beginPath();
      b.moveTo(x - 1.5, y);
      b.lineTo(x - 2.5, y - h);
      b.moveTo(x + 1.5, y);
      b.lineTo(x + 2.5, y - h * 0.9);
      b.stroke();
    }
  }

  // the bank by the listener, muddy and trodden
  const bx = sx(BANK_MIN.x);
  const by = sy(BANK_MAX.y);
  const bw = (BANK_MAX.x - BANK_MIN.x) * scale;
  const bh = (BANK_MAX.y - BANK_MIN.y) * scale;
  b.fillStyle = '#3a3326';
  b.beginPath();
  b.roundRect(bx, by, bw, bh, Math.min(10, scale));
  b.fill();
  b.fillStyle = '#4a4232';
  for (let k = 0; k < 140; k++) {
    b.fillRect(bx + hash(k, 7) * bw, by + hash(k, 9) * bh, 1.5, 1.5);
  }

  // the pond: deeper towards the middle, shallow and light along the shore
  const cx = sx(POND_CENTER.x);
  const cy = sy(POND_CENTER.y);
  const rx = POND_RADII.x * scale;
  const ry = POND_RADII.y * scale;
  b.save();
  b.translate(cx, cy);
  b.scale(1, ry / rx);
  const water = b.createRadialGradient(0, 0, 0, 0, 0, rx);
  water.addColorStop(0, '#2f6a91');
  water.addColorStop(0.75, '#3f80a8');
  water.addColorStop(1, '#5a9ac0');
  b.fillStyle = water;
  b.beginPath();
  b.arc(0, 0, rx, 0, TAU);
  b.fill();
  b.restore();
  b.strokeStyle = 'rgba(190, 225, 240, 0.35)';
  b.lineWidth = 1.5;
  b.beginPath();
  b.ellipse(cx, cy, rx, ry, 0, 0, TAU);
  b.stroke();

  // ripples
  b.strokeStyle = 'rgba(210, 235, 250, 0.16)';
  b.lineWidth = 1;
  const rippleStep = 3.2;
  for (let i = -12; i <= 12; i++) {
    for (let j = 0; j <= 15; j++) {
      const wx = (i + hash(i, j, 5)) * rippleStep;
      const wy = (j + hash(i, j, 6)) * rippleStep;
      const dx = (wx - POND_CENTER.x) / (POND_RADII.x - 1.5);
      const dy = (wy - POND_CENTER.y) / (POND_RADII.y - 1.5);
      if (dx * dx + dy * dy > 1 || hash(i, j, 7) < 0.5) continue;
      const x = sx(wx);
      const y = sy(wy);
      const w = 3 + 3 * hash(i, j, 8);
      b.beginPath();
      b.moveTo(x - w, y);
      b.quadraticCurveTo(x - w / 2, y - 2.5, x, y);
      b.quadraticCurveTo(x + w / 2, y + 2.5, x + w, y);
      b.stroke();
    }
  }

  // where the peas land
  const ox = sx(0);
  const oy = sy(0);
  const cone = b.createRadialGradient(ox, oy, 0, ox, oy, 8 * scale);
  cone.addColorStop(0, 'rgba(255, 255, 255, 0.10)');
  cone.addColorStop(1, 'rgba(255, 255, 255, 0)');
  b.fillStyle = cone;
  b.beginPath();
  b.moveTo(ox, oy);
  b.arc(ox, oy, 8 * scale, -Math.PI / 2 - THROW, -Math.PI / 2 + THROW);
  b.closePath();
  b.fill();

  // distance rings, labelled straight ahead
  b.setLineDash([3, 5]);
  b.strokeStyle = 'rgba(255, 255, 255, 0.2)';
  for (const r of RINGS) {
    b.beginPath();
    b.arc(ox, oy, r * scale, 0, TAU);
    b.stroke();
  }
  b.setLineDash([]);
  b.font = '500 11px system-ui, sans-serif';
  b.textBaseline = 'bottom';
  for (const r of RINGS) {
    const text = `${r} m`;
    const x = ox + 4;
    const y = oy - r * scale - 2;
    b.lineWidth = 3;
    b.strokeStyle = 'rgba(20, 40, 50, 0.55)';
    b.strokeText(text, x, y);
    b.fillStyle = 'rgba(255, 255, 255, 0.72)';
    b.fillText(text, x, y);
  }

  // you
  b.fillStyle = '#fff';
  b.strokeStyle = COLORS.outline;
  b.lineWidth = 2;
  b.beginPath();
  b.arc(ox, oy, 5, 0, TAU);
  b.fill();
  b.stroke();
  b.beginPath();
  b.moveTo(ox, oy - 12);
  b.lineTo(ox - 4, oy - 7);
  b.lineTo(ox + 4, oy - 7);
  b.closePath();
  b.fill();
  b.font = '600 11px system-ui, sans-serif';
  b.textAlign = 'center';
  b.textBaseline = 'top';
  b.fillStyle = 'rgba(255, 255, 255, 0.85)';
  b.fillText('you', ox, oy + 8);
  return bg;
}

function duckLength(kind) {
  const adult = Math.max(12, 1.1 * scale);
  return kind === 'duckling' ? Math.max(8, adult * 0.6) : adult;
}

function drawDuck(d, angle, len, calling) {
  g.save();
  g.translate(sx(d.x), sy(d.y));
  g.rotate(-angle);
  g.lineWidth = 1.4;
  g.strokeStyle = calling ? COLORS.call : COLORS.outline;
  g.fillStyle = COLORS[d.kind];
  // body, with a tail
  g.beginPath();
  g.ellipse(-len * 0.08, 0, len * 0.42, len * 0.27, 0, 0, TAU);
  g.moveTo(-len * 0.42, -len * 0.12);
  g.lineTo(-len * 0.6, 0);
  g.lineTo(-len * 0.42, len * 0.12);
  g.fill();
  g.stroke();
  // head and bill
  g.beginPath();
  g.arc(len * 0.3, 0, len * 0.18, 0, TAU);
  g.fill();
  g.stroke();
  g.fillStyle = COLORS.bill;
  g.beginPath();
  g.ellipse(len * 0.52, 0, len * 0.11, len * 0.07, 0, 0, TAU);
  g.fill();
  g.restore();
}

/** Sound spreading out from a calling duck. */
function drawCallRings(d, len, now) {
  const x = sx(d.x);
  const y = sy(d.y);
  g.lineWidth = 1.5;
  for (let i = 0; i < 3; i++) {
    const p = ((now / 1000) * 1.4 + i / 3) % 1;
    g.strokeStyle = `rgba(238, 128, 210, ${(1 - p) * 0.75})`;
    g.beginPath();
    g.arc(x, y, len * 0.6 + p * len * 1.8, 0, TAU);
    g.stroke();
  }
}

/** A label `gap` to the right of (x, y), or to its left if there's no room
 *  there; centred on x if `gap` is 0. */
function drawLabel(x, y, text, color, gap) {
  g.font = '600 12px system-ui, sans-serif';
  const w = g.measureText(text).width + 12;
  const h = 18;
  let lx = gap > 0 ? x + gap : x - w / 2;
  if (gap > 0 && lx + w > width - 2) lx = x - gap - w;
  lx = Math.min(Math.max(2, lx), width - w - 2);
  const ly = Math.min(Math.max(2, y - h / 2), height - h - 2);
  g.fillStyle = 'rgba(18, 12, 20, 0.78)';
  g.beginPath();
  g.roundRect(lx, ly, w, h, 9);
  g.fill();
  g.fillStyle = color;
  g.textAlign = 'left';
  g.textBaseline = 'middle';
  g.fillText(text, lx + 6, ly + h / 2 + 0.5);
}

function draw(now) {
  g.setTransform(1, 0, 0, 1, 0, 0);
  if (background) g.drawImage(background, 0, 0);
  if (!current) return;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);

  const pea = Math.max(2.2, 0.14 * scale);
  g.fillStyle = COLORS.pea;
  g.strokeStyle = COLORS.outline;
  g.lineWidth = 1;
  for (const [x, y] of current.peas) {
    g.beginPath();
    g.arc(sx(x), sy(y), pea, 0, TAU);
    g.fill();
    g.stroke();
  }

  // adults on top of ducklings
  const ducks = [...current.ducks].sort((a, b) => (a.kind === 'duckling' ? 0 : 1) - (b.kind === 'duckling' ? 0 : 1));
  for (const d of ducks) {
    if (d.call) drawCallRings(d, duckLength(d.kind), now);
  }
  for (const d of ducks) {
    const len = duckLength(d.kind);
    if (d.name === selected) {
      g.strokeStyle = '#fff';
      g.lineWidth = 2;
      g.beginPath();
      g.arc(sx(d.x), sy(d.y), len * 0.85, 0, TAU);
      g.stroke();
    }
    drawDuck(d, motion.get(d.name)?.angle ?? Math.PI / 2, len, d.call !== null);
  }
  // what they are saying: the grown-ups, and a duckling only when it's lost
  for (const d of ducks) {
    const len = duckLength(d.kind);
    const x = sx(d.x);
    const y = sy(d.y);
    if (d.name === selected) drawLabel(x, y - len - 6, d.name, '#fff', 0);
    if (d.call && (d.kind !== 'duckling' || d.call === 'peep peep')) {
      drawLabel(x, y, d.call, COLORS.call, len * 0.8);
    }
  }
}

canvas.addEventListener('click', (e) => {
  if (!current) return;
  const rect = canvas.getBoundingClientRect();
  const [x, y] = [e.clientX - rect.left, e.clientY - rect.top];
  let best = null;
  let bestDist = 24;
  for (const d of current.ducks) {
    const dist = Math.hypot(sx(d.x) - x, sy(d.y) - y);
    if (dist < bestDist) [best, bestDist] = [d.name, dist];
  }
  select(best === selected ? null : best);
});

new ResizeObserver(layout).observe(canvas);

// --- the list of ducks ------------------------------------------------------

const list = $('ducks');
const rows = new Map();

function select(name) {
  selected = name;
  for (const [n, row] of rows) row.button.setAttribute('aria-pressed', String(n === name));
}

function makeRow(d) {
  const li = document.createElement('li');
  const button = document.createElement('button');
  button.className = 'duck';
  button.type = 'button';
  button.setAttribute('aria-pressed', 'false');
  button.innerHTML = `<i class="dot ${d.kind}"></i><span class="name"></span><span class="dist"></span><span class="bearing"></span><span class="doing"><span class="activity"></span><span class="call"></span></span>`;
  button.querySelector('.name').textContent = d.name;
  button.addEventListener('click', () => select(selected === d.name ? null : d.name));
  li.append(button);
  list.append(li);
  const field = (name) => ({ el: button.querySelector(`.${name}`), text: '' });
  return { li, button, order: -1, calling: false, dist: field('dist'), bearing: field('bearing'), activity: field('activity'), call: field('call') };
}

/** Change what's on the page only when it differs, as this runs every frame. */
function set(field, text) {
  if (field.text !== text) {
    field.text = text;
    field.el.textContent = text;
  }
}

let peaCount = -1;

function updateList(snap) {
  const ducks = snap.ducks.map((d) => ({ d, r: Math.hypot(d.x, d.y) })).sort((a, b) => a.r - b.r);
  ducks.forEach(({ d, r }, order) => {
    let row = rows.get(d.name);
    if (!row) {
      row = makeRow(d);
      rows.set(d.name, row);
    }
    if (row.order !== order) {
      row.order = order;
      row.li.style.order = order;
    }
    const bearing = (Math.atan2(d.x, d.y) * 180) / Math.PI;
    set(row.dist, `${r.toFixed(1)} m`);
    set(row.bearing, Math.abs(bearing) < 5 ? 'ahead' : `${Math.abs(bearing).toFixed(0)}° ${bearing < 0 ? 'L' : 'R'}`);
    set(row.activity, d.activity);
    set(row.call, d.call ?? '');
    if (row.calling !== (d.call !== null)) {
      row.calling = d.call !== null;
      row.button.classList.toggle('calling', row.calling);
    }
  });
  if (peaCount !== snap.peas.length) {
    peaCount = snap.peas.length;
    $('pea-count').textContent = peaCount;
  }
}

// --- every frame --------------------------------------------------------------

let last = performance.now();

function frame(now) {
  requestAnimationFrame(frame);
  if ((window.devicePixelRatio || 1) !== dpr) layout();
  const dt = Math.min((now - last) / 1000, 0.5);
  last = now;
  if (audio) catchUp();
  if (current) {
    track(current, dt);
    updateList(current);
  }
  draw(now);
}

layout();
requestAnimationFrame(frame);
