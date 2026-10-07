const { invoke } = window.__TAURI__.core;
const dialog = window.__TAURI__.dialog;

const PAL = ['#00a300','#2d89ef','#da532c','#b91d47','#9f00a7','#00aba9','#e3a21a','#7e3878'];
const FALLBACK_DEV = ['Устройство по умолчанию'];

let uid = 1;
let view = 'chain';
let sel = null;
let LIB = [];                      // приходит из Rust: scan_plugins()
let DIRS = [];                     // пользовательские папки сканирования
let chain = [];                    // [{id, name, vendor, format, path, off}]
let devices = [];      // реальные устройства, если WebView их отдал
let devIdx = 0;
let engineOn = false;

const $ = s => document.querySelector(s);
const main = $('#main');

// --- сохранение состояния: ничего не сбрасывается при перезапуске приложения ---
function save() {
  localStorage.setItem('winxer.state', JSON.stringify({ chain, devIdx, devices, devName: deviceName() }));
}
function load() {
  try {
    const s = JSON.parse(localStorage.getItem('winxer.state') || '{}');
    if (Array.isArray(s.chain)) { chain = s.chain; uid = chain.reduce((m, p) => Math.max(m, p.id), 0) + 1; }
    if (s.devices?.length) devices = s.devices;
    if (s.devName) { const i = devices.indexOf(s.devName); if (i >= 0) devIdx = i; }
  } catch { }
}
function presets() { try { return JSON.parse(localStorage.getItem('winxer.presets') || '{}'); } catch { return {}; } }
function savePresets(p) { localStorage.setItem('winxer.presets', JSON.stringify(p)); }

const colorOf = (() => {
  const map = new Map();
  let n = 0;
  return key => {
    if (!map.has(key)) map.set(key, PAL[n++ % PAL.length]);
    return map.get(key);
  };
})();

async function scan() {
  try {
    const [raw, dirs] = await Promise.all([invoke('scan_plugins'), invoke('list_custom_dirs')]);
    LIB = raw.map(p => ({ ...p, c: colorOf(p.path.toLowerCase()) }));
    DIRS = dirs;
  } catch (e) {
    LIB = [];
    msg('сканирование не удалось: ' + e);
  }
}

async function enumDevices() {
  try {
    const list = await invoke('list_devices');
    if (list.length) devices = list;
  } catch { }
  if (!devices.length) devices = FALLBACK_DEV;
}

function deviceName() { return devices[devIdx] || FALLBACK_DEV[0]; }

// Переносимость имени: wbr перед заглавными CamelCase и после «_»/дефиса.
function wbrName(s) {
  return s.replace(/([a-z0-9])([A-Z])/g, '$1<wbr>$2').replace(/_/g, '_<wbr>');
}

function tile(p, i) {
  // Сохранённый размер (wide – единственный допустимый в горизонтальном ряду).
  const sz = tileSizes[p.path] === 'wide' ? ' wide' : '';
  return `<div class="tile${sz}${p.off ? ' off' : ''}${sel === p.id ? ' sel' : ''}" data-id="${p.id}" title="${esc(p.name)} – ${esc(p.path)}" style="--c:${p.c};--d:${i * 70}ms"><span class="chk">✓</span><span class="idx">${String(i + 1).padStart(2, '0')}</span><span class="fmt">${p.format}</span><b>${wbrName(esc(p.name))}</b><small>${esc(p.vendor) || 'сторонний'}</small></div>`;
}
function esc(s) { return String(s).replace(/[&<>"]/g, m => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[m])); }

// FNV-1a 64 — должен совпадать с vsthost::stable_id (Rust).
// Возвращаем BigInt: id > Number.MAX_SAFE_INTEGER, строка сломала бы тип u64.
// id по пути считает Rust (stable_id) — на фронте BigInt/Number ненадёжны.
function winxerId(path) { return path; }

function render() {
  setEdit(null); // перерисовка списка убивает режим правки
  main.classList.toggle('chainview', view == 'chain');
  $('#pivot').innerHTML = [['chain', 'цепочка'], ['lib', 'плагины'], ['dirs', 'папки'], ['pre', 'пресеты']]
    .map(([k, t]) => `<button data-v="${k}" class="${view == k ? 'on' : ''}">${t}</button>`).join('');

  if (view == 'chain') {
    main.innerHTML =
      `<section class="grp"><h2>какое устройство фильтровать</h2><div class="row">` +
      `<div class="tile dev" data-dev style="--c:#2d89ef;--d:0ms"><span class="big">⇥</span><b>${esc(deviceName())}</b><small>нажми, чтобы сменить</small></div></div>` +
      `<p class="hint">захват идёт с устройства по умолчанию Windows, наш вывод в него не попадает – петли нет, кабель не нужен. Обработанный звук играет на устройстве выше</p>` +
      `<section class="grp"><h2>обработка, ${chain.length}</h2><div class="row">${chain.map(tile).join('')}<div class="tile add" data-add style="--d:${chain.length * 70}ms"><span>+</span></div></div></section>`;
  } else if (view == 'lib') {
    // Пробел перед классом размера: без него получается «tiletall», и .tile
    // перестаёт матчиться — плитка теряет фон и размер (голый текст).
    const size = p => { const s = tileSizes[p.path]; return s ? ' ' + s : ''; };
    main.innerHTML =
      `<section class="grp"><h2>найдено ${LIB.length}<button class="rescan" data-rescan>пересканировать</button></h2>` +
      `<div class="lib">${LIB.map((p, i) =>
        `<div class="tile${size(p)}${chain.some(c => c.path == p.path) ? ' in' : ''}" data-lib="${i}" title="${esc(p.name)}&#10;${esc(p.path)}" style="--c:${p.c};--d:${i * 30}ms"><span class="fmt">${p.format}</span><b>${wbrName(esc(p.name))}</b><small>${esc(p.path)}</small></div>`).join('')}</div></section>` +
      (LIB.length ? '' : `<section class="grp"><h2>Плагины не найдены</h2><p class="hint">Положи папки .vst3 в C:\Program Files\Common Files\VST3 или добавь свою папку на вкладке «папки», затем нажми «пересканировать».</p></section>`);
  } else if (view == 'dirs') {
    main.innerHTML =
      `<section class="grp"><h2>где искать плагины, ${DIRS.length}</h2><div class="row">` +
      DIRS.map((d, i) =>
        `<div class="tile dir${sel === 'dir:' + i ? ' sel' : ''}" data-dir="${i}" style="--c:${PAL[i % 8]};--d:${i * 60}ms"><b>${esc(d)}</b><small>нажми, чтобы убрать</small></div>`).join('') +
      `<div class="tile add" data-adddir style="--d:${DIRS.length * 60}ms"><span>+</span></div></div>` +
      `<p class="hint">Стандартные папки (C:\\Program Files\\VSTPlugins и т.п.) сканируются всегда. Здесь – твои дополнительные. Поиск в них рекурсивный, вложенность до 4 уровней.</p></section>`;
  } else {
    const P = presets();
    const names = Object.keys(P);
    main.innerHTML =
      `<section class="grp"><h2>пресеты, ${names.length}</h2><div class="row">${names.map((n, i) =>
        `<div class="tile${n.length > 10 ? '' : ''}" data-pre="${esc(n)}" style="--c:${colorOf('pre:' + n)};--d:${i * 50}ms}"><b>${esc(n)}</b><small>${P[n].length} плаг. – нажми, чтобы загрузить</small></div>`).join('')}` +
      `<div class="tile add" data-save-pre style="--d:${names.length * 50}ms"><span>+</span></div></div>` +
      `<p class="hint">«+» сохраняет текущую цепочку как пресет (имя спросит внизу). Клик по пресету – загрузить.</p></section>`;
  }
  bar();
  save();
}

function bar() {
  const i = chain.findIndex(c => c.id === sel), on = view == 'chain' && i >= 0;
  $('#bar').classList.toggle('up', on);
  $('[data-a=left]').disabled = !(on && i > 0);
  $('[data-a=right]').disabled = !(on && i < chain.length - 1);
}

function setSel(id) {
  sel = sel === id ? null : id;
  document.querySelectorAll('.tile[data-id]').forEach(t => t.classList.toggle('sel', +t.dataset.id === sel));
  bar();
}

function msg(text, ms = 4000) {
  const m = $('#msg');
  m.textContent = text;
  m.classList.add('show');
  clearTimeout(msg.t);
  if (ms) msg.t = setTimeout(() => m.classList.remove('show'), ms);
}
function ask(title, def) { return prompt(title, def || ''); }

async function setPower(on) {
  if (on) {
    if (!chain.length) { msg('цепочка пуста – добавь плагины'); return; }
    const active = chain.filter(p => !p.off).map(p => p.path);
    if (!active.length) { msg('все плагины в цепочке выключены – включи хотя бы один'); return; }
    // Совпадение источника и выхода – легально: process loopback исключает наш вывод.
    console.log('[winxer] пуск:', active);
    try {
      await invoke('engine_start', { device: deviceName(), outDevice: deviceName(), chain: active });
      engineOn = true;
    } catch (e) { msg('пуск не удался: ' + e); }
  } else {
    try { await invoke('engine_stop'); } catch { }
    engineOn = false;
  }
  $('#pw').classList.toggle('on', engineOn);
  $('#pw').setAttribute('aria-pressed', engineOn);
  $('#pwl').textContent = engineOn ? 'стоп' : 'пуск';
}

main.addEventListener('click', async e => {
  const t = e.target.closest('.tile'); if (!t) return;
  if (t.dataset.id) { setSel(+t.dataset.id); return; }
  if (t.dataset.lib !== undefined) {
    const p = LIB[+t.dataset.lib];
    if (chain.some(c => c.path == p.path)) { view = 'chain'; render(); return; }
    chain.push({ id: uid++, name: p.name, vendor: p.vendor, format: p.format, path: p.path, off: false, c: p.c });
    view = 'chain';
  }
  else if (t.hasAttribute('data-dev')) { devIdx = (devIdx + 1) % devices.length; }
  else if (t.hasAttribute('data-add')) { view = 'lib'; render(); return; }
  else if (t.hasAttribute('data-save-pre')) {
    if (!chain.length) { msg('нечего сохранять – цепочка пуста'); return; }
    const name = ask('имя пресета', 'мой пресет');
    if (!name) return;
    const P = presets();
    P[name] = chain.map(p => ({ name: p.name, vendor: p.vendor, format: p.format, path: p.path, off: p.off }));
    savePresets(P);
  }
  else if (t.dataset.pre !== undefined) {
    const P = presets(); const pl = P[t.dataset.pre];
    if (pl) { chain = pl.map(p => ({ ...p, id: uid++, c: colorOf(p.path.toLowerCase()) })); sel = null; view = 'chain'; msg(`пресет «${t.dataset.pre}» загружен`); }
  }
  else if (t.dataset.dir !== undefined) {
    const d = DIRS[+t.dataset.dir];
    if (d) {
      DIRS = await invoke('remove_custom_dir', { dir: d });
      await scan();
      msg('папка убрана: ' + d);
    }
  }
  else if (t.hasAttribute('data-adddir')) {
    const dir = await dialog.open({ directory: true, title: 'папка с плагинами' });
    if (dir) {
      DIRS = await invoke('add_custom_dir', { dir });
      await scan();
      msg('папка добавлена: ' + dir);
    }
    return;
  }
  if (engineOn) {
    // Пересборка: стоп старого графа, выгрузка удалённых плагинов, старт нового.
    const active = chain.filter(p => !p.off).map(p => p.path);
    try {
      await invoke('engine_rebuild', { device: deviceName(), outDevice: deviceName(), chain: active });
    } catch (e) { msg('пересборка не удалась: ' + e); }
  }
  render();
});

$('#pivot').addEventListener('click', e => {
  const b = e.target.closest('button'); if (!b) return;
  if (b.dataset.rescan !== undefined) { scan().then(render); return; }
  if (view === b.dataset.v) return;
  view = b.dataset.v; sel = null;
  main.scrollLeft = 0; // смена вкладки — всегда с начала списка
  main.dispatchEvent(new Event('scroll')); // синхронизировать pos/target в initSmooth
  render();
});

$('#pw').addEventListener('click', () => setPower(!engineOn));

$('#bar').addEventListener('click', async e => {
  const b = e.target.closest('button'); if (!b) return;
  const i = chain.findIndex(c => c.id === sel); if (i < 0) return;
  switch (b.dataset.a) {
    case 'off': chain[i].off = !chain[i].off; break;
    case 'left': if (i > 0) [chain[i - 1], chain[i]] = [chain[i], chain[i - 1]]; break;
    case 'right': if (i < chain.length - 1) [chain[i + 1], chain[i]] = [chain[i], chain[i + 1]]; break;
    case 'del': chain.splice(i, 1); sel = null; break;
    case 'gui':
      try { await invoke('open_editor', { path: chain[i].path, title: chain[i].name }); }
      catch (err) { msg('не удалось открыть редактор: ' + err); }
      return;
  }
  if (engineOn) {
    const active = chain.filter(p => !p.off).map(p => p.path);
    try {
      await invoke('engine_rebuild', { device: deviceName(), outDevice: deviceName(), chain: active });
    } catch (e) { msg('пересборка не удалась: ' + e); }
  }
  render();
});

// фирменное «вдавливание» плитки Metro: зажал — плитка гнётся под курсор.
let pressed = null;
function bend(t, cx, cy) {
  const r = t.getBoundingClientRect();
  let x = (cx - r.left) / r.width - .5, y = (cy - r.top) / r.height - .5;
  // За пределами плитки — тянемся к курсору (до ±1.5), внутри — обычный наклон.
  x = Math.max(-1.5, Math.min(1.5, x)); y = Math.max(-1.5, Math.min(1.5, y));
  const inside = x >= -.5 && x <= .5 && y >= -.5 && y <= .5;
  const k = inside ? 16 : 7; // внутри наклоняем сильнее, снаружи — плавнее
  const scale = inside ? .96 : 1;
  t.style.animation = 'none';
  t.style.transform = `perspective(500px) rotateY(${x * k}deg) rotateX(${-y * k}deg) scale(${scale})`;
}
main.addEventListener('pointerdown', e => {
  if (e.target.closest('.rz')) return; // кнопка ресайза: не гнём и НЕ захватываем pointer — иначе click уйдёт плитке
  const t = e.target.closest('.tile'); if (!t) return;
  pressed = t;
  try { t.setPointerCapture(e.pointerId); } catch { }
  bend(t, e.clientX, e.clientY);
});
main.addEventListener('pointermove', e => {
  if (!pressed) return;
  bend(pressed, e.clientX, e.clientY);
});
['pointerup', 'pointercancel'].forEach(ev => main.addEventListener(ev, e => {
  if (pressed) { pressed.style.transform = ''; pressed = null; }
}, true));
main.addEventListener('pointerleave', () => {
  if (pressed) { pressed.style.transform = ''; pressed = null; }
});

// Плавная прокрутка + сжатие плиток у краёв (одна функция на все списки).
const ZONE = 200;        // px от края, где плитки сжимаются
const MIN = .55;         // минимальный размер плитки
const SCROLL_EASE = .16; // 0..1, чем меньше, тем «мягче» доезжает прокрутка
const FX_EASE = .22;     // 0..1, чем меньше, тем плавнее плитки сжимаются

// Элементы — все .tile внутри скролл-контейнера (у нас секции с заголовками).
function initSmooth(el) {
  let items = [], raf = 0;
  let pos = el.scrollLeft, target = pos; // float-прокрутка (scrollLeft округляется)
  const maxScroll = () => el.scrollWidth - el.clientWidth;
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));

  function measure() {
    // Плитки + заголовки секций (не-sticky — т.е. на «цепочке», где они уезжают).
    items = [...el.querySelectorAll('.tile, .grp h2')]
      .filter(t => !t.matches('h2') || getComputedStyle(t).position !== 'sticky')
      .map(t => ({ t, cx: t.offsetLeft + t.offsetWidth / 2, k: null }));
    kick();
  }

  // Колесо не прыгает, а задаёт цель, к которой прокрутка плавно едет.
  el.addEventListener('wheel', e => {
    const d = Math.abs(e.deltaY) > Math.abs(e.deltaX) ? e.deltaY : e.deltaX;
    target = clamp(target + d, 0, maxScroll());
    e.preventDefault();
    kick();
  }, { passive: false });

  // Прокрутили не колесом (ползунок, тач, клавиши) — синхронизируемся.
  el.addEventListener('scroll', () => {
    if (Math.abs(el.scrollLeft - pos) > 1.5) { pos = target = el.scrollLeft; }
    kick();
  }, { passive: true });

  // Один цикл кадров: и прокрутка, и сжатие плавно догоняют цель.
  function tick() {
    raf = 0;
    let moving = false;

    const dx = target - pos;
    if (Math.abs(dx) > .4) { pos += dx * SCROLL_EASE; moving = true; } else pos = target;
    if (Math.abs(el.scrollLeft - pos) > .4) el.scrollLeft = pos;

    const sl = el.scrollLeft, w = el.clientWidth, max = maxScroll();
    const zl = Math.min(sl, ZONE), zr = Math.min(Math.max(max - sl, 0), ZONE);

    for (const it of items) {
      const x = it.cx - sl;
      let g = 1;
      if (zl > 0 && x < zl) g = Math.min(g, x / zl);
      if (zr > 0 && w - x < zr) g = Math.min(g, (w - x) / zr);
      g = clamp(g, 0, 1); g = g * g * (3 - 2 * g);

      if (it.k === null) { it.k = g; } // первый показ: сразу правильное значение
      else {
        const d = g - it.k;
        if (Math.abs(d) < .003) it.k = g;
        else { it.k += d * FX_EASE; moving = true; }
      }
      it.t.style.scale = MIN + (1 - MIN) * it.k;
      it.t.style.opacity = it.k;
    }
    if (moving) kick();
  }
  const kick = () => { if (!raf) raf = requestAnimationFrame(tick); };

  new ResizeObserver(measure).observe(el);
  new MutationObserver(measure).observe(el, { childList: true, subtree: true });
  el.addEventListener('relayout', measure);
  measure();
}

initSmooth(main);

document.addEventListener('contextmenu', e => e.preventDefault());

// --- Плавное изменение размера плиток (ПКМ — режим правки) ----------------
const ORDER = ['', 'tall', 'wide']; // круг: обычная, вертикальная, широкая, снова обычная
const ORDER_FLAT = ['', 'wide'];   // в цепочке (горизонтальный ряд): вертикальной нет — некуда расти
const DUR = 320, EASE = 'cubic-bezier(.2,.8,.2,1)';

// Сохранённые размеры плиток: путь плагина → класс ('tall'|'wide'|'').
let tileSizes = {};
try { tileSizes = JSON.parse(localStorage.getItem('winxer.tileSizes') || '{}'); } catch { }
const saveTileSizes = () => localStorage.setItem('winxer.tileSizes', JSON.stringify(tileSizes));

const ICON_GROW = `<svg viewBox="0 0 24 24"><path d="M14 4h6v6M10 20H4v-6M20 4l-7 7M4 20l7-7"/></svg>`;
const ICON_SHRINK = `<svg viewBox="0 0 24 24"><path d="M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7"/></svg>`;

const cur = t => t.classList.contains('tall') ? 'tall' : t.classList.contains('wide') ? 'wide' : '';
const orderFor = t => t.closest('.row') ? ORDER_FLAT : ORDER;
const next = t => orderFor(t)[(orderFor(t).indexOf(cur(t)) + 1) % orderFor(t).length];

function updateBtn(t) {
  const b = t.querySelector('.rz'); if (!b) return;
  const ord = orderFor(t);
  const last = ord.indexOf(cur(t)) === ord.length - 1;
  b.innerHTML = last ? ICON_SHRINK : ICON_GROW;
  const label = last ? 'Уменьшить' : 'Увеличить';
  b.title = label; b.setAttribute('aria-label', label);
}

let editEl = null;
function setEdit(t) {
  if (editEl) { editEl.classList.remove('edit'); editEl.querySelector('.rz')?.remove(); }
  editEl = t;
  if (t) {
    t.classList.add('edit');
    const b = document.createElement('button'); b.className = 'rz';
    t.appendChild(b); updateBtn(t);
  }
}
main.addEventListener('contextmenu', e => {
  const t = e.target.closest('.tile'); if (!t) return;
  e.preventDefault(); e.stopPropagation();
  console.log('[winxer] ПКМ по плитке, вхожу в правку:', t.dataset.lib ?? t.dataset.id);
  setEdit(t);
});
document.addEventListener('click', e => {
  if (!editEl) return;
  const b = e.target.closest('.rz');
  if (b && editEl.contains(b)) {
    e.stopPropagation(); e.preventDefault(); // не проваливаться в действие плитки
    cycle(editEl);
    return;
  }
  e.stopPropagation(); e.preventDefault(); setEdit(null);
}, true);
document.addEventListener('keydown', e => { if (e.key === 'Escape') setEdit(null); });

// Размеры/позиции из offset* (не getBoundingClientRect — режим правки сжимает).
const geom = x => ({ l: x.offsetLeft, t: x.offsetTop, w: x.offsetWidth, h: x.offsetHeight });

// Ключ плитки: путь плагина (data-lib → LIB[i].path) или data-id для цепочки.
function tileKey(t) {
  if (t.dataset.lib !== undefined) return LIB[+t.dataset.lib]?.path;
  if (t.dataset.id !== undefined) return 'chain:' + t.dataset.id;
  return null;
}

function cycle(t) {
  const list = t.closest('.lib') || t.closest('.row') || main;
  const tiles = [...list.querySelectorAll('.tile')];
  const before = new Map(tiles.map(x => [x, geom(x)]));

  const n = next(t);
  t.classList.remove('tall', 'wide');
  if (n) t.classList.add(n);
  const key = tileKey(t);
  if (key) { tileSizes[key] = n; saveTileSizes(); }
  updateBtn(t);

  const view0 = main.scrollLeft, view1 = view0 + main.clientWidth;
  for (const x of tiles) {
    const a = before.get(x), b = geom(x);
    const moved = a.l !== b.l || a.t !== b.t;
    if (x === t) {
      x.style.zIndex = 2;
      const anim = x.animate([
        { width: a.w + 'px', height: a.h + 'px', translate: `${a.l - b.l}px ${a.t - b.t}px` },
        { width: b.w + 'px', height: b.h + 'px', translate: '0 0' }
      ], { duration: DUR, easing: EASE });
      anim.onfinish = anim.oncancel = () => { x.style.zIndex = ''; };
    } else if (moved && !((a.l + a.w < view0 && b.l + b.w < view0) || (a.l > view1 && b.l > view1))) {
      x.animate([{ translate: `${a.l - b.l}px ${a.t - b.t}px` }, { translate: '0 0' }], { duration: DUR, easing: EASE });
    }
  }
  main.dispatchEvent(new Event('relayout'));
}

// Заголовки секций: не сжимаются и остаются на месте при скролле.
// (h2 sticky слева, эффект касается только .tile — см. initSmooth.)

(async () => {
  load();
  await enumDevices();
  await scan();
  render();
})();
