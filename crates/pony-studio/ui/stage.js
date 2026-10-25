// Stage: сцена в SVG. Части персонажа — <image> с матрицей из Rust (та же
// математика, что у экспорта и GPU-рендера), поэтому всё чёткое на любом
// зуме: браузер растеризует векторные части прямо в разрешении экрана.
'use strict';

const MIN_ZOOM = 0.05, MAX_ZOOM = 64;

// Аффинные матрицы [a b c d e f] как в SVG matrix().
const M = {
  mul(m, n) {
    return [m[0] * n[0] + m[2] * n[1], m[1] * n[0] + m[3] * n[1], m[0] * n[2] + m[2] * n[3], m[1] * n[2] + m[3] * n[3], m[0] * n[4] + m[2] * n[5] + m[4], m[1] * n[4] + m[3] * n[5] + m[5]];
  },
  inv(m) {
    const det = m[0] * m[3] - m[1] * m[2] || 1e-12;
    return [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det, (m[2] * m[5] - m[3] * m[4]) / det, (m[1] * m[4] - m[0] * m[5]) / det];
  },
  apply(m, x, y) { return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]; },
  T: (x, y) => [1, 0, 0, 1, x, y],
  S: (x, y) => [x, 0, 0, y ?? x, 0, 0],
  R: a => [Math.cos(a), Math.sin(a), -Math.sin(a), Math.cos(a), 0, 0],
  str: m => `matrix(${m.map(v => +v.toFixed(6)).join(' ')})`,
};
const FLIP = [1, 0, 0, -1, 0, 0];

const DRAW_TOOLS = new Set(['pen', 'pencil', 'brush', 'line', 'rect', 'oval', 'poly']);
const SHAPE_TOOLS = new Set(['line', 'rect', 'oval', 'poly']);
const RUST_DRAG_TOOLS = new Set(['sel', 'xform', 'subsel']);
const CLICK_TOOLS = new Set(['sel', 'xform', 'drop', 'bucket', 'ink', 'subsel']);

const Stage = {
  zoom: 0.5,
  pan: { x: 0, y: 0 },
  size: { w: 100, h: 100 },
  mouse: null,          // экранная точка курсора
  mouseDoc: null,       // точка в пикселях документа (для Info/линеек)
  g: null,              // текущий жест
  pen: [],              // точки незаконченного пути Pen (мир)
  parts: new Map(),
  uids: new Map(),
  nextUid: 1,
  lastFit: 0,
  needsFit: null,

  init() {
    this.view = document.getElementById('stage-view');
    this.svg = svgEl('svg', { xmlns: SVGNS });
    this.defs = svgEl('defs');
    this.bg = svgEl('rect', { x: 0, y: 0, width: '100%', height: '100%', fill: 'rgb(135,207,235)' });
    this.world = svgEl('g');
    this.partsG = svgEl('g');
    this.particlesG = svgEl('g');
    this.vecG = svgEl('g', { transform: 'scale(1,-1)' });
    this.world.append(this.partsG, this.particlesG, this.vecG);
    this.dim = svgEl('path', { fill: 'rgba(30,30,34,0.78)', 'fill-rule': 'evenodd' });
    this.border = svgEl('rect', { fill: 'none', stroke: '#0e0e10', 'stroke-width': 1.5 });
    this.overlay = svgEl('g');
    this.svg.append(this.defs, this.bg, this.world, this.dim, this.border, this.overlay);
    this.view.appendChild(this.svg);
    this.hud = document.getElementById('stage-hud');

    new ResizeObserver(() => this.resize()).observe(this.view);
    this.resize();

    const v = this.view;
    v.addEventListener('pointerdown', e => this.onDown(e));
    v.addEventListener('pointermove', e => this.onMove(e));
    v.addEventListener('pointerup', e => this.onUp(e));
    v.addEventListener('pointercancel', e => this.onUp(e, true));
    v.addEventListener('pointerleave', () => { this.mouse = null; this.mouseDoc = null; this.drawOverlay(); this.drawRulers(); this.drawHud(); });
    v.addEventListener('wheel', e => this.onWheel(e), { passive: false });
    v.addEventListener('contextmenu', e => e.preventDefault());
    v.addEventListener('dblclick', e => {
      if (S.ui && S.ui.tool === 'sel' && S.ui.selected_layer) {
        const p = S.doc.parts.find(x => x.id === S.ui.selected_layer);
        if (p && p.svg) rpc('edit_svg', { id: p.id });
      }
    });
  },

  resize() {
    const r = this.view.getBoundingClientRect();
    this.size = { w: Math.max(1, r.width), h: Math.max(1, r.height) };
    if (this.needsFit) this.fit(this.needsFit);
    this.renderView();
  },

  // ------------------------------------------------------------ вид

  camera() {
    return (S.frame && S.frame.camera) || { x: 0, y: 0, rot: 0, zoom: 1 };
  },
  center() { return [this.size.w / 2 + this.pan.x, this.size.h / 2 + this.pan.y]; },
  /** Мир (Y вверх) -> экран Stage. */
  viewMatrix() {
    const c = this.camera();
    const [cx, cy] = this.center();
    return [M.T(cx, cy), M.S(this.zoom, -this.zoom), M.S(c.zoom), M.R(-c.rot), M.T(-c.x, -c.y)].reduce(M.mul);
  },
  toScreen(x, y) { return M.apply(this.viewMatrix(), x, y); },
  toWorld(sx, sy) { return M.apply(M.inv(this.viewMatrix()), sx, sy); },
  /** Мировых единиц на экранный пиксель. */
  ppw() { return 1 / (this.zoom * this.camera().zoom); },
  docRect() {
    const [cx, cy] = this.center();
    const w = App.scene[0] * this.zoom, h = App.scene[1] * this.zoom;
    return { x: cx - w / 2, y: cy - h / 2, w, h };
  },

  setZoom(z, anchor) {
    const nz = clamp(z, MIN_ZOOM, MAX_ZOOM);
    const a = anchor || [this.size.w / 2, this.size.h / 2];
    const k = nz / this.zoom;
    const fx = a[0] - this.size.w / 2, fy = a[1] - this.size.h / 2;
    this.pan.x = fx - (fx - this.pan.x) * k;
    this.pan.y = fy - (fy - this.pan.y) * k;
    this.zoom = nz;
    this.renderView();
    Chrome.renderZoom();
  },

  fit(kind) {
    if (this.size.w < 20 || !S.frame) { this.needsFit = kind; return; }
    this.needsFit = null;
    const c = this.camera();
    const b = S.frame.bounds;
    if (kind === 'document' || !b) {
      this.zoom = Math.min(this.size.w / App.scene[0], this.size.h / App.scene[1]) * 0.95;
      this.pan = { x: 0, y: 0 };
    } else {
      const sw = Math.max(1, b[2] - b[0]) * c.zoom, sh = Math.max(1, b[3] - b[1]) * c.zoom;
      this.zoom = clamp(Math.min(this.size.w / sw, this.size.h / sh) * 0.6, MIN_ZOOM, MAX_ZOOM);
      this.pan = { x: 0, y: 0 };
      const [sx, sy] = this.toScreen((b[0] + b[2]) / 2, (b[1] + b[3]) / 2);
      this.pan = { x: this.size.w / 2 - sx, y: this.size.h / 2 - sy };
    }
    this.renderView();
    Chrome.renderZoom();
  },

  // ------------------------------------------------------------ отрисовка

  render(changed) {
    if (changed.ui && S.ui.fit.n !== this.lastFit) {
      this.lastFit = S.ui.fit.n;
      this.fit(S.ui.fit.kind);
    }
    if (changed.frame) this.renderParts();
    if (changed.vec) this.vecG.innerHTML = S.vec.markup;
    if (changed.vec || changed.frame) this.vecG.setAttribute('transform', M.str(S.vec.map));
    if (changed.ui) this.view.className = this.cursorClass();
    this.renderView();
  },

  cursorClass() {
    const t = S.ui.tool;
    if (S.ui.placing_bone) return 't-bone';
    if (t === 'hand') return 't-hand';
    if (t === 'zoom') return 't-zoom';
    if (t === 'drop') return 't-drop';
    if (DRAW_TOOLS.has(t) || t === 'lasso') return 't-draw';
    return '';
  },

  uid(id) {
    if (!this.uids.has(id)) this.uids.set(id, this.nextUid++);
    return this.uids.get(id);
  },

  renderParts() {
    const items = S.frame.items;
    const seen = new Set();
    let prev = null;
    for (const it of items) {
      seen.add(it.id);
      let rec = this.parts.get(it.id);
      if (!rec) {
        rec = { g: svgEl('g'), el: null, href: undefined, mask: null, filter: null, uid: this.uid(it.id) };
        this.parts.set(it.id, rec);
      }
      this.setQuad(rec, 'el', rec.g, it.href, it.fill, it.m);
      // свет: множитель цвета (линейное пространство, как в шейдере)
      if (it.light) {
        if (!rec.filter) {
          rec.filter = svgEl('filter', { id: 'lt' + rec.uid, x: '-5%', y: '-5%', width: '110%', height: '110%' }, svgEl('feColorMatrix', { type: 'matrix' }));
          this.defs.appendChild(rec.filter);
        }
        const [r, g, b] = it.light;
        rec.filter.firstChild.setAttribute('values', `${r} 0 0 0 0 0 ${g} 0 0 0 0 0 ${b} 0 0 0 0 0 1 0`);
        rec.g.setAttribute('filter', `url(#lt${rec.uid})`);
      } else if (rec.filter) {
        rec.filter.remove(); rec.filter = null; rec.g.removeAttribute('filter');
      }
      // маска: альфа другой части
      if (it.mask) {
        if (!rec.mask) {
          rec.mask = { node: svgEl('mask', { id: 'mk' + rec.uid, 'mask-type': 'alpha', style: 'mask-type:alpha', maskUnits: 'userSpaceOnUse', x: -1e5, y: -1e5, width: 2e5, height: 2e5 }), el: null, href: undefined };
          this.defs.appendChild(rec.mask.node);
        }
        this.setQuad(rec.mask, 'el', rec.mask.node, it.mask.href, it.mask.fill, it.mask.m);
        rec.g.setAttribute('mask', `url(#mk${rec.uid})`);
      } else if (rec.mask) {
        rec.mask.node.remove(); rec.mask = null; rec.g.removeAttribute('mask');
      }
      // порядок отрисовки
      const want = prev ? prev.nextSibling : this.partsG.firstChild;
      if (want !== rec.g) this.partsG.insertBefore(rec.g, want);
      prev = rec.g;
    }
    for (const [id, rec] of this.parts) {
      if (!seen.has(id)) {
        rec.g.remove();
        if (rec.filter) rec.filter.remove();
        if (rec.mask) rec.mask.node.remove();
        this.parts.delete(id);
      }
    }
    this.particlesG.innerHTML = S.frame.particles.map(p => `<rect x="${p[0] - p[2] / 2}" y="${p[1] - p[2] / 2}" width="${p[2]}" height="${p[2]}" fill="${p[3]}"/>`).join('');
  },

  /** Квад [-0.5..0.5] с картинкой части (или заглушкой цветом). */
  setQuad(rec, key, parent, href, fill, m) {
    const kind = href ? 'image' : 'rect';
    let el = rec[key];
    if (!el || el.tagName !== kind) {
      if (el) el.remove();
      el = svgEl(kind, { x: -0.5, y: -0.5, width: 1, height: 1 });
      if (kind === 'image') el.setAttribute('preserveAspectRatio', 'none');
      parent.appendChild(el);
      rec[key] = el;
      rec.href = undefined;
    }
    if (kind === 'image') {
      if (rec.href !== href) { el.setAttribute('href', href); rec.href = href; }
    } else {
      el.setAttribute('fill', fill);
    }
    el.setAttribute('transform', M.str(M.mul(m, FLIP)));
  },

  renderView() {
    if (!S.frame) return;
    this.world.setAttribute('transform', M.str(this.viewMatrix()));
    const d = this.docRect(), { w, h } = this.size;
    this.dim.setAttribute('d', `M0 0H${w}V${h}H0Z M${d.x} ${d.y}h${d.w}v${d.h}h${-d.w}Z`);
    Object.entries({ x: d.x, y: d.y, width: d.w, height: d.h }).forEach(([k, v]) => this.border.setAttribute(k, v));
    this.drawOverlay();
    this.drawRulers();
    this.drawHud();
  },

  drawOverlay() {
    if (!S.frame || !S.ui) return;
    const out = [];
    const P = (x, y) => this.toScreen(x, y);
    const boxPath = (b, color, width) => {
      const pts = [[-1, -1], [1, -1], [1, 1], [-1, 1]].map(([sx, sy]) => P(b.c[0] + sx * b.h[0], b.c[1] + sy * b.h[1]));
      return `<path d="M${pts.map(p => p.map(v => v.toFixed(1)).join(' ')).join('L')}Z" fill="none" stroke="${color}" stroke-width="${width}"/>`;
    };
    for (const b of S.frame.multi) out.push(boxPath(b, '#78e68c', 1.5));
    if (S.frame.sel) {
      const b = S.frame.sel;
      out.push(boxPath(b, '#5aaaff', 1.5));
      const [bx, by] = P(b.bone[0], b.bone[1]);
      const [cx, cy] = P(b.c[0], b.c[1]);
      out.push(`<line x1="${bx}" y1="${by}" x2="${cx}" y2="${cy}" stroke="#ffbe5a" stroke-width="1"/>`, `<circle cx="${bx}" cy="${by}" r="4" fill="none" stroke="#ffbe5a" stroke-width="1.5"/>`);
    }
    if (S.ui.show_skeleton) {
      for (const b of S.frame.bones) {
        const sel = b.id === S.ui.selected_bone;
        const col = sel ? '#ffd23c' : '#78beff';
        const [x, y] = P(b.x, b.y);
        if (b.parent) {
          const [px, py] = P(b.parent[0], b.parent[1]);
          out.push(`<line x1="${px}" y1="${py}" x2="${x}" y2="${y}" stroke="${col}" stroke-opacity=".7" stroke-width="1.5"/>`);
        }
        out.push(`<circle cx="${x}" cy="${y}" r="${sel ? 5 : 3.5}" fill="${col}" stroke="#0b1a2a" stroke-width="1"/>`);
        if (sel) out.push(`<circle cx="${x}" cy="${y}" r="8" fill="none" stroke="${col}" stroke-width="1.5"/>`);
      }
    }
    if (S.ui.tool === 'subsel' && S.vec && S.vec.cps.length) {
      for (const [x, y] of S.vec.cps) {
        const [sx, sy] = P(x, y);
        out.push(`<rect x="${sx - 3.5}" y="${sy - 3.5}" width="7" height="7" rx="1" fill="#fff" stroke="#3c8cdc"/>`);
      }
    }
    const stroke = S.ui ? rgbaCss(S.ui.stroke) : '#fff';
    if (this.pen.length) {
      const pts = this.pen.map(p => P(p[0], p[1]));
      if (this.mouse) pts.push([this.mouse.x, this.mouse.y]);
      out.push(`<polyline points="${pts.map(p => p.join(',')).join(' ')}" fill="none" stroke="${stroke}" stroke-width="1.5"/>`);
      this.pen.forEach((p, i) => {
        const [x, y] = P(p[0], p[1]);
        out.push(`<circle cx="${x}" cy="${y}" r="${i === 0 ? 4.5 : 3}" fill="${i === 0 ? '#fff' : '#78c8ff'}" stroke="#2a6ebd"/>`);
      });
    }
    const g = this.g;
    if (g && g.dragging && this.mouse) {
      const [x0, y0] = [g.sx, g.sy], [x1, y1] = [this.mouse.x, this.mouse.y];
      const st = `fill="none" stroke="${stroke}" stroke-width="1.5" stroke-dasharray="5 3"`;
      if (g.tool === 'rect') out.push(`<rect x="${Math.min(x0, x1)}" y="${Math.min(y0, y1)}" width="${Math.abs(x1 - x0)}" height="${Math.abs(y1 - y0)}" ${st}/>`);
      if (g.tool === 'oval') out.push(`<ellipse cx="${(x0 + x1) / 2}" cy="${(y0 + y1) / 2}" rx="${Math.abs(x1 - x0) / 2}" ry="${Math.abs(y1 - y0) / 2}" ${st}/>`);
      if (g.tool === 'line') out.push(`<line x1="${x0}" y1="${y0}" x2="${x1}" y2="${y1}" ${st}/>`);
      if (g.tool === 'poly') {
        const r = Math.hypot(x1 - x0, y1 - y0), n = S.ui.poly_sides;
        const pts = [...Array(n)].map((_, i) => { const a = i / n * Math.PI * 2 - Math.PI / 2; return [x0 + Math.cos(a) * r, y0 + Math.sin(a) * r].join(','); });
        out.push(`<polygon points="${pts.join(' ')}" ${st}/>`);
      }
      if (g.tool === 'lasso') out.push(`<rect x="${Math.min(x0, x1)}" y="${Math.min(y0, y1)}" width="${Math.abs(x1 - x0)}" height="${Math.abs(y1 - y0)}" fill="rgba(120,230,140,.08)" stroke="#78e68c" stroke-dasharray="4 3"/>`);
      if ((g.tool === 'pencil' || g.tool === 'brush') && g.points.length > 1) {
        const pts = g.points.map(p => P(p[0], p[1]).join(',')).join(' ');
        out.push(`<polyline points="${pts}" fill="none" stroke="${stroke}" stroke-width="${g.tool === 'brush' ? 4 * this.zoom * this.camera().zoom : 1.5}" stroke-linecap="round" stroke-linejoin="round"/>`);
      }
    }
    if (S.ui.placing_bone && this.mouse) out.push(`<circle cx="${this.mouse.x}" cy="${this.mouse.y}" r="6" fill="none" stroke="#ffd23c" stroke-width="1.5"/>`);
    if (S.env && S.env.point_enabled && Panels.tab === 'light') {
      const [x, y] = P(S.env.point_x, S.env.point_y);
      out.push(`<circle cx="${x}" cy="${y}" r="${S.env.point_radius * this.zoom * this.camera().zoom}" fill="none" stroke="${rgbaCss(S.env.point_color)}" stroke-dasharray="3 4" stroke-opacity=".8"/>`, `<circle cx="${x}" cy="${y}" r="4" fill="${rgbaCss(S.env.point_color)}"/>`);
    }
    this.overlay.innerHTML = out.join('');
  },

  drawRulers() {
    const top = document.getElementById('ruler-top'), left = document.getElementById('ruler-left');
    if (!top.offsetWidth || !App.rulers) return;
    const dpr = devicePixelRatio || 1, d = this.docRect();
    const step = this.zoom >= 2 ? 10 : this.zoom >= 0.5 ? 50 : this.zoom >= 0.2 ? 100 : 250;
    const label = step * (this.zoom * step < 40 ? 4 : 2);
    for (const [cv, horiz] of [[top, true], [left, false]]) {
      const w = cv.offsetWidth, hgt = cv.offsetHeight;
      if (cv.width !== Math.round(w * dpr)) cv.width = Math.round(w * dpr);
      if (cv.height !== Math.round(hgt * dpr)) cv.height = Math.round(hgt * dpr);
      const ctx = cv.getContext('2d');
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, hgt);
      ctx.fillStyle = '#26272c'; ctx.fillRect(0, 0, w, hgt);
      ctx.strokeStyle = '#5d606b'; ctx.fillStyle = '#8c909b'; ctx.font = '9.5px ' + getComputedStyle(document.body).fontFamily; ctx.lineWidth = 1;
      const origin = horiz ? d.x : d.y, len = horiz ? w : hgt;
      const first = Math.floor(-origin / (step * this.zoom)) * step;
      ctx.beginPath();
      for (let v = first; origin + v * this.zoom < len; v += step) {
        const p = Math.round(origin + v * this.zoom) + 0.5;
        const big = v % label === 0;
        const t = big ? 18 : 6;
        if (horiz) { ctx.moveTo(p, 18 - t); ctx.lineTo(p, 18); } else { ctx.moveTo(18 - t, p); ctx.lineTo(18, p); }
        if (big) {
          if (horiz) ctx.fillText(String(v), p + 3, 9);
          else { ctx.save(); ctx.translate(9, p + 3); ctx.rotate(-Math.PI / 2); ctx.fillText(String(v), -ctx.measureText(String(v)).width - 2, 0); ctx.restore(); }
        }
      }
      ctx.stroke();
      if (this.mouse) {
        ctx.strokeStyle = '#4d8dff'; ctx.beginPath();
        const m = horiz ? this.mouse.x : this.mouse.y;
        if (horiz) { ctx.moveTo(m + 0.5, 0); ctx.lineTo(m + 0.5, 18); } else { ctx.moveTo(0, m + 0.5); ctx.lineTo(18, m + 0.5); }
        ctx.stroke();
      }
    }
  },

  drawHud() {
    const z = Math.round(this.zoom * 100) + '%';
    this.hud.textContent = this.mouseDoc ? `${z}   x ${Math.round(this.mouseDoc[0])}  y ${Math.round(this.mouseDoc[1])}` : z;
  },

  // ------------------------------------------------------------ ввод

  local(e) {
    const r = this.view.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  },

  onWheel(e) {
    e.preventDefault();
    const p = this.local(e);
    const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
    this.setZoom(this.zoom * Math.exp(-dy * (e.ctrlKey ? 0.01 : 0.0015)), [p.x, p.y]);
  },

  onDown(e) {
    if (!S.ui) return;
    document.activeElement && document.activeElement.blur && document.activeElement.blur();
    const p = this.local(e);
    const tool = S.ui.tool;
    this.view.setPointerCapture(e.pointerId);
    if (e.button === 1 || (e.button === 0 && tool === 'hand' && !S.ui.placing_bone)) {
      this.g = { pan: true, lx: p.x, ly: p.y };
      this.view.classList.add('panning');
      e.preventDefault();
      return;
    }
    if (e.button === 2) {
      if (tool === 'zoom') this.setZoom(this.zoom / 1.25, [p.x, p.y]);
      return;
    }
    if (e.button !== 0) return;
    const [wx, wy] = this.toWorld(p.x, p.y);
    this.g = { tool, sx: p.x, sy: p.y, wx, wy, lx: wx, ly: wy, dragging: false, alt: e.altKey, shift: e.shiftKey, points: [[wx, wy]] };
  },

  onMove(e) {
    const p = this.local(e);
    this.mouse = p;
    const d = this.docRect();
    this.mouseDoc = [(p.x - d.x) / this.zoom, (p.y - d.y) / this.zoom];
    const g = this.g;
    if (g && g.pan) {
      this.pan.x += p.x - g.lx; this.pan.y += p.y - g.ly;
      g.lx = p.x; g.ly = p.y;
      this.renderView();
      return;
    }
    if (g) {
      const [wx, wy] = this.toWorld(p.x, p.y);
      if (!g.dragging && Math.hypot(p.x - g.sx, p.y - g.sy) > 3) {
        g.dragging = true;
        if (RUST_DRAG_TOOLS.has(g.tool) && !S.ui.placing_bone) rpc('stage_drag_start', { x: g.wx, y: g.wy, alt: g.alt, shift: g.shift, ppw: this.ppw() });
      }
      if (g.dragging) {
        if (RUST_DRAG_TOOLS.has(g.tool) && !S.ui.placing_bone) {
          rpc('stage_drag', { x: wx, y: wy, px: g.lx, py: g.ly, shift: e.shiftKey }, { coalesce: 'stage_drag', merge: (old, nw) => ({ ...nw, px: old.px, py: old.py }) });
        }
        if (g.tool === 'pencil' || g.tool === 'brush') g.points.push([wx, wy]);
        g.lx = wx; g.ly = wy;
      }
    }
    this.drawOverlay();
    this.drawRulers();
    this.drawHud();
    Panels.onMouse();
  },

  onUp(e, cancel) {
    const g = this.g;
    this.g = null;
    this.view.classList.remove('panning');
    if (!g || g.pan || cancel) { this.drawOverlay(); return; }
    const p = this.local(e);
    const [wx, wy] = this.toWorld(p.x, p.y);
    if (!g.dragging) {
      this.click(g, wx, wy, e);
    } else if (S.ui.placing_bone) {
      // перетаскивание в режиме установки кости — ничего
    } else if (RUST_DRAG_TOOLS.has(g.tool)) {
      rpc('stage_drag_end', { x: wx, y: wy, sx: g.wx, sy: g.wy });
    } else if (SHAPE_TOOLS.has(g.tool) || g.tool === 'lasso') {
      rpc('stage_drag_end', { x: wx, y: wy, sx: g.wx, sy: g.wy });
    } else if (g.tool === 'pencil' || g.tool === 'brush') {
      g.points.push([wx, wy]);
      if (g.points.length >= 2) rpc('add_polyline', { points: g.points, brush: g.tool === 'brush' });
    }
    this.drawOverlay();
  },

  click(g, wx, wy, e) {
    if (S.ui.placing_bone) { rpc('stage_click', { x: wx, y: wy, ppw: this.ppw() }); return; }
    if (g.tool === 'zoom') { this.setZoom(this.zoom * (e.altKey ? 1 / 1.25 : 1.25), [g.sx, g.sy]); return; }
    if (g.tool === 'pen') { this.penClick(wx, wy); return; }
    if (CLICK_TOOLS.has(g.tool)) rpc('stage_click', { x: wx, y: wy, alt: g.alt, shift: g.shift, ppw: this.ppw() });
  },

  penClick(wx, wy) {
    if (this.pen.length >= 3) {
      const [fx, fy] = this.toScreen(this.pen[0][0], this.pen[0][1]);
      const [cx, cy] = this.toScreen(wx, wy);
      if (Math.hypot(fx - cx, fy - cy) < 10) { this.penCommit(true); return; }
    }
    this.pen.push([wx, wy]);
    this.drawOverlay();
    Chrome.status(this.pen.length === 1 ? 'Pen: кликай точки; клик по первой — замкнуть, Enter — закончить, Esc — отмена' : null);
  },
  penCommit(closed) {
    if (this.pen.length >= 2) rpc('pen_commit', { points: this.pen, closed });
    this.pen = [];
    this.drawOverlay();
  },
  penCancel() {
    if (!this.pen.length) return false;
    this.pen = [];
    this.drawOverlay();
    Chrome.status('Путь Pen отменён');
    return true;
  },
};
