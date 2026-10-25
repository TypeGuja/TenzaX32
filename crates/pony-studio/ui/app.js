// Ядро интерфейса: связь с Rust, меню, тулбар, инструменты, горячие клавиши.
'use strict';

/** Состояние, присланное Rust: ui, doc, anim, play, frame, vec, env, xfl, xfl_preview. */
const S = {};

const App = {
  scene: [960, 720],
  rulers: true,
  showPanel(on) { document.getElementById('app').classList.toggle('no-panel', !on); App.panelOn = on; Chrome.renderToolbar(); setTimeout(() => Stage.resize(), 0); },
  showTimeline(on) { document.getElementById('app').classList.toggle('no-timeline', !on); App.timelineOn = on; setTimeout(() => { Stage.resize(); Timeline.layout(); }, 0); },
  setRulers(on) { App.rulers = on; document.getElementById('stage').classList.toggle('no-rulers', !on); setTimeout(() => Stage.resize(), 0); },
  panelOn: true,
  timelineOn: true,
};

// ---------------------------------------------------------------- связь с Rust

const Pointer = { down: false };
window.addEventListener('pointerdown', () => { Pointer.down = true; }, true);
window.addEventListener('pointerup', () => { Pointer.down = false; }, true);
window.addEventListener('pointercancel', () => { Pointer.down = false; }, true);
window.addEventListener('blur', () => { Pointer.down = false; });

const Net = { queue: [], inflight: null, nextId: 1 };

/**
 * Отправить команду в Rust. Команды выполняются строго по очереди, одна в
 * полёте; `coalesce` — частые сообщения (перетаскивание) склеиваются, пока
 * ждут отправки. Promise разрешается результатом, при ошибке — undefined
 * (текст ошибки уже показан в строке состояния).
 */
function rpc(cmd, args = {}, opts = {}) {
  return new Promise(resolve => {
    const down = opts.down ?? Pointer.down;
    if (opts.coalesce) {
      const q = Net.queue.find(m => m.coalesce === opts.coalesce);
      if (q) {
        q.args = opts.merge ? opts.merge(q.args, args) : args;
        q.down = down;
        q.waiters.push(resolve);
        return;
      }
    }
    Net.queue.push({ cmd, args, down, coalesce: opts.coalesce, waiters: [resolve] });
    pump();
  });
}

function pump() {
  if (Net.inflight || !Net.queue.length) return;
  const m = Net.queue.shift();
  m.id = Net.nextId++;
  Net.inflight = m;
  window.ipc.postMessage(JSON.stringify({ id: m.id, cmd: m.cmd, args: m.args, down: m.down }));
}

const changedAcc = {};
let renderQueued = false;

window.__pony = {
  recv(reply) {
    // id == null — не ответ, а сообщение от Rust (например, экспорт закончился).
    const m = reply.id == null ? null : Net.inflight;
    if (m) Net.inflight = null;
    for (const [k, v] of Object.entries(reply.patch || {})) { S[k] = v; changedAcc[k] = true; }
    if (reply.error) Chrome.flash();
    if (m) for (const w of m.waiters) w(reply.error ? undefined : reply.result);
    if (!renderQueued) { renderQueued = true; requestAnimationFrame(renderAll); }
    pump();
  },
  confirmClose() {
    if (Modal.stack.some(r => r.win.classList.contains('confirm'))) return;
    confirmDialog(h('div', null, h('b', null, 'Сохранить изменения перед выходом?'), h('p', { class: 'hint' }, 'Несохранённые правки персонажа будут потеряны.')), 'Сохранить', 'Не сохранять').then(async v => {
      if (v === 'extra') rpc('exit');
      else if (v === true && await Actions.save()) rpc('exit');
    });
  },
};

function renderAll() {
  renderQueued = false;
  const changed = Object.assign({}, changedAcc);
  for (const k of Object.keys(changedAcc)) delete changedAcc[k];
  if (!S.ui) return;
  try {
    Chrome.render(changed);
    Stage.render(changed);
    Timeline.render(changed);
    Panels.render(changed);
    XflWindow.onState(changed);
  } catch (e) {
    console.error(e);
    Chrome.status('Ошибка интерфейса: ' + e.message);
  }
}

// Такт анимации: пока что-то движется (воспроизведение, частицы, тряска,
// моргание), Rust получает dt, иначе цикл ничего не шлёт.
let lastTick = 0;
function tickLoop(t) {
  requestAnimationFrame(tickLoop);
  if (!S.ui || !S.ui.busy) { lastTick = 0; return; }
  const busyTick = (Net.inflight && Net.inflight.cmd === 'tick') || Net.queue.some(m => m.cmd === 'tick');
  if (busyTick) return;
  const dt = lastTick ? Math.min((t - lastTick) / 1000, 0.1) : 1 / 60;
  lastTick = t;
  rpc('tick', { dt });
}

// ---------------------------------------------------------------- действия

const Actions = {
  async open() {
    const p = await fileDialog('open', { start: S.ui.asset_path });
    if (p) await rpc('open', { path: p });
  },
  async save() {
    const r = await rpc('save');
    if (r && r.need_path) return this.saveAs();
    return !!(r && r.saved);
  },
  async saveAs() {
    const p = await fileDialog('save', { start: S.ui.asset_path, name: S.ui.names.asset });
    if (!p) return false;
    const r = await rpc('save', { path: p });
    return !!(r && r.saved);
  },
  async importLayer() {
    const p = await fileDialog('layer', { start: S.ui.asset_path });
    if (p) await rpc('import_layer', { path: p });
  },
  async importAnimate() {
    XflWindow.open();
    if (!S.xfl.loaded) {
      const p = await fileDialog('animate', { start: S.xfl.path });
      if (p) await rpc('xfl_load', { path: p });
    }
  },
  async export(kind) {
    const p = await fileDialog(kind === 'gif' ? 'gif' : 'sheet', { start: S.ui.asset_path, name: kind === 'gif' ? S.ui.names.gif : S.ui.names.png });
    if (!p) return;
    await rpc('export', { kind, path: p });
  },
  zoomIn() { Stage.setZoom(Stage.zoom * 1.25); },
  zoomOut() { Stage.setZoom(Stage.zoom / 1.25); },
  zoom100() { Stage.setZoom(1); },
  escape() {
    if (Stage.penCancel()) return;
    rpc('escape');
  },
  deleteKey() {
    if (S.ui.tool === 'subsel' && S.ui.selected_shape != null) rpc('delete_shape');
    else if (S.ui.selected_keyframe) rpc('tl_delete_key');
  },
  showDebug() {
    const d = S.doc.counts;
    const w = Modal.open({
      title: 'Отладочная информация', iconName: 'info',
      body: h('div', { class: 'kv', style: { minWidth: '360px' } }, ...[
        ['Рендер сцены', 'WebView, SVG'], ['Экспорт', 'resvg (CPU), видеокарта не нужна'], ['User agent', navigator.userAgent],
        ['Частей', d.parts], ['Костей', d.bones], ['Анимаций', d.anims], ['Фигур на холсте', d.shapes], ['Живых частиц', S.env.alive],
        ['Undo / Redo', `${S.ui.undo} / ${S.ui.redo}`], ['Очередь команд', Net.queue.length],
      ].flatMap(([k, v]) => [h('span', null, k), h('span', { style: { whiteSpace: 'normal' } }, String(v))])),
      foot: [button('Закрыть', () => Modal.close(w), { primary: true })],
    });
  },
};

// ---------------------------------------------------------------- инструменты

const TOOL_INFO = {
  sel: { name: 'Selection', key: 'V', hint: 'клик — выбрать часть, тащить — двигать её, двойной клик по SVG-части — редактировать' },
  subsel: { name: 'SubSelection', key: 'A', hint: 'клик — выбрать фигуру холста, тащить узлы, Delete — удалить фигуру' },
  xform: { name: 'Free Transform', key: 'Q', hint: 'тащить — двигать кость части, Shift — вращать, Alt+клик — кость-родитель (вся голова)' },
  lasso: { name: 'Lasso', key: 'L', hint: 'рамкой выделить несколько частей (групповые операции — меню Edit)' },
  pen: { name: 'Pen', key: 'P', hint: 'клик — точка, клик по первой — замкнуть, Enter — закончить, Esc — отмена' },
  pencil: { name: 'Pencil', key: 'Y', hint: 'рисовать тонкой линией' },
  brush: { name: 'Brush', key: 'B', hint: 'рисовать толстой линией' },
  line: { name: 'Line', key: 'N', hint: 'тащить — отрезок' },
  rect: { name: 'Rectangle', key: 'R', hint: 'тащить — прямоугольник' },
  oval: { name: 'Oval', key: 'O', hint: 'тащить — эллипс' },
  poly: { name: 'PolyStar', key: '', hint: 'тащить от центра — правильный многоугольник' },
  bucket: { name: 'Paint Bucket', key: 'K', hint: 'клик по фигуре холста — залить цветом Fill' },
  ink: { name: 'Ink Bottle', key: 'S', hint: 'клик по фигуре холста — обводка цветом Stroke' },
  drop: { name: 'Eyedropper', key: 'I', hint: 'клик — взять цвет со сцены в Fill' },
  hand: { name: 'Hand', key: 'H', hint: 'тащить — панорама (средняя кнопка работает в любом инструменте)' },
  zoom: { name: 'Zoom', key: 'Z', hint: 'клик — приблизить, Alt+клик / правый клик — отдалить' },
};
const TOOL_GROUPS = [['sel', 'subsel', 'xform', 'lasso'], ['pen', 'pencil', 'brush', 'line', 'rect', 'oval', 'poly'], ['bucket', 'ink', 'drop'], ['hand', 'zoom']];

// ---------------------------------------------------------------- меню и хром

const sc = (k, label, fn, opts = {}) => ({ label, fn, sc: k, ...opts });

function menuDefs() {
  const ui = S.ui, d = S.doc;
  const hasSel = !!ui.selected_layer, multi = ui.multi.length;
  const anims = d.anims.map(n => ({ label: n, fn: () => rpc('play_anim', { name: n, start: true }), checked: S.anim.name === n, icon: 'play' }));
  return [
    ['Файл', [
      sc('Ctrl+N', 'Новый', () => rpc('new'), { icon: 'new' }),
      sc('Ctrl+O', 'Открыть…', () => Actions.open(), { icon: 'open' }),
      sc('Ctrl+S', 'Сохранить', () => Actions.save(), { icon: 'save' }),
      sc('Ctrl+Shift+S', 'Сохранить как…', () => Actions.saveAs()),
      { note: ui.asset_path ? 'Файл: ' + ui.asset_path : 'Документ ещё не сохранён' },
      '-',
      { label: 'Демо-пони (пример)', fn: () => rpc('demo'), icon: 'pony' },
      '-',
      { label: 'Импорт Adobe Animate (.fla / .xfl)…', fn: () => Actions.importAnimate(), icon: 'animate' },
      { label: 'Импорт картинки как слоя (PNG, SVG, PSD, KRA)…', fn: () => Actions.importLayer(), icon: 'image' },
      '-',
      { label: 'Экспорт GIF… (текущая анимация)', fn: () => Actions.export('gif'), icon: 'export' },
      { label: 'Экспорт спрайт-листа… (PNG)', fn: () => Actions.export('sheet') },
      '-',
      { label: 'Выход', fn: () => (ui.dirty ? __pony.confirmClose() : rpc('exit')) },
    ]],
    ['Правка', [
      sc('Ctrl+Z', `Отменить (${ui.undo})`, () => rpc('undo'), { icon: 'undo', disabled: !ui.undo }),
      sc('Ctrl+Shift+Z', `Вернуть (${ui.redo})`, () => rpc('redo'), { icon: 'redo', disabled: !ui.redo }),
      '-',
      { label: 'Удалить слой', fn: () => rpc('delete_layer'), icon: 'trash', disabled: !hasSel },
      { label: 'Дублировать слой', fn: () => rpc('duplicate_layer'), disabled: !hasSel },
      sc('Esc', 'Снять выделение', () => rpc('deselect'), { disabled: !hasSel }),
      ...(multi ? ['-', { note: `Выделено лассо: ${multi} частей` },
        { label: 'Скрыть группу', fn: () => rpc('multi', { op: 'hide' }), icon: 'eyeoff' },
        { label: 'Показать группу', fn: () => rpc('multi', { op: 'show' }), icon: 'eye' },
        { label: 'Удалить группу', fn: () => rpc('multi', { op: 'delete' }), icon: 'trash' },
        { label: 'Group (создать постоянную группу)', fn: () => rpc('multi', { op: 'group' }), icon: 'group' },
        { label: 'Снять групповое выделение', fn: () => rpc('multi', { op: 'clear' }) }] : []),
    ]],
    ['Вид', [
      sc('Ctrl+=', 'Приблизить', () => Actions.zoomIn(), { icon: 'zoom' }),
      sc('Ctrl+−', 'Отдалить', () => Actions.zoomOut()),
      sc('Ctrl+1', 'Масштаб 100%', () => Actions.zoom100()),
      sc('Ctrl+2', 'Весь документ', () => Stage.fit('document')),
      sc('Ctrl+3', 'Персонаж целиком', () => Stage.fit('character'), { icon: 'fit' }),
      '-',
      { label: 'Линейки', fn: () => App.setRulers(!App.rulers), checked: App.rulers },
      { label: 'Таймлайн', fn: () => App.showTimeline(!App.timelineOn), checked: App.timelineOn },
      { label: 'Правая панель', fn: () => App.showPanel(!App.panelOn), checked: App.panelOn },
      { label: 'Скелет', fn: () => rpc('show_skeleton', { on: !ui.show_skeleton }), checked: ui.show_skeleton },
    ]],
    ['Вставка', [
      sc('F6', `Ключевой кадр (${ui.channel_label})`, () => rpc('insert_keyframe'), { icon: 'key', disabled: !hasSel }),
      { label: 'Пустой слой (заглушка цветом)', fn: () => rpc('blank_layer'), icon: 'layers' },
      { label: 'Кость (кликом на сцене)', fn: () => { rpc('placing_bone', { on: true }); Panels.show('bones'); }, icon: 'bone' },
      { label: 'Новая анимация', fn: () => rpc('anim_new'), icon: 'plus' },
      { label: 'Новый персонаж в сцене', fn: () => rpc('char_new'), icon: 'char' },
    ]],
    ['Изменить', [
      { label: 'Отразить по горизонтали', fn: () => rpc('modify', { op: 'flip_h' }), disabled: !hasSel },
      { label: 'Отразить по вертикали', fn: () => rpc('modify', { op: 'flip_v' }), disabled: !hasSel },
      { label: 'Сбросить трансформацию', fn: () => rpc('modify', { op: 'reset' }), disabled: !hasSel },
      '-',
      { label: 'Поднять слой выше', fn: () => rpc('modify', { op: 'up' }), icon: 'up', disabled: !hasSel },
      { label: 'Опустить слой ниже', fn: () => rpc('modify', { op: 'down' }), disabled: !hasSel },
      '-',
      { label: 'Combine Paths…', fn: () => Panels.show('combine'), icon: 'combine' },
      { label: 'Convert to Symbol…', fn: () => Panels.show('symbols'), icon: 'symbol' },
      { label: 'Градиенты…', fn: () => Panels.show('gradients'), icon: 'gradient' },
    ]],
    ['Команды', [
      { label: 'Выполнить скрипт из вкладки «Скрипт»', fn: () => rpc('script_run', { text: F.script }), icon: 'script' },
      '-', { note: 'Готовые примеры:' },
      ...SCRIPT_EXAMPLES.map(([l, src]) => ({ label: l, fn: () => { F.script = src; Panels.show('script'); } })),
    ]],
    ['Управление', [
      sc('Space', S.play.playing ? 'Стоп' : 'Воспроизвести', () => rpc('toggle_play'), { icon: S.play.playing ? 'pause' : 'play' }),
      sc('Home', 'В начало', () => rpc('seek', { op: 'start' }), { icon: 'first' }),
      sc('End', 'В конец анимации', () => rpc('seek', { op: 'end' }), { icon: 'last' }),
      sc('←', 'Кадр назад', () => rpc('seek', { op: 'prev' }), { keep: true }),
      sc('→', 'Кадр вперёд', () => rpc('seek', { op: 'next' }), { keep: true }),
      '-', { note: anims.length ? 'Анимации персонажа:' : 'У персонажа нет анимаций' },
      ...anims,
    ]],
    ['Окно', TABS.map(t => ({ label: t.label, fn: () => Panels.show(t.id), icon: t.icon, checked: App.panelOn && Panels.tab === t.id }))],
    ['Справка', [
      { label: 'Горячие клавиши', fn: helpDialog, icon: 'info' },
      { label: 'Отладочная информация', fn: () => Actions.showDebug() },
      { label: 'О программе', fn: aboutDialog, icon: 'pony' },
    ]],
  ];
}

const Chrome = {
  openMenu: null,

  init() {
    this.menus = document.getElementById('menus');
    this.toolbar = document.getElementById('toolbar');
    this.tools = document.getElementById('tools');
    this.statusEl = document.getElementById('statusbar');
    this.docname = document.getElementById('docname');
    document.addEventListener('pointerdown', e => { if (this.openMenu && !e.target.closest('.menu')) this.closeMenus(); });
  },

  render(changed) {
    if (changed.ui || changed.doc || changed.anim || changed.play) this.renderMenus();
    if (changed.ui) { this.renderToolbar(); this.renderTools(); this.renderDocname(); this.renderBanner(); }
    if (changed.vec) this.renderBanner();
    if (changed.ui || changed.doc) this.renderStatus();
  },

  renderMenus() {
    const defs = menuDefs();
    fill(this.menus, ...defs.map(([title, items], mi) => {
      const dd = h('div', { class: 'dropdown' }, ...items.map(it => {
        if (it === '-') return h('div', { class: 'msep' });
        if (it.note) return h('div', { class: 'mnote' }, it.note);
        const el = h('div', { class: 'mi' + (it.disabled ? ' disabled' : '') + (it.checked ? ' checked' : '') },
          it.checked ? icon('check') : it.icon ? icon(it.icon) : icon('x', 'empty'), h('span', null, it.label), it.sc ? h('span', { class: 'sc' }, it.sc) : null);
        if (!it.disabled) el.addEventListener('click', () => { if (!it.keep) this.closeMenus(); it.fn(); });
        return el;
      }));
      const menu = h('div', { class: 'menu' + (this.openMenu === mi ? ' open' : '') }, h('div', { class: 'menu-title' }, title), dd);
      const t = menu.firstChild;
      t.addEventListener('pointerdown', e => { e.preventDefault(); this.openMenu === mi ? this.closeMenus() : this.setOpen(mi); });
      t.addEventListener('pointerenter', () => { if (this.openMenu != null && this.openMenu !== mi) this.setOpen(mi); });
      return menu;
    }));
  },
  setOpen(i) {
    this.openMenu = i;
    [...this.menus.children].forEach((m, j) => m.classList.toggle('open', j === i));
  },
  closeMenus() { this.openMenu = null; [...this.menus.children].forEach(m => m.classList.remove('open')); },

  renderToolbar() {
    if (!S.ui) return;
    const ui = S.ui;
    const tb = (ic, title, fn, opts = {}) => {
      const b = h('button', { class: 'tbtn' + (opts.on ? ' on' : ''), title, disabled: opts.disabled, type: 'button' }, icon(ic), opts.label ? h('span', null, opts.label) : null);
      b.addEventListener('click', fn);
      return b;
    };
    this.zoomRange = h('input', { type: 'range', min: 0, max: 1000, step: 1 });
    this.zoomVal = h('span', { class: 'zoomval' });
    this.zoomRange.addEventListener('input', () => Stage.setZoom(MIN_ZOOM * Math.pow(MAX_ZOOM / MIN_ZOOM, this.zoomRange.value / 1000)));
    fill(this.toolbar, 
      tb('new', 'Новый (Ctrl+N)', () => rpc('new')),
      tb('open', 'Открыть… (Ctrl+O)', () => Actions.open()),
      tb('save', ui.asset_path ? 'Сохранить (Ctrl+S) — ' + ui.asset_path : 'Сохранить… (Ctrl+S)', () => Actions.save()),
      h('span', { class: 'tb-sep' }),
      tb('undo', 'Отменить (Ctrl+Z)', () => rpc('undo'), { disabled: !ui.undo }),
      tb('redo', 'Вернуть (Ctrl+Shift+Z)', () => rpc('redo'), { disabled: !ui.redo }),
      h('span', { class: 'tb-sep' }),
      tb('animate', 'Импорт Adobe Animate (.fla / .xfl)', () => Actions.importAnimate(), { label: 'Animate' }),
      tb('image', 'Импорт картинки как слоя', () => Actions.importLayer(), { label: 'Картинка' }),
      tb('export', 'Экспорт текущей анимации в GIF', () => Actions.export('gif'), { label: 'GIF' }),
      h('span', { class: 'tb-sep' }),
      tb('pony', 'Демо-персонаж с анимацией Walk', () => rpc('demo'), { label: 'Демо' }),
      h('span', { class: 'tb-spacer' }),
      h('span', { class: 'zoombox' }, icon('zoom'), this.zoomRange, this.zoomVal),
      tb('fit', 'Персонаж целиком (Ctrl+3)', () => Stage.fit('character')),
      tb('rect', 'Весь документ (Ctrl+2)', () => Stage.fit('document')),
      h('span', { class: 'tb-sep' }),
      tb('layers', 'Таймлайн', () => App.showTimeline(!App.timelineOn), { on: App.timelineOn }),
      tb('sliders', 'Правая панель', () => App.showPanel(!App.panelOn), { on: App.panelOn }),
    );
    this.renderZoom();
  },
  renderZoom() {
    if (!this.zoomRange) return;
    this.zoomRange.value = Math.log(Stage.zoom / MIN_ZOOM) / Math.log(MAX_ZOOM / MIN_ZOOM) * 1000;
    this.zoomVal.textContent = Math.round(Stage.zoom * 100) + '%';
    if (S.ui) this.renderStatus();
  },

  renderTools() {
    const ui = S.ui;
    const kids = [];
    TOOL_GROUPS.forEach((g, gi) => {
      if (gi) kids.push(h('div', { class: 'tool-sep' }));
      for (const t of g) {
        const info = TOOL_INFO[t];
        const el = h('div', { class: 'tool' + (ui.tool === t ? ' on' : ''), title: `${info.name}${info.key ? ' (' + info.key + ')' : ''}\n${info.hint}` }, icon(t));
        el.addEventListener('click', () => { if (t !== 'pen') Stage.penCancel(); rpc('set_tool', { tool: t }); });
        kids.push(el);
      }
    });
    kids.push(h('div', { class: 'tool-sep' }));
    const sw = (cls, key, title) => {
      const input = h('input', { type: 'color', value: rgbHex(ui[key]), title });
      input.addEventListener('change', () => rpc('set_colors', { [key]: [...hexRgb(input.value), ui[key][3]] }));
      return h('div', { class: 'swatch ' + cls, style: { background: rgbaCss(ui[key]) }, title }, input);
    };
    kids.push(h('div', { class: 'swatches' }, sw('stroke', 'stroke', 'Stroke — цвет обводки'), sw('fill', 'fill', 'Fill — цвет заливки')));
    if (ui.tool === 'poly') {
      const inp = h('input', { type: 'number', min: 3, max: 12, value: ui.poly_sides, title: 'Число углов PolyStar' });
      inp.addEventListener('change', () => rpc('poly_sides', { n: +inp.value }));
      kids.push(h('div', { class: 'tool-mini' }, 'углов', inp));
    }
    fill(this.tools, ...kids);
  },

  renderBanner() {
    const stage = document.getElementById('stage');
    stage.querySelectorAll('.edit-banner').forEach(e => e.remove());
    if (!S.ui || !S.vec) return;
    const n = S.vec.shapes.length;
    if (!S.ui.editing_part && !n) return;
    const banner = h('div', { class: 'edit-banner' }, icon('svg'),
      S.ui.editing_part ? h('span', null, 'Редактируется ', h('b', null, S.ui.editing_part), ` — ${n} фигур`) : h('span', null, `Холст рисования: ${n} фигур`),
      button(S.ui.editing_part ? 'Сохранить часть' : 'Сохранить как слой', () => rpc('save_drawing'), { primary: true, class: 'small primary', icon: 'save', disabled: !n }),
      S.ui.editing_part ? button('Отмена', () => rpc('cancel_edit'), { class: 'small' }) : button('Очистить', () => rpc('clear_canvas'), { class: 'small' }));
    stage.appendChild(banner);
  },

  renderDocname() {
    const ui = S.ui;
    fill(this.docname, h('b', null, ui.active_name), ' — ', ui.asset_path || 'Без имени', ui.dirty ? h('span', { class: 'dirty' }, ' ●') : null);
  },

  renderStatus() {
    const ui = S.ui;
    const info = TOOL_INFO[ui.tool] || { name: ui.tool, hint: '' };
    const msg = h('span', { class: 'sb msg', title: this.lastStatus || ui.status }, this.lastStatus || ui.status);
    fill(this.statusEl, 
      h('span', { class: 'sb', title: info.hint }, icon(ui.tool), h('b', null, info.name)),
      h('span', { class: 'sb' }, 'Слой: ', h('b', null, ui.selected_layer || '—')),
      ui.selected_bone ? h('span', { class: 'sb' }, icon('bone'), h('b', null, ui.selected_bone)) : null,
      h('span', { class: 'sb' }, `${App.scene[0]}×${App.scene[1]}`),
      h('span', { class: 'sb' }, Math.round(Stage.zoom * 100) + '%'),
      ui.exporting ? h('span', { class: 'sb', style: { color: 'var(--gold)' } }, icon('export'), 'экспорт…') : null,
      msg);
    this.lastStatus = null;
  },
  status(text) {
    if (!text) return;
    this.lastStatus = text;
    if (S.ui) this.renderStatus();
  },
  flash() {
    requestAnimationFrame(() => { const m = this.statusEl.querySelector('.msg'); if (m) { m.classList.remove('flash'); void m.offsetWidth; m.classList.add('flash'); } });
  },
};

// ---------------------------------------------------------------- клавиши

const TOOL_KEYS = { v: 'sel', a: 'subsel', q: 'xform', l: 'lasso', p: 'pen', y: 'pencil', b: 'brush', n: 'line', r: 'rect', o: 'oval', k: 'bucket', s: 'ink', i: 'drop', h: 'hand', z: 'zoom' };

window.addEventListener('keydown', e => {
  if (!S.ui) return;
  const top = Modal.top();
  if (top && e.key === 'Escape') { e.preventDefault(); Modal.close(top.win); return; }
  if (Modal.blocking()) return;
  const t = e.target;
  const editing = t && (t.tagName === 'TEXTAREA' || (t.tagName === 'INPUT' && !['checkbox', 'range', 'color', 'button'].includes(t.type)) || t.isContentEditable);
  if (editing) { if (e.key === 'Escape') t.blur(); return; }
  if (Chrome.openMenu != null && e.key === 'Escape') { Chrome.closeMenus(); return; }
  const ctrl = e.ctrlKey || e.metaKey;
  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  const code = e.code;
  let handled = true;
  if (ctrl) {
    if (code === 'KeyS' && e.shiftKey) Actions.saveAs();
    else if (code === 'KeyZ' && e.shiftKey) rpc('redo');
    else if (code === 'KeyZ') rpc('undo');
    else if (code === 'KeyY') rpc('redo');
    else if (code === 'KeyS') Actions.save();
    else if (code === 'KeyO') Actions.open();
    else if (code === 'KeyN') rpc('new');
    else if (code === 'Digit1') Actions.zoom100();
    else if (code === 'Digit2') Stage.fit('document');
    else if (code === 'Digit3') Stage.fit('character');
    else if (code === 'Equal' || code === 'NumpadAdd') Actions.zoomIn();
    else if (code === 'Minus' || code === 'NumpadSubtract') Actions.zoomOut();
    else handled = false;
  } else if (e.altKey) {
    handled = false;
  } else if (key === ' ') rpc('toggle_play');
  else if (key === 'Home') rpc('seek', { op: 'start' });
  else if (key === 'End') rpc('seek', { op: 'end' });
  else if (key === 'ArrowLeft' || key === ',') rpc('seek', { op: 'prev' });
  else if (key === 'ArrowRight' || key === '.') rpc('seek', { op: 'next' });
  else if (key === 'F6') rpc('insert_keyframe');
  else if (key === 'Delete' || key === 'Backspace') Actions.deleteKey();
  else if (key === 'Escape') Actions.escape();
  else if (key === 'Enter' && Stage.pen.length >= 2) Stage.penCommit(false);
  else if (!e.shiftKey && TOOL_KEYS[code.replace('Key', '').toLowerCase()] && code.startsWith('Key')) {
    const tool = TOOL_KEYS[code.replace('Key', '').toLowerCase()];
    if (tool !== 'pen') Stage.penCancel();
    rpc('set_tool', { tool });
  } else handled = false;
  if (handled) e.preventDefault();
});

// Ctrl+колесо не должно масштабировать саму страницу.
window.addEventListener('wheel', e => { if (e.ctrlKey) e.preventDefault(); }, { passive: false });
document.addEventListener('contextmenu', e => { if (!e.target.closest('input, textarea')) e.preventDefault(); });

// ---------------------------------------------------------------- разделители

function initSplitters() {
  const root = document.documentElement;
  try {
    const pw = localStorage.getItem('pony.panelW'), th = localStorage.getItem('pony.tlH');
    if (pw) root.style.setProperty('--panel-w', pw);
    if (th) root.style.setProperty('--tl-h', th);
  } catch (e) { /* ок */ }
  const drag = (el, apply) => el.addEventListener('pointerdown', e => {
    e.preventDefault();
    el.classList.add('drag');
    const move = ev => { apply(ev); Stage.resize(); Timeline.layout(); };
    const up = () => { el.classList.remove('drag'); window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', up); try { localStorage.setItem('pony.panelW', root.style.getPropertyValue('--panel-w')); localStorage.setItem('pony.tlH', root.style.getPropertyValue('--tl-h')); } catch (er) { /* ок */ } };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  });
  drag(document.getElementById('split-right'), ev => root.style.setProperty('--panel-w', clamp(innerWidth - ev.clientX, 240, innerWidth * 0.6) + 'px'));
  drag(document.getElementById('split-timeline'), ev => root.style.setProperty('--tl-h', clamp(innerHeight - ev.clientY - 26, 90, innerHeight * 0.7) + 'px'));
}

// ---------------------------------------------------------------- старт

// Ошибки JavaScript — в консоль приложения (stderr), иначе их не видно.
window.addEventListener('error', e => rpc('log', { text: `${e.message} @ ${e.filename}:${e.lineno}` }));
window.addEventListener('unhandledrejection', e => rpc('log', { text: 'promise: ' + (e.reason && e.reason.stack || e.reason) }));

window.addEventListener('DOMContentLoaded', async () => {
  Chrome.init();
  Stage.init();
  Timeline.init();
  Panels.init();
  initSplitters();
  requestAnimationFrame(tickLoop);
  const hello = await rpc('hello');
  if (hello) App.scene = hello.scene;
});
