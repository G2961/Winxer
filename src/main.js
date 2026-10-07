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
let devIdx = 0, outIdx = 0;
let engineOn = false;

const $ = s => document.querySelector(s);
const main = $('#main');

// --- сохранение состояния: ничего не сбрасывается при перезапуске приложения ---
function save() {
  localStorage.setItem('winxer.state', JSON.stringify({ chain, devIdx, outIdx, devices, devName: deviceName(), outName: outName() }));
}
function load() {
  try {
    const s = JSON.parse(localStorage.getItem('winxer.state') || '{}');
    if (Array.isArray(s.chain)) { chain = s.chain; uid = chain.reduce((m, p) => Math.max(m, p.id), 0) + 1; }
    if (s.devices?.length) devices = s.devices;
    if (s.devName) { const i = devices.indexOf(s.devName); if (i >= 0) devIdx = i; }
    if (s.outName) { const i = devices.indexOf(s.outName); if (i >= 0) outIdx = i; }
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
  // Умный дефолт: источник = кабель (если есть), выход = реальное устройство.
  if (devIdx === 0 && outIdx === 0) {
    const cable = devices.findIndex(d => /cable/i.test(d));
    if (cable > 0) { devIdx = cable; outIdx = cable === 0 ? 1 : 0; }
  }
}

function deviceName() { return devices[devIdx] || FALLBACK_DEV[0]; }
function outName() { return devices[outIdx] || FALLBACK_DEV[0]; }

function tile(p, i) {
  return `<div class="tile${p.off ? ' off' : ''}${sel === p.id ? ' sel' : ''}" data-id="${p.id}" style="--c:${p.c};--d:${i * 70}ms"><span class="chk">✓</span><span class="idx">${String(i + 1).padStart(2, '0')}</span><span class="fmt">${p.format}</span><b>${esc(p.name)}</b><small>${esc(p.vendor) || 'сторонний'}</small></div>`;
}
function esc(s) { return String(s).replace(/[&<>"]/g, m => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[m])); }

// FNV-1a 64 — должен совпадать с vsthost::stable_id (Rust).
// Возвращаем BigInt: id > Number.MAX_SAFE_INTEGER, строка сломала бы тип u64.
// id по пути считает Rust (stable_id) — на фронте BigInt/Number ненадёжны.
function winxerId(path) { return path; }

function render() {
  $('#pivot').innerHTML = [['chain', 'цепочка'], ['lib', 'плагины'], ['dirs', 'папки'], ['pre', 'пресеты']]
    .map(([k, t]) => `<button data-v="${k}" class="${view == k ? 'on' : ''}">${t}</button>`).join('');

  if (view == 'chain') {
    const outHtml = outName() === deviceName()
      ? `<p class="hint warn">выход не выбран — нажми «пуск» не получится. Выбери выход кликом по второй плитке.</p>`
      : '';
    main.innerHTML =
      `<section class="grp"><h2>какое устройство фильтровать</h2><div class="row">` +
      `<div class="tile dev" data-dev style="--c:#2d89ef;--d:0ms"><span class="big">⇥</span><b>${esc(deviceName())}</b><small>нажми, чтобы сменить</small></div>` +
      `<div class="tile dev" data-devout style="--c:#00a300;--d:70ms"><span class="big">⇤</span><b>${esc(outName())}</b><small>куда играет обработанный</small></div></div>` +
      outHtml +
      `<section class="grp"><h2>обработка, ${chain.length}</h2><div class="row">${chain.map(tile).join('')}<div class="tile add" data-add style="--d:${chain.length * 70}ms"><span>+</span></div></div></section>`;
  } else if (view == 'lib') {
    main.innerHTML =
      `<section class="grp"><h2>найдено ${LIB.length}<button class="rescan" data-rescan>пересканировать</button></h2>` +
      `<div class="lib">${LIB.map((p, i) =>
        `<div class="tile${chain.some(c => c.path == p.path) ? ' in' : ''}" data-lib="${i}" style="--c:${p.c};--d:${i * 30}ms"><span class="fmt">${p.format}${p.arch == 'x86' ? ' · 32-бит (мост)' : ''}</span><b>${esc(p.name)}</b><small>${esc(p.path)}</small></div>`).join('')}</div></section>` +
      (LIB.length ? '' : `<section class="grp"><h2>Плагины не найдены</h2><p class="hint">Положи DLL (VST2) или папки .vst3 в C:\\Program Files\\Common Files\\VST3, C:\\Program Files\\VSTPlugins и т.п., затем нажми «пересканировать».</p></section>`);
  } else if (view == 'dirs') {
    main.innerHTML =
      `<section class="grp"><h2>где искать плагины, ${DIRS.length}</h2><div class="row">` +
      DIRS.map((d, i) =>
        `<div class="tile dir${sel === 'dir:' + i ? ' sel' : ''}" data-dir="${i}" style="--c:${PAL[i % 8]};--d:${i * 60}ms"><b>${esc(d)}</b><small>нажми, чтобы убрать</small></div>`).join('') +
      `<div class="tile add" data-adddir style="--d:${DIRS.length * 60}ms"><span>+</span></div></div>` +
      `<p class="hint">Стандартные папки (C:\\Program Files\\VSTPlugins и т.п.) сканируются всегда. Здесь — твои дополнительные. Поиск в них рекурсивный, вложенность до 4 уровней.</p></section>`;
  } else {
    const P = presets();
    const names = Object.keys(P);
    main.innerHTML =
      `<section class="grp"><h2>пресеты, ${names.length}</h2><div class="row">${names.map((n, i) =>
        `<div class="tile${n.length > 10 ? '' : ''}" data-pre="${esc(n)}" style="--c:${colorOf('pre:' + n)};--d:${i * 50}ms"><b>${esc(n)}</b><small>${P[n].length} плаг. — нажми, чтобы загрузить</small></div>`).join('')}` +
      `<div class="tile add" data-save-pre style="--d:${names.length * 50}ms"><span>+</span></div></div>` +
      `<p class="hint">«+» сохраняет текущую цепочку как пресет (имя спросит внизу). Клик по пресету — загрузить.</p></section>`;
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
    if (!chain.length) { msg('цепочка пуста — добавь плагины'); return; }
    const active = chain.filter(p => !p.off).map(p => p.path);
    if (!active.length) { msg('все плагины в цепочке выключены — включи хотя бы один'); return; }
    if (deviceName() === outName()) { msg('источник и выход совпадают — будет петля фидбэка. Смени выход'); return; }
    console.log('[winxer] пуск:', active);
    try {
      await invoke('engine_start', { device: deviceName(), outDevice: outName(), chain: active });
      engineOn = true;
    } catch (e) { msg('пуск не удался: ' + e); }
  } else {
    try { await invoke('engine_stop'); } catch { }
    engineOn = false;
  }
  $('#pw').classList.toggle('on', engineOn);
  $('#pwl').textContent = engineOn ? 'стоп' : 'пуск';
}

main.addEventListener('click', async e => {
  const t = e.target.closest('.tile'); if (!t) return;
  if (t.dataset.id) { setSel(+t.dataset.id); return; }
  if (t.dataset.lib !== undefined) {
    const p = LIB[+t.dataset.lib];
    // 32-бит теперь поддерживается через мост — блокировки нет.
    if (chain.some(c => c.path == p.path)) { view = 'chain'; render(); return; }
    chain.push({ id: uid++, name: p.name, vendor: p.vendor, format: p.format, path: p.path, off: false, c: p.c });
    view = 'chain';
  }
  else if (t.hasAttribute('data-dev')) { devIdx = (devIdx + 1) % devices.length; }
  else if (t.hasAttribute('data-devout')) { outIdx = (outIdx + 1) % devices.length; }
  else if (t.hasAttribute('data-add')) { view = 'lib'; render(); return; }
  else if (t.hasAttribute('data-save-pre')) {
    if (!chain.length) { msg('нечего сохранять — цепочка пуста'); return; }
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
      await invoke('engine_rebuild', { device: deviceName(), outDevice: outName(), chain: active });
    } catch (e) { msg('пересборка не удалась: ' + e); }
  }
  render();
});

$('#pivot').addEventListener('click', e => {
  const b = e.target.closest('button'); if (!b) return;
  if (b.dataset.rescan !== undefined) { scan().then(render); return; }
  view = b.dataset.v; sel = null; render();
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
      await invoke('engine_rebuild', { device: deviceName(), outDevice: outName(), chain: active });
    } catch (e) { msg('пересборка не удалась: ' + e); }
  }
  render();
});

// фирменное «вдавливание» плитки Metro
main.addEventListener('pointerdown', e => {
  const t = e.target.closest('.tile'); if (!t) return;
  const r = t.getBoundingClientRect(), x = (e.clientX - r.left) / r.width - .5, y = (e.clientY - r.top) / r.height - .5;
  t.style.animation = 'none'; t.style.transform = `perspective(500px) rotateY(${x * 16}deg) rotateX(${-y * 16}deg) scale(.96)`;
});
['pointerup', 'pointerleave', 'pointercancel'].forEach(ev => main.addEventListener(ev, e => {
  const t = e.target.closest && e.target.closest('.tile'); if (t) t.style.transform = '';
}, true));

(async () => {
  load();
  await enumDevices();
  await scan();
  render();
})();
