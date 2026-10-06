const { invoke, window: tauriWindow } = window.__TAURI__;

// label окна, заданный в Rust: editor-<id>
const label = (() => {
  try { return tauriWindow.getCurrent().label; } catch { return ''; }
})();

const params = new Map(location.hash.slice(1).split('&').map(s => s.split('=')));
const id = params.get('id');

const stateKey = `winxer.editor.${id}`;

function esc(s) { return String(s).replace(/[&<>"]/g, m => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[m])); }

function findPlugin() {
  try {
    const st = JSON.parse(localStorage.getItem('winxer.state') || '{}');
    return (st.chain || []).find(p => String(p.id) === String(id));
  } catch { return null; }
}

function renderPresets() {
  const box = $('#presets');
  let names = [];
  try { names = Object.keys(JSON.parse(localStorage.getItem(`winxer.pluginPresets.${id}`) || '{}')); } catch { }
  box.innerHTML = names.map(n => `<button data-pp="${esc(n)}">${esc(n)}</button>`).join('') +
    `<button data-pp-save>＋ сохранить настройки</button>`;
}

// Настройки плагина (пока параметры недоступны нативно — храним произвольные заметки/слайдеры UI).
function loadState() {
  try { return JSON.parse(localStorage.getItem(stateKey) || '{}'); } catch { return {}; }
}
function saveState(s) { localStorage.setItem(stateKey, JSON.stringify(s)); }

const $ = s => document.querySelector(s);

$('#close').addEventListener('click', () => invoke('close_window', { label }));

document.addEventListener('click', async e => {
  const b = e.target.closest('button'); if (!b) return;
  if (b.dataset.ppSave) {
    const name = prompt('имя набора настроек', 'настройки 1'); if (!name) return;
    const store = JSON.parse(localStorage.getItem(`winxer.pluginPresets.${id}`) || '{}');
    store[name] = loadState();
    localStorage.setItem(`winxer.pluginPresets.${id}`, JSON.stringify(store));
    renderPresets();
  } else if (b.dataset.pp) {
    const store = JSON.parse(localStorage.getItem(`winxer.pluginPresets.${id}`) || '{}');
    if (store[b.dataset.pp]) { saveState(store[b.dataset.pp]); show(); }
  }
});

function show() {
  const pl = findPlugin();
  if (pl) {
    $('#t').textContent = pl.name;
    document.documentElement.style.setProperty('--c', pl.c || '#9f00a7');
    $('#p').textContent = `${pl.format} · ${pl.path}`;
  }
  const st = loadState();
  $('#info').textContent = st.note || 'Нативный UI плагина будет подключён на этапе хостинга (VST2/VST3 через Rust). Здесь появятся его реальные параметры — и они будут сохраняться между открытиями окна.';
  renderPresets();
}

show();
