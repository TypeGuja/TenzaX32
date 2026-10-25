// Правая панель: вкладки свойств. Всё, что меняет документ, уходит в Rust
// командой; сами формы (имя нового градиента, выбор для IK и т.п.) живут
// здесь, в `F`, пока не нажата кнопка.
'use strict';

const TABS = [
  { id: 'props', label: 'Свойства слоя', icon: 'sliders', deps: ['ui', 'doc'] },
  { id: 'xform', label: 'Трансформация', icon: 'move', deps: ['ui', 'doc'] },
  { id: 'bones', label: 'Кости и IK', icon: 'bone', deps: ['ui', 'doc'] },
  { id: 'library', label: 'Библиотека ассетов', icon: 'library', deps: ['doc'] },
  { id: 'color', label: 'Цвет', icon: 'palette', deps: ['ui'] },
  { id: 'light', label: 'Освещение', icon: 'sun', deps: ['env'] },
  { id: 'particles', label: 'Частицы', icon: 'sparkle', deps: ['env'] },
  { id: 'script', label: 'Скрипт', icon: 'script', deps: ['ui'] },
  { id: 'svg', label: 'SVG-части', icon: 'svg', deps: ['doc', 'ui'] },
  { id: 'symbols', label: 'Символы', icon: 'symbol', deps: ['vec', 'ui'] },
  { id: 'gradients', label: 'Градиенты', icon: 'gradient', deps: ['vec', 'ui'] },
  { id: 'combine', label: 'Combine Paths', icon: 'combine', deps: ['vec'] },
  { id: 'groups', label: 'Группы', icon: 'group', deps: ['doc'] },
  { id: 'info', label: 'Инфо', icon: 'info', deps: ['ui', 'doc', 'vec', 'env', 'frame'] },
];

const SCRIPT_EXAMPLES = [
  ['Улыбнуться и моргнуть', 'pony.Smile(0.9);\npony.Blink();'],
  ['Посмотреть вверх', 'pony.Look(0.0, 1.0);'],
  ['Пойти', 'pony.Walk();'],
  ['Тряска камеры', 'camera.Shake(0.6);'],
  ['Приблизить камеру', 'camera.Zoom(1.3);'],
];

const F = {
  script: '// Пример — жми «Выполнить» (Ctrl+Enter)\npony.Smile(0.8);\npony.Blink();\ncamera.Zoom(1.2);',
  boneName: '',
  boneNameFor: null,
  ik: { root: null, mid: null, tip: null },
  symbolName: '',
  grad: null,
  boolFront: null, boolBack: null, boolOp: 'union',
};

function newGradForm() {
  return { editing: null, name: '', radial: false, x1: 0, y1: 0, x2: 100, y2: 0, cx: 50, cy: 50, r: 50, stops: [{ offset: 0, color: [255, 255, 255, 255] }, { offset: 1, color: [0, 0, 0, 255] }] };
}
F.grad = newGradForm();

const Panels = {
  tab: 'props',
  lastRender: 0,
  pending: false,

  init() {
    this.tabsEl = document.getElementById('panel-tabs');
    this.body = document.getElementById('panel-body');
    try { this.tab = localStorage.getItem('pony.tab') || 'props'; } catch (e) { /* без хранилища */ }
    if (!TABS.some(t => t.id === this.tab)) this.tab = 'props';
    this.renderTabs();
  },

  show(tab) {
    this.tab = tab;
    try { localStorage.setItem('pony.tab', tab); } catch (e) { /* ок */ }
    App.showPanel(true);
    this.renderTabs();
    this.renderBody();
    Stage.drawOverlay();
  },

  renderTabs() {
    fill(this.tabsEl, ...TABS.map(t => {
      const el = h('div', { class: 'ptab' + (t.id === this.tab ? ' on' : ''), title: t.label }, icon(t.icon), t.id === this.tab ? t.label : null);
      el.addEventListener('click', () => this.show(t.id));
      return el;
    }));
  },

  render(changed) {
    if (!S.ui) return;
    const t = TABS.find(x => x.id === this.tab);
    if (!t.deps.some(d => changed[d])) return;
    // Во время воспроизведения поза меняется каждый кадр — панель
    // обновляем не чаще ~8 раз в секунду.
    const now = performance.now();
    if (S.ui.busy && now - this.lastRender < 120) {
      if (!this.pending) { this.pending = true; setTimeout(() => { this.pending = false; this.renderBody(); }, 130); }
      return;
    }
    this.renderBody();
  },

  onMouse() {
    if (this.tab === 'info' && !this.mouseQueued) {
      this.mouseQueued = true;
      requestAnimationFrame(() => { this.mouseQueued = false; this.renderBody(); });
    }
  },

  renderBody() {
    if (!S.ui) return;
    this.lastRender = performance.now();
    const t = TABS.find(x => x.id === this.tab);
    replaceKeepingFocus(this.body, () => [h('h3', null, icon(t.icon), t.label), ...[].concat(this['tab_' + t.id]())]);
  },

  part() { return S.ui.selected_layer && S.doc.parts.find(p => p.id === S.ui.selected_layer); },
  bone(id) { return S.doc.bones.find(b => b.id === id); },
  boneOptions(withNone) {
    const opts = S.doc.bones.map(b => [b.id, b.id]);
    return withNone ? [[null, '—'], ...opts] : opts;
  },

  // ------------------------------------------------------------ вкладки

  tab_props() {
    const p = this.part();
    if (!p) {
      const d = S.doc;
      return [
        h('div', { class: 'empty-state' }, icon('layers'), 'Слой не выбран', h('div', { class: 'hint' }, 'Кликни по части на сцене (инструмент Selection, V) или по строке в таймлайне.')),
        group('Персонаж', h('div', { class: 'kv' },
          h('span', null, 'Имя'), h('span', null, S.ui.active_name),
          h('span', null, 'Частей'), h('span', null, d.counts.parts),
          h('span', null, 'Костей'), h('span', null, d.counts.bones),
          h('span', null, 'Анимаций'), h('span', null, d.counts.anims))),
        group('С чего начать',
          row(button('Открыть…', () => Actions.open(), { icon: 'open' }), button('Демо-пони', () => rpc('demo'), { icon: 'pony' })),
          row(button('Импорт Animate…', () => Actions.importAnimate(), { icon: 'animate' }), button('Картинка как слой…', () => Actions.importLayer(), { icon: 'image' }))),
      ];
    }
    const id = p.id;
    const set = (args, fin = true) => rpc('part_set', { id, ...args }, { down: !fin });
    const masks = S.doc.parts.filter(o => o.id !== id && !o.clip_by).map(o => [o.id, o.id]);
    return [
      h('div', { class: 'card' },
        h('div', { class: 'card-title' }, icon('layers'), id, h('span', { class: 'badge' }, p.kind), S.ui.editing_part === id ? h('span', { class: 'badge green' }, 'редактируется') : null),
        h('div', { class: 'hint', style: { margin: '4px 0 0', wordBreak: 'break-all' } }, p.source)),
      p.svg ? row(button('Редактировать SVG', () => rpc('edit_svg', { id }), { icon: 'svg', primary: true, title: 'Открыть фигуры этой части на холсте (инструмент SubSelection)' })) : null,
      group('Привязка',
        field('Кость', select({ key: 'p-bone', value: p.bone, options: this.boneOptions(!p.bone), onChange: v => v && set({ bone: v }), title: 'Часть останется на месте на экране' })),
        field('Смещение', numField({ key: 'p-px', label: 'X', value: p.pivot[0], step: 0.5, onChange: (v, f) => set({ px: v }, f) }), numField({ key: 'p-py', label: 'Y', value: p.pivot[1], step: 0.5, onChange: (v, f) => set({ py: v }, f) }))),
      group('Размер и порядок',
        field('Размер', numField({ key: 'p-w', label: 'Ш', value: p.size[0], step: 0.5, min: 1, max: 1000, onChange: (v, f) => set({ w: v }, f) }), numField({ key: 'p-h', label: 'В', value: p.size[1], step: 0.5, min: 1, max: 1000, onChange: (v, f) => set({ h: v }, f) })),
        p.has_size ? row(button('Сбросить размер к виду части', () => set({ reset_size: true }), { class: 'small' })) : null,
        field('Порядок', numField({ key: 'p-layer', label: 'слой', value: p.layer, step: 0.1, digits: 0, onChange: (v, f) => set({ layer: Math.round(v) }, f) }),
          button('', () => rpc('modify', { op: 'up' }), { icon: 'up', class: 'small', title: 'Поднять выше' }),
          button('', () => rpc('modify', { op: 'down' }), { icon: 'chev', class: 'small', title: 'Опустить ниже' }))),
      group('Маска (Clip by)',
        field('Маска', select({ key: 'p-mask', value: p.clip_by, options: [[null, '(нет)'], ...masks], onChange: v => set({ clip_by: v || '' }) })),
        hint('Маска обрезает часть по альфе другой части, как маскирующий слой в Animate. Сама маска продолжает рисоваться.')),
      group('Действия',
        row(button('Дублировать', () => rpc('duplicate_layer'), { class: 'small' }), button('Отразить ↔', () => rpc('modify', { op: 'flip_h' }), { class: 'small' }), button('Отразить ↕', () => rpc('modify', { op: 'flip_v' }), { class: 'small' })),
        row(button('Сбросить трансформацию кости', () => rpc('modify', { op: 'reset' }), { class: 'small' }), button('Удалить слой', () => rpc('delete_layer'), { class: 'small danger', icon: 'trash' }))),
      hint('Selection (V): тащить — двигать саму часть. Free Transform (Q): двигать/вращать её кость.'),
    ];
  },

  tab_xform() {
    const p = this.part();
    const out = [group('Поворот персонажа (2.5D)',
      slider({ key: 'yaw', label: 'Yaw', value: S.doc.facing_yaw, min: -89, max: 89, step: 1, digits: 0, suffix: '°', onChange: (v, f) => rpc('facing_yaw', { deg: v }, { down: !f }) }),
      hint('Части смещаются по глубине (номеру слоя) — персонаж «поворачивается» без отдельного рисунка.'))];
    if (!p) out.push(hint('Выбери слой, чтобы править трансформацию его кости.'));
    else if (!p.bone) out.push(hint('У выбранного слоя нет кости.'));
    else out.push(group('Кость ' + p.bone, ...this.boneFields(this.bone(p.bone))));
    return out;
  },

  boneFields(b) {
    if (!b) return [];
    const set = (args, f) => rpc('bone_set', { id: b.id, ...args }, { down: !f });
    return [
      field('Позиция', numField({ key: 'b-x', label: 'X', value: b.x, step: 0.5, onChange: (v, f) => set({ x: v }, f) }), numField({ key: 'b-y', label: 'Y', value: b.y, step: 0.5, onChange: (v, f) => set({ y: v }, f) })),
      field('Поворот', numField({ key: 'b-rot', label: '∠', value: b.rot, step: 1, digits: 1, suffix: '°', onChange: (v, f) => set({ rot: v }, f) })),
      field('Масштаб', numField({ key: 'b-sx', label: 'X', value: b.sx, step: 0.01, digits: 3, onChange: (v, f) => set({ sx: v }, f) }), numField({ key: 'b-sy', label: 'Y', value: b.sy, step: 0.01, digits: 3, onChange: (v, f) => set({ sy: v }, f) })),
    ];
  },

  tab_bones() {
    const ui = S.ui, bones = S.doc.bones;
    const children = {};
    for (const b of bones) (children[b.parent || ''] = children[b.parent || ''] || []).push(b);
    const items = [];
    const walk = (parent, depth) => {
      for (const b of (children[parent] || []).sort((a, c) => a.id.localeCompare(c.id))) {
        const el = h('div', { class: 'list-item' + (ui.selected_bone === b.id ? ' on' : ''), style: { paddingLeft: 8 + depth * 14 + 'px' } }, icon('bone'), h('span', { class: 'grow' }, b.id));
        el.addEventListener('click', () => rpc('select_bone', { id: b.id }));
        items.push(el);
        if (depth < 64) walk(b.id, depth + 1);
      }
    };
    walk('', 0);
    const sel = ui.selected_bone && this.bone(ui.selected_bone);
    const out = [
      row(checkbox({ key: 'show-skel', label: 'Показывать скелет на сцене', checked: ui.show_skeleton, onChange: v => rpc('show_skeleton', { on: v }) })),
      h('div', { class: 'list', style: { maxHeight: '190px' } }, ...items),
      row(button(ui.placing_bone ? 'Кликни на сцене… (Esc — отмена)' : 'Добавить кость', () => rpc('placing_bone', { on: !ui.placing_bone }), { icon: 'plus', class: ui.placing_bone ? 'primary' : '' })),
      hint('Новая кость станет дочерней для выбранной (или корня) в точке клика.'),
    ];
    if (sel) {
      if (F.boneNameFor !== sel.id) { F.boneNameFor = sel.id; F.boneName = sel.id; }
      const parents = bones.filter(b => b.id !== sel.id).map(b => [b.id, b.id]);
      out.push(group('Кость ' + sel.id,
        field('Имя', h('input', { type: 'text', key: 'bone-name', value: F.boneName, oninput: e => { F.boneName = e.target.value; }, onkeydown: e => { if (e.key === 'Enter') rpc('bone_rename', { id: sel.id, name: F.boneName }); } }),
          button('OK', () => rpc('bone_rename', { id: sel.id, name: F.boneName }), { class: 'small' })),
        field('Родитель', sel.parent ? select({ key: 'bone-parent', value: sel.parent, options: parents, onChange: v => v && rpc('bone_reparent', { id: sel.id, parent: v }) }) : h('span', { class: 'hint' }, '(корневая кость)')),
        ...this.boneFields(sel),
        field('Длина', numField({ key: 'b-len', label: 'L', value: sel.length, step: 0.5, min: 0, max: 200, onChange: (v, f) => rpc('bone_set', { id: sel.id, length: v }, { down: !f }) })),
        row(button('Удалить кость с потомками', () => rpc('bone_delete', { id: sel.id }), { class: 'small danger', icon: 'trash', disabled: !sel.parent }))));
    }
    // IK
    const ik = S.doc.ik.map(k => h('div', { class: 'card' },
      h('div', { class: 'card-title' }, k.id, h('span', { class: 'meta' }, `${k.root} → ${k.mid} → ${k.tip}`),
        h('span', { style: { marginLeft: 'auto' } }, checkbox({ key: 'ik-en-' + k.id, label: 'вкл', checked: k.enabled, onChange: v => rpc('ik_set', { id: k.id, enabled: v }) }))),
      field('Цель', numField({ key: 'ik-tx-' + k.id, label: 'X', value: k.tx, step: 0.5, onChange: (v, f) => rpc('ik_set', { id: k.id, tx: v }, { down: !f }) }), numField({ key: 'ik-ty-' + k.id, label: 'Y', value: k.ty, step: 0.5, onChange: (v, f) => rpc('ik_set', { id: k.id, ty: v }, { down: !f }) })),
      slider({ key: 'ik-w-' + k.id, label: 'Weight', value: k.weight, min: 0, max: 1, onChange: (v, f) => rpc('ik_set', { id: k.id, weight: v }, { down: !f }) }),
      row(checkbox({ key: 'ik-pole-' + k.id, label: 'Pole Target', checked: !!k.pole, onChange: v => rpc('ik_set', { id: k.id, pole: v ? true : null }) }),
        k.pole ? numField({ key: 'ik-px-' + k.id, label: 'X', value: k.pole[0], step: 0.5, onChange: (v, f) => rpc('ik_set', { id: k.id, pole: [v, k.pole[1]] }, { down: !f }) }) : null,
        k.pole ? numField({ key: 'ik-py-' + k.id, label: 'Y', value: k.pole[1], step: 0.5, onChange: (v, f) => rpc('ik_set', { id: k.id, pole: [k.pole[0], v] }, { down: !f }) }) : null),
      row(button('Удалить IK', () => rpc('ik_remove', { id: k.id }), { class: 'small danger' }))));
    const distinct = F.ik.root && F.ik.mid && F.ik.tip && new Set([F.ik.root, F.ik.mid, F.ik.tip]).size === 3;
    const det = h('details', { open: S.doc.ik.length > 0 || F.ikOpen }, h('summary', { onclick: () => { F.ikOpen = !F.ikOpen; } }, `IK (Two Bone) — ${S.doc.ik.length}`),
      hint('Цепочка Root → Mid → Tip тянется к точке цели. Пример: Hip → Knee → Hoof.'),
      ...ik,
      group('Новый IK-констрейнт',
        ...['root', 'mid', 'tip'].map(k => field(k[0].toUpperCase() + k.slice(1), select({ key: 'ik-new-' + k, value: F.ik[k], options: this.boneOptions(true), onChange: v => { F.ik[k] = v; this.renderBody(); } }))),
        F.ik.root && F.ik.mid && F.ik.tip && !distinct ? h('div', { class: 'warn' }, icon('warn'), 'Root, Mid и Tip должны быть разными костями') : null,
        row(button('Создать IK-констрейнт', () => rpc('ik_create', F.ik).then(r => { if (r === undefined) return; F.ik = { root: null, mid: null, tip: null }; this.renderBody(); }), { icon: 'plus', disabled: !distinct }))));
    out.push(h('div', { class: 'group' }, det));
    return out;
  },

  tab_library() {
    const lib = S.doc.library;
    if (!lib.length) return [h('div', { class: 'empty-state' }, icon('library'), 'Ассетов нет', h('div', { class: 'hint' }, 'File → Import image as layer или импорт из Adobe Animate.'))];
    return [hint(`Файлы, из которых собран персонаж (${lib.length}):`), h('div', { class: 'list', style: { maxHeight: 'none' } }, ...lib.map(l => {
      const kind = l.slice(1, 4);
      return h('div', { class: 'list-item', title: l }, icon(kind === 'SVG' ? 'svg' : 'image'), h('span', { class: 'badge' }, kind), h('span', { class: 'grow' }, l.slice(6)));
    }))];
  },

  tab_color() {
    return [
      group('Цвета инструментов рисования',
        field('Fill', colorField({ key: 'c-fill', value: S.ui.fill, alpha: true, onChange: (c, f) => f && rpc('set_colors', { fill: c }) })),
        field('Stroke', colorField({ key: 'c-stroke', value: S.ui.stroke, alpha: true, onChange: (c, f) => f && rpc('set_colors', { stroke: c }) })),
        row(button('Поменять местами', () => rpc('set_colors', { fill: S.ui.stroke, stroke: S.ui.fill }), { class: 'small' }))),
      hint('Fill — заливка новых фигур (Rect/Oval/Poly/Pen) и Paint Bucket (K). Stroke — обводка и Ink Bottle (S). Пипетка (I) берёт цвет со сцены в Fill.'),
    ];
  },

  tab_light() {
    const e = S.env;
    const set = (args, f = true) => rpc('env_set', args, { down: !f });
    return [
      group('Ambient',
        slider({ key: 'amb-i', label: 'Яркость', value: e.ambient_intensity, min: 0, max: 2, onChange: (v, f) => set({ ambient_intensity: v }, f) }),
        field('Цвет', colorField({ key: 'amb-c', value: [...e.ambient_color, 255], onChange: (c, f) => f && set({ ambient_color: c }) }))),
      group('Солнце',
        checkbox({ key: 'sun-on', label: 'Включить', checked: e.sun_enabled, onChange: v => set({ sun_enabled: v }) }),
        e.sun_enabled ? field('Цвет', colorField({ key: 'sun-c', value: [...e.sun_color, 255], onChange: (c, f) => f && set({ sun_color: c }) })) : null,
        e.sun_enabled ? slider({ key: 'sun-i', label: 'Яркость', value: e.sun_intensity, min: 0, max: 2, onChange: (v, f) => set({ sun_intensity: v }, f) }) : null),
      group('Точечный свет',
        checkbox({ key: 'pt-on', label: 'Включить', checked: e.point_enabled, onChange: v => set({ point_enabled: v }) }),
        ...(e.point_enabled ? [
          field('Цвет', colorField({ key: 'pt-c', value: [...e.point_color, 255], onChange: (c, f) => f && set({ point_color: c }) })),
          slider({ key: 'pt-i', label: 'Яркость', value: e.point_intensity, min: 0, max: 3, onChange: (v, f) => set({ point_intensity: v }, f) }),
          slider({ key: 'pt-r', label: 'Радиус', value: e.point_radius, min: 10, max: 300, step: 1, digits: 0, onChange: (v, f) => set({ point_radius: v }, f) }),
          field('Позиция', numField({ key: 'pt-x', label: 'X', value: e.point_x, step: 1, onChange: (v, f) => set({ point_x: v }, f) }), numField({ key: 'pt-y', label: 'Y', value: e.point_y, step: 1, onChange: (v, f) => set({ point_y: v }, f) })),
          hint('Пунктирный круг на сцене — радиус действия источника.'),
        ] : [])),
    ];
  },

  tab_particles() {
    const e = S.env;
    const set = (args, f = true) => rpc('env_set', args, { down: !f });
    return [
      checkbox({ key: 'pa-on', label: 'Включить эмиттер', checked: e.particles, onChange: v => set({ particles: v }) }),
      field('Вид', select({ key: 'pa-kind', value: e.kind, options: ['Dust', 'Snow', 'Rain', 'Spark', 'Magic', 'Smoke', 'Cloud'], onChange: v => set({ kind: v }) })),
      slider({ key: 'pa-rate', label: 'Частиц/с', value: e.rate, min: 0, max: 60, step: 0.5, digits: 1, onChange: (v, f) => set({ rate: v }, f) }),
      slider({ key: 'pa-life', label: 'Жизнь, с', value: e.lifetime, min: 0.1, max: 5, onChange: (v, f) => set({ lifetime: v }, f) }),
      slider({ key: 'pa-spread', label: 'Разброс', value: e.spread, min: 0, max: 150, step: 1, digits: 0, onChange: (v, f) => set({ spread: v }, f) }),
      field('Позиция', numField({ key: 'pa-x', label: 'X', value: e.ex, step: 1, onChange: (v, f) => set({ ex: v }, f) }), numField({ key: 'pa-y', label: 'Y', value: e.ey, step: 1, onChange: (v, f) => set({ ey: v }, f) })),
      h('div', { class: 'kv' }, h('span', null, 'Живых частиц'), h('span', null, e.alive)),
    ];
  },

  tab_script() {
    const ta = h('textarea', { key: 'script', rows: 9, spellcheck: 'false', value: F.script, oninput: e => { F.script = e.target.value; } });
    ta.addEventListener('keydown', e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); rpc('script_run', { text: F.script }); } });
    const log = S.ui.script_log;
    return [
      hint('pony.Move(dx, dy) · Look(x, y) · Blink() · Smile(k) · Walk() · camera.Move/Rotate/Zoom/Shake/Depth/Blur'),
      ta,
      row(button('Выполнить', () => rpc('script_run', { text: F.script }), { icon: 'play', primary: true, title: 'Ctrl+Enter' })),
      h('div', { class: 'log' + (log.startsWith('Ошибка') ? ' err' : log ? ' ok' : '') }, log || 'Результат появится здесь'),
      group('Примеры', h('div', { class: 'chips' }, ...SCRIPT_EXAMPLES.map(([l, src]) => button(l, () => { F.script = src; this.renderBody(); }, { class: 'small' })))),
    ];
  },

  tab_svg() {
    const svgs = S.doc.parts.filter(p => p.svg).sort((a, b) => a.id.localeCompare(b.id));
    if (!svgs.length) return [h('div', { class: 'empty-state' }, icon('svg'), 'SVG-частей пока нет', h('div', { class: 'hint' }, 'Нарисуй фигуру (Rect, Oval, Pen…) и нажми «Сохранить как слой» в палитре слева.'))];
    return [hint(`SVG-частей в персонаже: ${svgs.length}. Любую можно открыть на холсте и править инструментами.`), ...svgs.map(p => h('div', { class: 'card' },
      h('div', { class: 'card-title' }, p.id, S.ui.editing_part === p.id ? h('span', { class: 'badge green' }, 'редактируется') : null),
      h('div', { class: 'hint', style: { margin: '2px 0', wordBreak: 'break-all' } }, `${p.svg} · кость ${p.bone || '—'} · слой ${p.layer}`),
      row(button('Редактировать', () => rpc('edit_svg', { id: p.id }), { class: 'small', icon: 'svg' }), button('Выбрать', () => { rpc('select_layer', { id: p.id }); this.show('props'); }, { class: 'small' }))))];
  },

  tab_symbols() {
    const v = S.vec, sel = S.ui.selected_shape;
    const shape = sel != null ? v.shapes[sel] : null;
    const out = [
      hint('Символ — переиспользуемая фигура: правка определения меняет все инстансы.'),
      group('Convert to Symbol',
        row(h('input', { type: 'text', key: 'sym-name', placeholder: 'имя символа', value: F.symbolName, oninput: e => { F.symbolName = e.target.value; } }),
          button('Создать', () => rpc('symbol_convert', { name: F.symbolName }).then(r => { if (r === undefined) return; F.symbolName = ''; this.renderBody(); }), { disabled: !shape || shape.instance, primary: true })),
        !shape ? hint('Выбери фигуру на холсте инструментом SubSelection (A).') : null),
    ];
    if (!v.symbols.length) out.push(hint('В документе пока нет символов.'));
    else out.push(group(`Определения (${v.symbols.length})`, ...v.symbols.map(s => h('div', { class: 'card' },
      h('div', { class: 'card-title' }, icon('symbol'), s.name, h('span', { class: 'meta' }, `${s.shapes} фигур · ${s.instances} инстансов`)),
      row(button('Разместить инстанс', () => rpc('symbol_place', { name: s.name }), { class: 'small', icon: 'plus' }))))));
    if (shape && shape.instance) out.push(group('Выбранный инстанс', row(button('Break Apart (разорвать связь)', () => rpc('symbol_break'), { class: 'small' }))));
    return out;
  },

  tab_gradients() {
    const g = F.grad, v = S.vec, sel = S.ui.selected_shape;
    const shape = sel != null ? v.shapes[sel] : null;
    const css = st => `linear-gradient(90deg, ${st.slice().sort((a, b) => a.offset - b.offset).map(s => `${rgbaCss(s.color)} ${s.offset * 100}%`).join(', ')})`;
    const redraw = () => this.renderBody();
    const out = [
      group(g.editing ? `Редактирование «${g.editing}»` : 'Новый градиент',
        field('Имя', h('input', { type: 'text', key: 'g-name', value: g.name, oninput: e => { g.name = e.target.value; } })),
        field('Тип', h('span', { class: 'seg' }, button('Linear', () => { g.radial = false; redraw(); }, { class: 'small' + (g.radial ? '' : ' primary') }), button('Radial', () => { g.radial = true; redraw(); }, { class: 'small' + (g.radial ? ' primary' : '') }))),
        ...(g.radial ? [
          field('Центр', numField({ key: 'g-cx', label: 'X', value: g.cx, onChange: x => { g.cx = x; } }), numField({ key: 'g-cy', label: 'Y', value: g.cy, onChange: x => { g.cy = x; } })),
          field('Радиус', numField({ key: 'g-r', label: 'R', value: g.r, min: 0.01, onChange: x => { g.r = x; } })),
        ] : [
          field('Точка 1', numField({ key: 'g-x1', label: 'X', value: g.x1, onChange: x => { g.x1 = x; } }), numField({ key: 'g-y1', label: 'Y', value: g.y1, onChange: x => { g.y1 = x; } })),
          field('Точка 2', numField({ key: 'g-x2', label: 'X', value: g.x2, onChange: x => { g.x2 = x; } }), numField({ key: 'g-y2', label: 'Y', value: g.y2, onChange: x => { g.y2 = x; } })),
        ]),
        h('div', { class: 'grad-preview', style: { background: css(g.stops) } }),
        ...g.stops.map((s, i) => h('div', { class: 'stop-row' },
          numField({ key: 'g-off-' + i, label: 'offset', value: s.offset, step: 0.01, min: 0, max: 1, onChange: x => { s.offset = x; } }),
          colorField({ key: 'g-col-' + i, value: s.color, alpha: true, onChange: (c, f) => { s.color = c; if (f) redraw(); } }),
          button('', () => { g.stops.splice(i, 1); redraw(); }, { icon: 'x', class: 'small ghost', disabled: g.stops.length < 2 }))),
        row(button('Остановка', () => { const last = g.stops[g.stops.length - 1]; g.stops.push({ offset: clamp(((last ? last.offset : 0) + 1) / 2, 0, 1), color: [128, 128, 128, 255] }); redraw(); }, { class: 'small', icon: 'plus' })),
        row(button('Сохранить градиент', () => rpc('gradient_save', g).then(r => { if (r === undefined) return; g.editing = g.name.trim(); redraw(); }), { primary: true, icon: 'save', disabled: !g.name.trim() }),
          button('Новый', () => { F.grad = newGradForm(); redraw(); }, { class: 'small' }))),
    ];
    if (!v.gradients.length) out.push(hint('В документе пока нет градиентов.'));
    else out.push(group(`Градиенты (${v.gradients.length})`, ...v.gradients.map(d => h('div', { class: 'card' },
      h('div', { class: 'card-title' }, d.name, h('span', { class: 'meta' }, `${d.radial ? 'Radial' : 'Linear'} · ${d.stops.length} стопов`)),
      h('div', { class: 'grad-preview', style: { background: css(d.stops) } }),
      row(button('В форму', () => { F.grad = { ...newGradForm(), ...JSON.parse(JSON.stringify(d)), editing: d.name }; redraw(); }, { class: 'small' }),
        button('К фигуре', () => rpc('gradient_apply', { name: d.name }), { class: 'small', disabled: !(shape && shape.gradable), title: 'Применить к выбранной фигуре (SubSelection)' }),
        button('', () => rpc('gradient_delete', { name: d.name }), { class: 'small danger', icon: 'trash' }))))));
    return out;
  },

  tab_combine() {
    const v = S.vec;
    const cands = v.shapes.map((s, i) => [i, s]).filter(([, s]) => s.flat).map(([i, s]) => [String(i), `#${i} ${s.label}`]);
    if (cands.length < 2) return [hint('Нужно минимум 2 замкнутые фигуры на холсте (Rect, Ellipse, Polygon, замкнутый Path).')];
    const same = F.boolFront != null && F.boolFront === F.boolBack;
    const ops = [['union', 'Union'], ['difference', 'Difference'], ['intersection', 'Intersection'], ['xor', 'XOR'], ['divide', 'Divide']];
    const desc = { union: 'Объединить фигуры в одну.', difference: 'Вычесть нижнюю фигуру из верхней.', intersection: 'Оставить только пересечение.', xor: 'Оставить непересекающиеся части.', divide: 'Разрезать на все непересекающиеся куски.' };
    return [
      hint('Modify → Combine Paths: исходные фигуры заменяются результатом.'),
      field('Верхняя', select({ key: 'bo-f', value: F.boolFront, options: [[null, '—'], ...cands], onChange: x => { F.boolFront = x; this.renderBody(); } })),
      field('Нижняя', select({ key: 'bo-b', value: F.boolBack, options: [[null, '—'], ...cands], onChange: x => { F.boolBack = x; this.renderBody(); } })),
      field('Операция', select({ key: 'bo-op', value: F.boolOp, options: ops, onChange: x => { F.boolOp = x; this.renderBody(); } })),
      hint(desc[F.boolOp]),
      same ? h('div', { class: 'warn' }, icon('warn'), 'Фигуры не могут совпадать') : null,
      row(button('Выполнить', () => rpc('boolean_run', { front: +F.boolFront, back: +F.boolBack, op: F.boolOp }).then(r => { if (r === undefined) return; F.boolFront = F.boolBack = null; this.renderBody(); }), { primary: true, disabled: F.boolFront == null || F.boolBack == null || same })),
    ];
  },

  tab_groups() {
    const gs = S.doc.groups;
    if (!gs.length) return [h('div', { class: 'empty-state' }, icon('group'), 'Групп нет', h('div', { class: 'hint' }, 'Выдели несколько частей инструментом Lasso (L), затем Edit → Group.'))];
    const out = [];
    const walk = (parent, depth) => {
      for (const g of gs.filter(x => (x.parent || null) === parent)) {
        out.push(h('div', { class: 'list-item', style: { paddingLeft: 8 + depth * 16 + 'px' } }, icon('group'), h('span', { class: 'grow' }, g.name), h('span', { class: 'meta' }, `${g.members} ч.`),
          checkbox({ key: 'grp-h-' + g.id, label: 'скрыта', checked: g.hidden, onChange: v => rpc('group_hidden', { id: g.id, hidden: v }) }),
          button('Ungroup', () => rpc('ungroup', { id: g.id }), { class: 'small' })));
        if (depth < 32) walk(g.id, depth + 1);
      }
    };
    walk(null, 0);
    return [hint('Организационные группы поверх частей — не влияют на кости.'), h('div', { class: 'list', style: { maxHeight: 'none' } }, ...out)];
  },

  tab_info() {
    const ui = S.ui, d = S.doc;
    const kv = (...pairs) => h('div', { class: 'kv' }, ...pairs.flatMap(([k, v]) => [h('span', null, k), h('span', null, v)]));
    return [
      group('Сцена', kv(
        ['Курсор', Stage.mouseDoc ? `${Math.round(Stage.mouseDoc[0])}, ${Math.round(Stage.mouseDoc[1])}` : '—'],
        ['Зум', Math.round(Stage.zoom * 100) + '%'],
        ['Документ', `${App.scene[0]} × ${App.scene[1]}`],
        ['Инструмент', TOOL_INFO[ui.tool] ? TOOL_INFO[ui.tool].name : ui.tool])),
      group('Выделение', kv(
        ['Слой', ui.selected_layer || '—'],
        ['Мир. позиция', ui.sel_world ? `${fmt(ui.sel_world[0], 1)}, ${fmt(ui.sel_world[1], 1)}` : '—'],
        ['Кость', ui.selected_bone || '—'],
        ['Лассо', ui.multi.length ? `${ui.multi.length} частей` : '—'])),
      group('Документ', kv(
        ['Частей', d.counts.parts], ['Костей', d.counts.bones], ['Анимаций', d.counts.anims], ['Фигур на холсте', d.counts.shapes],
        ['Частиц', S.env.alive], ['Undo / Redo', `${ui.undo} / ${ui.redo}`], ['Рендер', 'WebView (SVG), экспорт — resvg (CPU)'])),
    ];
  },
};
