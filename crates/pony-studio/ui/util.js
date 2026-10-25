// Мелкие помощники интерфейса: построение DOM, поля ввода, цвета.
'use strict';

const SVGNS = 'http://www.w3.org/2000/svg';

/** h('div', {class: 'x', onclick: f}, 'текст', child, [детей…]) */
function h(tag, props, ...children) {
  const el = document.createElement(tag);
  setProps(el, props);
  appendKids(el, children);
  return el;
}

function svgEl(tag, attrs, ...children) {
  const el = document.createElementNS(SVGNS, tag);
  if (attrs) for (const [k, v] of Object.entries(attrs)) if (v != null) el.setAttribute(k, v);
  appendKids(el, children);
  return el;
}

function setProps(el, props) {
  if (!props) return;
  for (const [k, v] of Object.entries(props)) {
    if (v == null || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
    else if (k === 'key') el.dataset.key = v;
    else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v);
    else if (k === 'value') el.value = v;
    else if (k === 'checked' || k === 'disabled' || k === 'selected') el[k] = !!v;
    else if (k === 'html') el.innerHTML = v;
    else el.setAttribute(k, v === true ? '' : v);
  }
}

function appendKids(el, children) {
  for (const c of children.flat(Infinity)) {
    if (c == null || c === false) continue;
    el.appendChild(c instanceof Node ? c : document.createTextNode(String(c)));
  }
}

/** Заменить детей элемента (null/false пропускаются, массивы разворачиваются). */
function fill(el, ...kids) {
  el.replaceChildren(...kids.flat(Infinity).filter(k => k != null && k !== false).map(k => (k instanceof Node ? k : document.createTextNode(String(k)))));
  return el;
}

function icon(name, cls) {
  const s = document.createElementNS(SVGNS, 'svg');
  s.setAttribute('class', 'ico' + (cls ? ' ' + cls : ''));
  const u = document.createElementNS(SVGNS, 'use');
  u.setAttribute('href', '#i-' + name);
  s.appendChild(u);
  return s;
}

const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

function fmt(v, digits = 2) {
  if (v == null || !isFinite(v)) return '';
  const s = Number(v).toFixed(digits);
  return s.includes('.') ? s.replace(/\.?0+$/, '') || '0' : s;
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// ---------------------------------------------------------------- цвета

const hex2 = n => clamp(Math.round(n), 0, 255).toString(16).padStart(2, '0');
const rgbHex = c => '#' + hex2(c[0]) + hex2(c[1]) + hex2(c[2]);
function hexRgb(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex);
  if (!m) return [0, 0, 0];
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
const rgbaCss = c => `rgba(${c[0]},${c[1]},${c[2]},${(c[3] ?? 255) / 255})`;

// ---------------------------------------------------------------- поля

/**
 * Числовое поле как DragValue в egui: тянуть подпись влево-вправо —
 * менять значение, клик в поле — ввести число (Enter/уход фокуса).
 * onChange(v, final): final=false во время перетаскивания.
 */
function numField({ key, label, value, step = 1, min = -Infinity, max = Infinity, digits = 2, suffix = '', onChange, title, disabled }) {
  const input = h('input', { type: 'text', inputmode: 'decimal', key, value: fmt(value, digits), disabled, spellcheck: 'false' });
  const lab = label != null ? h('span', { class: 'nf-label', title: title || 'Тяни влево-вправо, чтобы изменить' }, label) : null;
  const wrap = h('label', { class: 'numfield' + (disabled ? ' disabled' : ''), title }, lab, input, suffix ? h('span', { class: 'nf-suffix' }, suffix) : null);
  const commit = final => {
    const v = parseFloat(String(input.value).replace(',', '.'));
    if (!isFinite(v)) { input.value = fmt(value, digits); return; }
    const c = clamp(v, min, max);
    input.value = fmt(c, digits);
    if (c !== value || final) onChange(c, final);
  };
  input.addEventListener('keydown', e => {
    if (e.key === 'Enter') { commit(true); input.blur(); }
    else if (e.key === 'Escape') { input.value = fmt(value, digits); input.blur(); }
    else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault();
      const v = parseFloat(input.value) || 0;
      input.value = fmt(clamp(v + (e.key === 'ArrowUp' ? 1 : -1) * step * (e.shiftKey ? 10 : 1), min, max), digits);
      commit(true);
    }
  });
  input.addEventListener('change', () => commit(true));
  if (lab && !disabled) {
    lab.addEventListener('pointerdown', e => {
      e.preventDefault();
      const startX = e.clientX, start = parseFloat(input.value) || 0;
      let last = start;
      document.body.classList.add('scrubbing');
      const move = ev => {
        const k = ev.shiftKey ? 10 : ev.altKey ? 0.1 : 1;
        last = clamp(start + (ev.clientX - startX) * step * k, min, max);
        input.value = fmt(last, digits);
        onChange(last, false);
      };
      const up = () => {
        window.removeEventListener('pointermove', move);
        window.removeEventListener('pointerup', up);
        document.body.classList.remove('scrubbing');
        onChange(last, true);
      };
      window.addEventListener('pointermove', move);
      window.addEventListener('pointerup', up);
    });
  }
  return wrap;
}

/** Ползунок с числом. */
function slider({ key, label, value, min, max, step = 0.01, digits = 2, suffix = '', onChange, log }) {
  const toPos = v => log ? Math.log(v / min) / Math.log(max / min) * 1000 : v;
  const fromPos = p => log ? min * Math.pow(max / min, p / 1000) : p;
  const out = h('span', { class: 'sl-val' }, fmt(value, digits) + suffix);
  const range = h('input', { type: 'range', key, min: log ? 0 : min, max: log ? 1000 : max, step: log ? 1 : step, value: toPos(value) });
  range.addEventListener('input', () => { const v = fromPos(+range.value); out.textContent = fmt(v, digits) + suffix; onChange(v, false); });
  range.addEventListener('change', () => onChange(fromPos(+range.value), true));
  return h('label', { class: 'slider' }, label ? h('span', { class: 'sl-label' }, label) : null, range, out);
}

function checkbox({ key, label, checked, onChange, title, disabled }) {
  const cb = h('input', { type: 'checkbox', key, checked, disabled });
  cb.addEventListener('change', () => onChange(cb.checked));
  return h('label', { class: 'check' + (disabled ? ' disabled' : ''), title }, cb, h('span', null, label));
}

function select({ key, value, options, onChange, title, disabled }) {
  const sel = h('select', { key, title, disabled });
  for (const o of options) {
    const [v, l] = Array.isArray(o) ? o : [o, o];
    sel.appendChild(h('option', { value: v == null ? '' : v, selected: (v ?? '') === (value ?? '') }, l));
  }
  sel.addEventListener('change', () => onChange(sel.value === '' ? null : sel.value));
  return sel;
}

function colorField({ key, value, onChange, alpha = false, title }) {
  const input = h('input', { type: 'color', key, value: rgbHex(value), title });
  const send = final => {
    const c = hexRgb(input.value);
    onChange([...c, alpha ? +a.value : (value[3] ?? 255)], final);
  };
  input.addEventListener('input', () => send(false));
  input.addEventListener('change', () => send(true));
  const a = alpha ? h('input', { type: 'number', class: 'alpha', min: 0, max: 255, value: value[3] ?? 255, title: 'Непрозрачность 0–255', key: key && key + '.a' }) : null;
  if (a) a.addEventListener('change', () => send(true));
  return h('span', { class: 'colorfield' }, input, a);
}

function button(label, onClick, opts = {}) {
  const b = h('button', { class: opts.class || (opts.primary ? 'primary' : null), title: opts.title, disabled: opts.disabled, key: opts.key, type: 'button' },
    opts.icon ? icon(opts.icon) : null, label ? h('span', null, label) : null);
  b.addEventListener('click', e => { e.stopPropagation(); onClick(e); });
  return b;
}

const row = (...kids) => h('div', { class: 'row' }, ...kids);
const field = (label, ...kids) => h('div', { class: 'field' }, h('span', { class: 'field-label' }, label), h('div', { class: 'field-body' }, ...kids));
const hint = text => h('p', { class: 'hint' }, text);
const group = (title, ...kids) => h('section', { class: 'group' }, title ? h('h4', null, title) : null, ...kids);

/**
 * Заменить содержимое контейнера, сохранив фокус и выделение в поле ввода
 * с тем же data-key (иначе перерисовка панели сбивала бы набор текста).
 */
function replaceKeepingFocus(container, build) {
  const active = document.activeElement;
  let key = null, selStart = null, selEnd = null, typed = null;
  if (active && container.contains(active) && active.dataset.key) {
    key = active.dataset.key;
    try { selStart = active.selectionStart; selEnd = active.selectionEnd; } catch (e) { /* не текстовое поле */ }
    if (active.tagName === 'TEXTAREA' || (active.tagName === 'INPUT' && active.type === 'text')) typed = active.value;
  }
  const scroll = container.scrollTop;
  fill(container, build());
  container.scrollTop = scroll;
  if (key) {
    const el = container.querySelector(`[data-key="${CSS.escape(key)}"]`);
    if (el) {
      if (typed != null) el.value = typed;
      el.focus({ preventScroll: true });
      try { if (selStart != null) el.setSelectionRange(selStart, selEnd); } catch (e) { /* ок */ }
    }
  }
}

/** Перетаскивание окна за заголовок. */
function makeDraggable(win, handle) {
  handle.addEventListener('pointerdown', e => {
    if (e.target.closest('button, input, select')) return;
    e.preventDefault();
    const r = win.getBoundingClientRect();
    const dx = e.clientX - r.left, dy = e.clientY - r.top;
    const move = ev => {
      win.style.left = clamp(ev.clientX - dx, 0, innerWidth - 80) + 'px';
      win.style.top = clamp(ev.clientY - dy, 0, innerHeight - 40) + 'px';
      win.style.transform = 'none';
    };
    const up = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', up); };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  });
}
