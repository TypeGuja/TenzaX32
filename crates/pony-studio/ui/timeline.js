// Timeline как в Animate: слева слои (видимость/блокировка), справа сетка
// КАДРОВ на canvas: ключи-ромбы, плейхед, затемнение после конца анимации.
// клик — перейти на кадр · двойной клик / F6 — ключ · тащить ключ — сдвинуть
// · тащить по пустому — скраб · Delete — удалить выбранный ключ.
'use strict';

const ROW_H = 22, RULER_H = 20;

const Timeline = {
  zoom: 1,

  init() {
    this.root = document.getElementById('timeline');
    this.head = h('div', { class: 'tl-head' });
    this.body = h('div', { class: 'tl-body' });
    this.labels = h('div', { class: 'tl-labels' });
    this.labelsInner = h('div', { class: 'tl-labels-inner' });
    this.labels.appendChild(this.labelsInner);
    this.grid = h('div', { class: 'tl-grid' });
    this.spacer = h('div', { class: 'tl-spacer' });
    this.canvas = h('canvas');
    this.grid.append(this.canvas, this.spacer);
    this.all = h('div', { class: 'tl-all' });
    this.root.append(this.head, this.body);
    this.grid.addEventListener('scroll', () => { this.labelsInner.style.top = -this.grid.scrollTop + 'px'; this.draw(); });
    this.labels.addEventListener('wheel', e => { this.grid.scrollTop += e.deltaY; e.preventDefault(); }, { passive: false });
    new ResizeObserver(() => this.draw()).observe(this.grid);
    this.canvas.addEventListener('pointerdown', e => this.onDown(e));
    this.canvas.addEventListener('pointermove', e => this.onMove(e));
    this.canvas.addEventListener('pointerup', e => this.onUp(e));
    this.canvas.addEventListener('dblclick', e => this.onDbl(e));
    this.canvas.addEventListener('wheel', e => {
      if (e.ctrlKey) { e.preventDefault(); this.zoom = clamp(this.zoom * Math.exp(-e.deltaY * 0.004), 0.25, 6); this.renderHead(); this.layout(); }
      else if (e.shiftKey) { e.preventDefault(); this.grid.scrollLeft += e.deltaY; }
    }, { passive: false });
  },

  render(changed) {
    if (!S.ui) return;
    if (changed.ui || changed.anim || changed.doc || changed.play) this.renderHead();
    const all = S.ui.timeline_all;
    if (all) {
      if (this.body.firstChild !== this.all) fill(this.body, this.all);
      if (changed.ui) this.renderAll();
      return;
    }
    if (this.body.firstChild !== this.labels) fill(this.body, this.labels, this.grid);
    if (changed.anim || changed.ui) this.renderLabels();
    this.layout();
  },

  renderHead() {
    const ui = S.ui, an = S.anim, play = S.play;
    const anims = S.doc.anims;
    const playing = play.playing;
    fill(this.head, 
      h('span', { class: 'seg' },
        button('Персонаж', () => rpc('timeline_mode', { all: false }), { class: ui.timeline_all ? '' : 'on', title: 'Таймлайн активного персонажа' }),
        button('Все', () => rpc('timeline_mode', { all: true }), { class: ui.timeline_all ? 'on' : '', title: 'Обзор всех персонажей сцены' })),
      h('span', { class: 'play' },
        tbtn('first', 'В начало (Home)', () => rpc('seek', { op: 'start' })),
        tbtn('prev', 'Кадр назад (←)', () => rpc('seek', { op: 'prev' })),
        tbtn(playing ? 'pause' : 'play', 'Play / Pause (Space)', () => rpc('toggle_play'), 'big'),
        tbtn('next', 'Кадр вперёд (→)', () => rpc('seek', { op: 'next' })),
        tbtn('last', 'В конец (End)', () => rpc('seek', { op: 'end' }))),
      h('span', { class: 'frame-counter', title: 'Текущий кадр / всего кадров (24 fps)' }, 'Кадр ', h('b', null, play.frame), ' / ', play.total),
      h('span', { class: 'tb-sep' }),
      h('span', { class: 'lbl' }, 'Анимация'),
      select({ key: 'tl-anim', value: an.name, options: anims.length ? anims : [[null, '(нет)']], onChange: v => v && rpc('play_anim', { name: v }) }),
      button('', () => rpc('anim_new'), { icon: 'plus', class: 'ghost', title: 'Новая пустая анимация' }),
      an.duration != null ? numField({ key: 'tl-dur', label: 'Длит.', value: an.duration, step: 0.01, min: 1 / 24, max: 600, suffix: 'с', onChange: (v, fin) => rpc('anim_set', { duration: v }, { down: !fin }) }) : null,
      an.duration != null ? checkbox({ key: 'tl-loop', label: 'Loop', checked: an.looping, onChange: v => rpc('anim_set', { looping: v }) }) : null,
      h('span', { class: 'tb-sep' }),
      h('span', { class: 'lbl' }, 'Канал'),
      select({ key: 'tl-ch', value: ui.channel, options: [['x', 'Position X'], ['y', 'Position Y'], ['rot', 'Rotation'], ['sx', 'Scale X'], ['sy', 'Scale Y']], onChange: v => rpc('channel', { ch: v }), title: 'Какой канал кости показывать и ключевать' }),
      h('span', { class: 'lbl' }, 'Масштаб'),
      h('input', { type: 'range', class: 'tl-zoom', min: 0.25, max: 6, step: 0.05, value: this.zoom, oninput: e => { this.zoom = +e.target.value; this.layout(); } }),
      h('span', { class: 'tb-spacer' }),
      button('Персонаж', () => rpc('char_new'), { icon: 'plus', class: 'ghost', title: 'Добавить в сцену нового пустого персонажа' }),
      button('', () => confirmDialog('Удалить активного персонажа из сцены?', 'Удалить').then(ok => ok && rpc('char_delete')), { icon: 'trash', class: 'ghost', title: 'Удалить активного персонажа', disabled: ui.characters.length < 2 }),
    );
  },

  rows() { return (S.anim && S.anim.rows) || []; },

  renderLabels() {
    const ui = S.ui;
    const hidden = new Set(ui.hidden), locked = new Set(ui.locked);
    const head = h('div', { class: 'tl-lhead' }, icon('eye'), icon('lock'), h('span', { style: { marginLeft: '6px' } }, `Слои — ${ui.active_name}`));
    const rows = this.rows().map(r => {
      const isPart = r.kind === 'part';
      const sel = isPart ? ui.selected_layer === r.label : r.kind === 'bone' && ui.selected_bone === r.label.replace('кость: ', '');
      const el = h('div', { class: `tl-row ${r.kind}${sel ? ' sel' : ''}${isPart && hidden.has(r.label) ? ' hidden-layer' : ''}`, title: r.label },
        isPart ? h('span', { class: 'tgl' + (hidden.has(r.label) ? ' off' : ''), title: 'Показать / скрыть', onclick: e => { e.stopPropagation(); rpc('toggle_hidden', { id: r.label }); } }, icon(hidden.has(r.label) ? 'eyeoff' : 'eye')) : h('span', { class: 'tgl' }, r.kind === 'bone' ? icon('bone') : null),
        isPart ? h('span', { class: 'tgl' + (locked.has(r.label) ? ' locked' : ' off'), title: 'Заблокировать / разблокировать', onclick: e => { e.stopPropagation(); rpc('toggle_lock', { id: r.label }); } }, icon(locked.has(r.label) ? 'lock' : 'unlock')) : h('span', { class: 'tgl' }),
        h('span', { class: 'name' }, r.label));
      el.addEventListener('click', () => {
        if (isPart) rpc('select_layer', { id: r.label });
        else if (r.kind === 'bone') rpc('select_bone', { id: r.label.replace('кость: ', '') });
      });
      return el;
    });
    if (!rows.length) rows.push(h('div', { class: 'tl-row misc' }, h('span', { class: 'name' }, 'Нет слоёв — импортируй картинку или нарисуй фигуру')));
    fill(this.labelsInner, head, ...rows);
  },

  frameW() { return Math.max(8 * this.zoom, 1.5); },
  totalFrames() { return (S.anim && S.anim.total_frames) || 24; },
  displayFrames() { return Math.max(this.totalFrames() + 24, Math.ceil(this.grid.clientWidth / this.frameW())); },

  layout() {
    if (!S.anim || S.ui.timeline_all) return;
    const fw = this.frameW();
    this.spacer.style.width = this.displayFrames() * fw + 'px';
    this.spacer.style.height = RULER_H + Math.max(1, this.rows().length) * ROW_H + 'px';
    this.draw();
  },

  draw() {
    if (!S.anim || !S.ui || S.ui.timeline_all) return;
    const dpr = devicePixelRatio || 1;
    const w = this.grid.clientWidth, hgt = this.grid.clientHeight;
    if (!w || !hgt) return;
    const cv = this.canvas;
    cv.style.width = w + 'px'; cv.style.height = hgt + 'px';
    cv.style.marginBottom = -hgt + 'px';
    if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(hgt * dpr)) { cv.width = Math.round(w * dpr); cv.height = Math.round(hgt * dpr); }
    const ctx = cv.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const sx = this.grid.scrollLeft, sy = this.grid.scrollTop;
    const fw = this.frameW(), total = this.totalFrames(), disp = this.displayFrames();
    const rows = this.rows();
    const font = getComputedStyle(document.body).fontFamily;
    ctx.fillStyle = '#1c1d22'; ctx.fillRect(0, 0, w, hgt);
    const f0 = Math.max(0, Math.floor(sx / fw)), f1 = Math.min(disp, Math.ceil((sx + w) / fw));
    // строки
    for (let i = 0; i < rows.length; i++) {
      const y = RULER_H + i * ROW_H - sy;
      if (y + ROW_H < RULER_H || y > hgt) continue;
      ctx.fillStyle = i % 2 ? '#1f2025' : '#212228';
      ctx.fillRect(0, y, w, ROW_H);
      const r = rows[i];
      if ((r.kind === 'part' && S.ui.selected_layer === r.label)) { ctx.fillStyle = 'rgba(77,141,255,.14)'; ctx.fillRect(0, y, w, ROW_H); }
      ctx.fillStyle = '#26272d'; ctx.fillRect(0, y + ROW_H - 1, w, 1);
    }
    // сетка кадров и каждые 5 — акцент, как в Animate
    if (fw >= 4) {
      for (let f = f0; f <= f1; f++) {
        const x = Math.round(f * fw - sx) + 0.5;
        ctx.fillStyle = f % 5 === 0 ? '#2c2d34' : '#25262c';
        ctx.fillRect(x, RULER_H, 1, hgt);
      }
    }
    // после конца анимации — затемнение
    const endX = total * fw - sx;
    ctx.fillStyle = 'rgba(0,0,0,.32)'; ctx.fillRect(Math.max(endX, 0), RULER_H, w, hgt);
    ctx.fillStyle = '#7878a8'; ctx.fillRect(Math.round(endX), 0, 1.5, hgt);
    // ключи
    const selKey = S.ui.selected_keyframe;
    for (let i = 0; i < rows.length; i++) {
      const y = RULER_H + i * ROW_H - sy + ROW_H / 2;
      if (y < RULER_H || y > hgt + ROW_H) continue;
      for (const f of rows[i].frames) {
        if (f < f0 - 1 || f > f1 + 1) continue;
        const x = f * fw + fw / 2 - sx;
        const sel = selKey && selKey[0] === rows[i].label && selKey[1] === f;
        const r = sel ? 6 : 4.6;
        ctx.beginPath(); ctx.moveTo(x, y - r); ctx.lineTo(x + r, y); ctx.lineTo(x, y + r); ctx.lineTo(x - r, y); ctx.closePath();
        ctx.fillStyle = sel ? '#ffe28c' : (rows[i].kind === 'misc' ? '#9a9aa8' : '#e9b949');
        ctx.fill();
        ctx.lineWidth = sel ? 2 : 1; ctx.strokeStyle = sel ? '#fff' : '#6d5317'; ctx.stroke();
      }
    }
    // линейка
    ctx.fillStyle = '#2a2b31'; ctx.fillRect(0, 0, w, RULER_H);
    ctx.fillStyle = '#3a3b44'; ctx.fillRect(0, RULER_H - 1, w, 1);
    const every = fw >= 14 ? 1 : fw >= 6 ? 5 : fw >= 3 ? 10 : 24;
    ctx.font = '10px ' + font; ctx.textBaseline = 'middle';
    for (let f = Math.floor(f0 / every) * every; f <= f1; f += every) {
      const x = Math.round(f * fw - sx) + 0.5;
      const major = f % (every === 1 ? 5 : every * 2) === 0;
      ctx.fillStyle = major ? '#80838f' : '#4c4e58';
      ctx.fillRect(x, major ? 6 : 12, 1, RULER_H - (major ? 6 : 12));
      if (major || fw * every >= 26) { ctx.fillStyle = '#a9acb6'; ctx.fillText(String(f), x + 3, 8); }
    }
    // плейхед
    const cur = Math.min(Math.round(S.play.time * 24), total);
    const px = cur * fw + fw / 2 - sx;
    ctx.fillStyle = 'rgba(255,85,85,.18)'; ctx.fillRect(cur * fw - sx, RULER_H, fw, hgt);
    ctx.fillStyle = '#ff5a5a'; ctx.fillRect(Math.round(px) - 1, 0, 2, hgt);
    ctx.beginPath(); ctx.moveTo(px - 6, 0); ctx.lineTo(px + 6, 0); ctx.lineTo(px + 6, 9); ctx.lineTo(px, 15); ctx.lineTo(px - 6, 9); ctx.closePath(); ctx.fill();
  },

  // ------------------------------------------------------------ ввод

  hit(e) {
    const r = this.canvas.getBoundingClientRect();
    const x = e.clientX - r.left + this.grid.scrollLeft, yv = e.clientY - r.top;
    const fw = this.frameW();
    const frame = clamp(Math.floor(x / fw), 0, this.displayFrames());
    const ri = yv >= RULER_H ? Math.floor((yv - RULER_H + this.grid.scrollTop) / ROW_H) : -1;
    const row = ri >= 0 ? this.rows()[ri] : null;
    let key = null;
    if (row) {
      const rad = clamp(fw / 2, 7, 14);
      const k = row.frames.find(f => Math.abs(x - (f * fw + fw / 2)) < rad);
      if (k != null) key = [row.label, k];
    }
    return { frame, row, key, ruler: yv < RULER_H };
  },

  onDown(e) {
    if (e.button !== 0) return;
    this.canvas.setPointerCapture(e.pointerId);
    this.press = { x: e.clientX, y: e.clientY, hit: this.hit(e), dragging: false };
  },
  onMove(e) {
    const p = this.press;
    if (!p) return;
    if (!p.dragging && Math.hypot(e.clientX - p.x, e.clientY - p.y) > 3) {
      p.dragging = true;
      const k = p.hit.key;
      rpc('tl_drag_start', { label: k ? k[0] : null, frame: k ? k[1] : null });
    }
    if (p.dragging) rpc('tl_drag', { frame: this.hit(e).frame }, { coalesce: 'tl_drag' });
  },
  onUp(e) {
    const p = this.press;
    this.press = null;
    if (!p) return;
    if (p.dragging) { rpc('tl_drag', { frame: this.hit(e).frame }); return; }
    const hit = p.hit;
    if (hit.key) rpc('tl_click', { label: hit.key[0], frame: hit.key[1], key: true });
    else rpc('tl_click', { label: hit.row ? hit.row.label : null, frame: hit.frame, key: false });
  },
  onDbl(e) {
    const hit = this.hit(e);
    if (hit.row && hit.row.kind !== 'misc') rpc('tl_dblclick', { label: hit.row.label, frame: hit.frame });
  },

  // ------------------------------------------------------------ режим «Все»

  renderAll() {
    const chars = S.ui.characters;
    fill(this.all, 
      hint('Клик по строке — сделать персонажа активным и открыть его таймлайн.'),
      ...chars.map(c => {
        const cv = h('canvas');
        const el = h('div', { class: 'strip' + (c.active ? ' active' : '') },
          h('div', { class: 'strip-title' }, icon(c.playing ? 'play' : 'pause'), c.name, c.active ? h('span', { class: 'badge green' }, 'активен') : null,
            h('span', { class: 'meta' }, `${c.parts} частей · ${c.bones} костей · ${fmt(c.strip.duration)} с`)),
          cv);
        if (!c.active) el.addEventListener('click', () => rpc('char_switch', { index: c.index }));
        requestAnimationFrame(() => this.drawStrip(cv, c.strip));
        return el;
      }));
  },

  drawStrip(cv, strip) {
    const dpr = devicePixelRatio || 1, w = cv.clientWidth, hgt = cv.clientHeight;
    if (!w) return;
    cv.width = w * dpr; cv.height = hgt * dpr;
    const ctx = cv.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const y = hgt / 2, x0 = 4, x1 = w - 4, dur = Math.max(strip.duration, 0.01);
    ctx.fillStyle = '#5d606b'; ctx.fillRect(x0, y - 0.5, x1 - x0, 1);
    for (let i = 0; i <= 4; i++) ctx.fillRect(x0 + (x1 - x0) * i / 4, y - 3, 1, 6);
    for (const t of strip.keys) {
      const x = x0 + (x1 - x0) * clamp(t / dur, 0, 1);
      ctx.beginPath(); ctx.moveTo(x, y - 4); ctx.lineTo(x + 4, y); ctx.lineTo(x, y + 4); ctx.lineTo(x - 4, y); ctx.closePath();
      ctx.fillStyle = '#e9b949'; ctx.fill();
    }
    const px = x0 + (x1 - x0) * clamp(strip.time / dur, 0, 1);
    ctx.fillStyle = '#ff5a5a'; ctx.fillRect(px - 1, 0, 2, hgt);
  },
};

function tbtn(iconName, title, onClick, extra) {
  const b = h('button', { class: 'tbtn' + (extra ? ' ' + extra : ''), title, type: 'button' }, icon(iconName));
  b.addEventListener('click', e => { e.stopPropagation(); onClick(e); });
  return b;
}
