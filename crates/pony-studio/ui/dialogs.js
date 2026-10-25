// Окна: модальные диалоги, встроенный проводник, импорт Adobe Animate.
'use strict';

const Modal = {
  stack: [],
  open({ title, iconName, body, foot, cls, modal = true, onClose }) {
    const layer = document.getElementById('overlays');
    const back = modal ? h('div', { class: 'backdrop' }) : null;
    const close = () => this.close(win);
    const titleBar = h('div', { class: 'win-title' }, iconName ? icon(iconName) : null, h('span', null, title), button('', close, { icon: 'x', class: 'ghost small x', title: 'Закрыть (Esc)' }));
    const win = h('div', { class: 'window ' + (cls || '') + (modal ? '' : ' floating') }, titleBar, h('div', { class: 'win-body' }, body), foot ? h('div', { class: 'win-foot' }, foot) : null);
    makeDraggable(win, titleBar);
    if (back) {
      layer.appendChild(back);
      back.addEventListener('pointerdown', e => { if (e.target === back) close(); });
    }
    layer.appendChild(win);
    const rec = { win, back, onClose, modal };
    this.stack.push(rec);
    win._rec = rec;
    return win;
  },
  close(win) {
    const i = this.stack.findIndex(r => r.win === win);
    if (i < 0) return;
    const [rec] = this.stack.splice(i, 1);
    rec.win.remove();
    if (rec.back) rec.back.remove();
    if (rec.onClose) rec.onClose();
  },
  top() { return this.stack[this.stack.length - 1]; },
  /** Открыт модальный диалог — горячие клавиши редактора не работают. */
  blocking() { return this.stack.some(r => r.modal); },
};

function confirmDialog(text, okLabel = 'OK', extra) {
  return new Promise(resolve => {
    let done = false;
    const finish = v => { if (done) return; done = true; Modal.close(win); resolve(v); };
    const win = Modal.open({
      title: 'Подтверждение', iconName: 'warn', cls: 'confirm',
      body: h('div', { style: { maxWidth: '420px', padding: '4px 0' } }, text),
      foot: [extra ? button(extra, () => finish('extra')) : null, button('Отмена', () => finish(false)), button(okLabel, () => finish(true), { primary: true })],
      onClose: () => finish(false),
    });
    setTimeout(() => win.querySelector('button.primary').focus(), 0);
  });
}

// ---------------------------------------------------------------- проводник

const PURPOSES = {
  open: { title: 'Открыть', ok: 'Открыть', icon: 'open', xfl: true, filters: [['Все поддерживаемые (.asset, .fla, .xfl)', ['asset', 'fla', 'xfl']], ['Персонаж (.asset)', ['asset']], ['Adobe Animate (.fla, .xfl)', ['fla', 'xfl']], ['Все файлы', null]] },
  save: { title: 'Сохранить как', ok: 'Сохранить', icon: 'save', save: true, filters: [['Персонаж (.asset)', ['asset']]] },
  layer: { title: 'Импорт картинки как слоя', ok: 'Импортировать', icon: 'image', filters: [['Картинки (.png, .svg, .psd, .kra)', ['png', 'svg', 'psd', 'kra']], ['PNG', ['png']], ['SVG', ['svg']], ['Photoshop (.psd)', ['psd']], ['Krita (.kra)', ['kra']], ['Все файлы', null]] },
  animate: { title: 'Открыть файл Adobe Animate', ok: 'Открыть', icon: 'animate', xfl: true, filters: [['Adobe Animate (.fla, .xfl)', ['fla', 'xfl']], ['Все файлы', null]] },
  gif: { title: 'Экспорт GIF', ok: 'Экспортировать', icon: 'export', save: true, filters: [['GIF', ['gif']]] },
  sheet: { title: 'Экспорт спрайт-листа', ok: 'Экспортировать', icon: 'export', save: true, filters: [['PNG', ['png']]] },
};

const PLACE_ICONS = { project: ['pony', 'Проект'], home: ['home', 'Домашняя'], desktop: ['desktop', 'Рабочий стол'], documents: ['docs', 'Документы'], downloads: ['download', 'Загрузки'] };
const extOf = n => { const m = /\.([^.\\/]+)$/.exec(n); return m ? m[1].toLowerCase() : ''; };
const FILE_ICONS = { asset: 'pony', fla: 'animate', xfl: 'animate', png: 'image', svg: 'svg', psd: 'image', kra: 'image', gif: 'image' };

/**
 * Встроенный проводник. Возвращает Promise с выбранным путём (или null).
 * `start` — файл или папка, с которой начать; `name` — имя по умолчанию.
 */
function fileDialog(purpose, { start, name } = {}) {
  const P = PURPOSES[purpose];
  return new Promise(resolve => {
    const st = { dir: '', entries: [], filter: 0, search: '', hidden: false, sel: -1, name: name || '', back: [], error: null, confirm: null, data: null };
    let done = false;
    const finish = path => {
      if (done) return;
      done = true;
      if (path) rpc('fs_remember', { dir: dirOf(path) });
      Modal.close(win);
      resolve(path);
    };
    const dirOf = p => p.replace(/[\\/][^\\/]*$/, '') || p;

    const pathInput = h('input', { type: 'text', class: 'path', spellcheck: 'false' });
    const search = h('input', { type: 'text', placeholder: 'Поиск…', style: { width: '150px' }, oninput: e => { st.search = e.target.value; renderList(); } });
    const crumbs = h('div', { class: 'fd-crumbs' });
    const places = h('div', { class: 'fd-places' });
    const list = h('div', { class: 'fd-list', tabindex: 0 });
    const nameInput = h('input', { type: 'text', spellcheck: 'false', value: st.name, placeholder: P.save ? 'имя файла' : 'выбери файл или впиши путь' });
    const filterSel = select({ value: '0', options: P.filters.map((f, i) => [String(i), f[0]]), onChange: v => { st.filter = +v; renderList(); } });
    const msg = h('div', { class: 'fd-msg' });
    const okBtn = button(P.ok, () => confirm(), { primary: true });

    pathInput.addEventListener('keydown', e => { if (e.key === 'Enter') navigate(pathInput.value, true); });
    nameInput.addEventListener('input', () => { st.name = nameInput.value; st.confirm = null; renderMsg(); });
    nameInput.addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); confirm(); } });

    const visible = () => {
      const exts = P.filters[st.filter][1];
      const q = st.search.toLowerCase();
      return st.entries.filter(e => (st.hidden || !e.name.startsWith('.')) && (!q || e.name.toLowerCase().includes(q)) && (e.dir || !exts || exts.includes(extOf(e.name))));
    };

    async function navigate(dir, push) {
      const data = await rpc('fs_list', { dir });
      if (data.error && !data.entries.length && st.dir) { st.error = data.error; renderMsg(); return; }
      if (push && st.dir && data.dir !== st.dir) st.back.push(st.dir);
      st.data = data; st.dir = data.dir; st.entries = data.entries; st.sel = -1; st.error = data.error || null; st.confirm = null;
      if (!P.save) { /* имя файла в Open сбрасывается при переходе */ }
      render();
    }

    function select_(i) {
      const v = visible();
      st.sel = clamp(i, -1, v.length - 1);
      const e = v[st.sel];
      if (e && !e.dir) { st.name = e.name; nameInput.value = e.name; }
      else if (e && e.dir && !P.save) { st.name = ''; nameInput.value = ''; }
      st.confirm = null;
      renderList(); renderMsg();
      const el = list.querySelector('.fd-item.on');
      if (el) el.scrollIntoView({ block: 'nearest' });
    }

    async function activate(e) {
      if (e.dir) {
        if (P.xfl && e.xfl) return finish(e.path);
        if (!P.save) { st.name = ''; nameInput.value = ''; }
        return navigate(e.path, true);
      }
      st.name = e.name; nameInput.value = e.name;
      confirm();
    }

    async function confirm() {
      let nm = st.name.trim();
      if (!nm) { const e = visible()[st.sel]; if (e && e.dir) return activate(e); return; }
      let path = await rpc('fs_join', { dir: st.dir, name: nm });
      if (P.save) {
        const exts = P.filters[0][1];
        if (!exts.includes(extOf(path))) path += '.' + exts[0];
      }
      const info = await rpc('fs_stat', { path });
      if (info.dir) {
        if (P.xfl && info.xfl) return finish(path);
        st.name = ''; nameInput.value = '';
        return navigate(path, true);
      }
      if (P.save) {
        if (info.exists && st.confirm !== path) { st.confirm = path; renderMsg(); return; }
        if (!info.parent_ok) { st.error = 'Такой папки не существует'; renderMsg(); return; }
      } else if (!info.file) { st.error = 'Файл не найден: ' + path; renderMsg(); return; }
      finish(path);
    }

    async function newFolder() {
      const box = h('input', { type: 'text', value: 'Новая папка' });
      const ok = async () => {
        const p = await rpc('fs_mkdir', { dir: st.dir, name: box.value });
        if (p === undefined) { st.error = S.ui.status; renderMsg(); return; }
        Modal.close(w); await navigate(st.dir, false); select_(visible().findIndex(e => e.path === p));
      };
      box.addEventListener('keydown', e => { if (e.key === 'Enter') ok(); });
      const w = Modal.open({ title: 'Новая папка', iconName: 'newfolder', body: box, foot: [button('Отмена', () => Modal.close(w)), button('Создать', ok, { primary: true })] });
      setTimeout(() => { box.focus(); box.select(); }, 0);
    }

    function renderMsg() {
      fill(msg, 
        st.confirm ? h('span', { class: 'ask' }, `Файл уже существует. Нажми «${P.ok}» ещё раз, чтобы перезаписать.`) : null,
        st.error ? h('span', { class: 'err' }, st.error) : null);
      okBtn.querySelector('span').textContent = st.confirm ? 'Перезаписать' : P.ok;
    }

    function renderList() {
      const v = visible();
      fill(list, h('div', { class: 'fd-head' }, h('span', null, 'Имя'), h('span', { class: 'r', style: { textAlign: 'right' } }, 'Размер'), h('span', null, 'Изменён')),
        ...(v.length ? v.map((e, i) => {
          const ext = extOf(e.name);
          const el = h('div', { class: 'fd-item' + (e.dir ? ' dir' : '') + (e.xfl ? ' xfl' : '') + (i === st.sel ? ' on' : ''), title: e.path },
            h('span', { class: 'nm' }, icon(e.xfl ? 'animate' : e.dir ? 'folder' : FILE_ICONS[ext] || 'file'), h('span', null, e.name)),
            h('span', { class: 'meta r' }, e.size), h('span', { class: 'meta' }, e.mtime));
          el.addEventListener('click', () => select_(i));
          el.addEventListener('dblclick', () => activate(e));
          return el;
        }) : [h('div', { class: 'fd-empty' }, st.search ? 'Ничего не найдено' : 'Папка пуста')]));
    }

    function render() {
      pathInput.value = st.dir;
      const d = st.data;
      fill(crumbs, ...d.crumbs.flatMap((c, i) => {
        const el = h('span', { class: 'crumb' }, c.name);
        el.addEventListener('click', () => navigate(c.path, true));
        return i ? [h('span', null, '›'), el] : [el];
      }));
      const place = (label, path, ic) => {
        const el = h('div', { class: 'fd-place' + (path === st.dir ? ' on' : ''), title: path }, icon(ic), h('span', null, label));
        el.addEventListener('click', () => navigate(path, true));
        return el;
      };
      fill(places, h('h5', null, 'Места'), ...d.places.map(p => {
        if (p.label.startsWith('drive:')) return place(p.label.slice(6), p.path, 'drive');
        const [ic, label] = PLACE_ICONS[p.label] || ['folder', p.label];
        return place(label, p.path, ic);
      }), d.recent.length ? h('h5', null, 'Недавние') : null, ...d.recent.map(p => place(p.split(/[\\/]/).filter(Boolean).pop() || p, p, 'clock')));
      renderList(); renderMsg();
    }

    list.addEventListener('keydown', e => {
      const v = visible();
      if (e.key === 'ArrowDown') { e.preventDefault(); select_(st.sel + 1); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); select_(st.sel - 1); }
      else if (e.key === 'Enter') { e.preventDefault(); const it = v[st.sel]; if (it) activate(it); else confirm(); }
      else if (e.key === 'Backspace') { e.preventDefault(); if (st.data.parent) navigate(st.data.parent, true); }
      else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
        // Начали печатать в списке — это имя/путь файла.
        nameInput.value = ''; st.name = ''; nameInput.focus();
      }
    });

    const body = h('div', { style: { display: 'contents' } },
      h('div', { class: 'fd-bar' },
        button('', () => { const b = st.back.pop(); if (b) navigate(b, false); }, { icon: 'back', class: 'ghost', title: 'Назад' }),
        button('', () => st.data.parent && navigate(st.data.parent, true), { icon: 'up', class: 'ghost', title: 'Наверх (Backspace)' }),
        button('', () => navigate(st.dir, false), { icon: 'refresh', class: 'ghost', title: 'Обновить' }),
        pathInput,
        h('span', { style: { display: 'inline-flex', alignItems: 'center', gap: '4px' } }, icon('search'), search),
        P.save ? button('', newFolder, { icon: 'newfolder', class: 'ghost', title: 'Новая папка' }) : null),
      crumbs,
      h('div', { class: 'fd-main' }, places, list),
      h('div', { class: 'fd-name' }, h('span', { class: 'field-label' }, P.save ? 'Имя файла' : 'Файл'), nameInput, filterSel),
      msg);

    const win = Modal.open({
      title: P.title, iconName: P.icon, cls: 'fd', body,
      foot: [h('div', { class: 'left' }, checkbox({ label: 'Скрытые файлы', checked: false, onChange: v => { st.hidden = v; renderList(); } })), button('Отмена', () => finish(null)), okBtn],
      onClose: () => { if (!done) { done = true; resolve(null); } },
    });
    win.addEventListener('keydown', e => { if (e.key === 'Escape') { e.stopPropagation(); finish(null); } });
    rpc('fs_list', { start: start || '' }).then(data => {
      st.data = data; st.dir = data.dir; st.entries = data.entries; st.error = data.error || null;
      render();
      setTimeout(() => (P.save ? nameInput : list).focus(), 0);
      if (P.save) { const dot = nameInput.value.lastIndexOf('.'); nameInput.setSelectionRange(0, dot > 0 ? dot : nameInput.value.length); }
    });
  });
}

// ---------------------------------------------------------------- Adobe Animate

const XflWindow = {
  win: null,
  filter: '',
  lastShow: 0,
  previewUrl: null,

  open() {
    if (this.win) return;
    this.body = h('div');
    this.win = Modal.open({ title: 'Импорт Adobe Animate', iconName: 'animate', cls: 'xfl-win', body: this.body, modal: false, onClose: () => { this.win = null; } });
    this.win.style.left = '240px'; this.win.style.top = '90px'; this.win.style.transform = 'none';
    this.render(true);
  },
  close() { if (this.win) Modal.close(this.win); },

  onState(changed) {
    if (!S.xfl) return;
    if (S.xfl.show !== this.lastShow) {
      const first = this.lastShow === 0 && S.xfl.show === 0;
      this.lastShow = S.xfl.show;
      if (!first) this.open();
    }
    if (changed.xfl_preview) this.updatePreview();
    if (this.win && changed.xfl) this.render(false);
  },

  updatePreview() {
    if (this.previewUrl) URL.revokeObjectURL(this.previewUrl);
    this.previewUrl = S.xfl_preview ? URL.createObjectURL(new Blob([S.xfl_preview], { type: 'image/svg+xml' })) : null;
    if (this.preview) this.fillPreview();
  },
  fillPreview() {
    fill(this.preview, this.previewUrl ? h('img', { src: this.previewUrl, alt: '' }) : h('span', { class: 'none' }, '(пустой кадр)'));
  },

  render() {
    if (!this.win) return;
    const x = S.xfl;
    const pathInput = h('input', { type: 'text', key: 'xfl-path', value: x.path, placeholder: 'путь к .fla / .xfl', spellcheck: 'false' });
    pathInput.addEventListener('keydown', e => { if (e.key === 'Enter') rpc('xfl_load', { path: pathInput.value }); });
    const top = h('div', { class: 'xfl-top' }, pathInput,
      button('Обзор…', () => fileDialog('animate', { start: x.path }).then(p => p && rpc('xfl_load', { path: p })), { icon: 'open' }),
      button('Загрузить', () => rpc('xfl_load', { path: pathInput.value })));
    const parts = [top, h('div', { class: 'xfl-info' }, x.info || '')];
    if (!x.loaded) {
      parts.push(h('div', { class: 'empty-state' }, icon('animate'), 'Выбери файл кнопкой «Обзор…» или впиши путь и нажми «Загрузить».'));
      replaceKeepingFocus(this.body, () => parts);
      return;
    }
    const symList = h('div', { class: 'list' });
    const fillSyms = () => {
      const q = this.filter.toLowerCase();
      const item = (label, meta, on, cls, click) => { const el = h('div', { class: 'list-item ' + cls + (on ? ' on' : ''), title: label }, h('span', { class: 'grow' }, label), h('span', { class: 'meta' }, meta)); el.addEventListener('click', click); return el; };
      fill(symList, 
        item('(главная шкала)', x.main_frames + ' кадр.', x.symbol == null, '', () => rpc('xfl_select', { symbol: null })),
        ...x.symbols.filter(([n]) => !q || n.toLowerCase().includes(q)).map(([n, f]) => item(n, f, x.symbol === n, n.startsWith('~') ? 'container' : '', () => rpc('xfl_select', { symbol: n }))));
    };
    fillSyms();
    const search = h('input', { type: 'text', key: 'xfl-search', placeholder: 'поиск символа…', value: this.filter, oninput: e => { this.filter = e.target.value; fillSyms(); } });
    const range = h('input', { type: 'range', min: 0, max: Math.max(0, x.count - 1), value: x.frame, step: 1 });
    range.addEventListener('input', () => rpc('xfl_frame', { frame: +range.value }, { coalesce: 'xfl_frame' }));
    this.preview = h('div', { class: 'xfl-preview' });
    this.fillPreview();
    const right = h('div', null,
      h('div', { class: 'xfl-frame' }, h('span', { class: 'lbl' }, 'Кадр'), range,
        button('', () => rpc('xfl_frame', { frame: x.frame - 1 }), { icon: 'prev', class: 'small ghost' }),
        button('', () => rpc('xfl_frame', { frame: x.frame + 1 }), { icon: 'next', class: 'small ghost' }),
        h('span', { class: 'fnum' }, `${x.frame} / ${x.count - 1}`)),
      x.labels.length ? h('div', { class: 'chips' }, h('span', { class: 'hint', style: { margin: '0 4px 0 0' } }, 'Метки:'), ...x.labels.map(([i, l]) => button(`${l} (${i})`, () => rpc('xfl_frame', { frame: i }), { class: 'small' + (i === x.frame ? ' primary' : '') }))) : null,
      this.preview,
      h('div', { class: 'row', style: { marginTop: '8px' } },
        numField({ key: 'xfl-scale', label: 'Масштаб', value: x.scale, step: 0.01, min: 0.01, max: 10, title: 'Мировых единиц на пиксель Animate', onChange: (v, f) => f && rpc('xfl_opts', { scale: v }) }),
        checkbox({ key: 'xfl-hidden', label: 'Скрытые слои', checked: x.hidden, title: 'Слои, скрытые глазком в Animate. Animate по умолчанию публикует их (так выглядит готовый мультфильм) — у Луны там тёмно-синяя грива и звёзды.', onChange: v => rpc('xfl_opts', { hidden: v }) }),
        checkbox({ key: 'xfl-expand', label: 'Раскрыть «~»-контейнеры', checked: x.expand, title: 'Вложенные символы с именем на «~» (например ~RD_Head) раскладываются на отдельные части: глаза, рот, брови…', onChange: v => rpc('xfl_opts', { expand: v }) })),
      hint('Каждый слой станет частью со своей костью в точке трансформации Animate — её можно двигать, вращать и анимировать.'),
      row(button('Импортировать как нового персонажа', () => rpc('xfl_import').then(r => r && r.close && this.close()), { primary: true, icon: 'import' })));
    parts.push(h('div', { class: 'xfl-cols' }, h('div', { class: 'xfl-syms' }, search, symList), right));
    replaceKeepingFocus(this.body, () => parts);
    const on = symList.querySelector('.on');
    if (on) on.scrollIntoView({ block: 'nearest' });
  },
};

// ---------------------------------------------------------------- справка

const HOTKEYS = [
  ['Ctrl+N / Ctrl+O / Ctrl+S', 'новый / открыть / сохранить'],
  ['Ctrl+Shift+S', 'сохранить как'],
  ['Ctrl+Z / Ctrl+Shift+Z, Ctrl+Y', 'отменить / вернуть'],
  ['Space', 'play / pause'],
  ['← → , .', 'кадр назад / вперёд'],
  ['Home / End', 'в начало / в конец анимации'],
  ['F6', 'ключевой кадр для выбранного слоя'],
  ['Delete', 'удалить выбранный ключ / фигуру (SubSelection)'],
  ['Колесо, Ctrl+= / Ctrl+−', 'зум к курсору'],
  ['Средняя кнопка, Hand (H)', 'панорама'],
  ['Ctrl+1 / Ctrl+2 / Ctrl+3', '100% / весь документ / персонаж'],
  ['V A Q L', 'Selection / SubSelection / Free Transform / Lasso'],
  ['P Y B N R O', 'Pen / Pencil / Brush / Line / Rectangle / Oval'],
  ['K S I H Z', 'Paint Bucket / Ink Bottle / Eyedropper / Hand / Zoom'],
  ['Free Transform', 'тащить — двигать кость, Shift — вращать, Alt+клик — кость-родитель'],
  ['Pen', 'клик — точка, клик по первой — замкнуть, Enter — закончить, Esc — отмена'],
  ['Двойной клик по части', 'открыть SVG-часть на редактирование'],
  ['Esc', 'отменить незаконченное / снять выделение'],
];

function helpDialog() {
  const w = Modal.open({
    title: 'Горячие клавиши', iconName: 'info',
    body: h('div', { class: 'keys', style: { maxWidth: '640px' } }, ...HOTKEYS.flatMap(([k, d]) => [h('span', null, ...k.split(' / ').flatMap((x, i) => i ? [' / ', h('kbd', null, x)] : [h('kbd', null, x)])), h('span', null, d)])),
    foot: [button('Закрыть', () => Modal.close(w), { primary: true })],
  });
}

function aboutDialog() {
  const w = Modal.open({
    title: 'О программе', iconName: 'pony',
    body: h('div', { style: { maxWidth: '460px', lineHeight: '1.6' } },
      h('p', null, h('b', null, 'Pony Studio'), ' — редактор персонажей pony-engine: части, кости, IK, анимация по ключам, импорт из Adobe Animate.'),
      h('p', { class: 'hint' }, 'Интерфейс — системная веб-панель (WebView2 на Windows, WebKitGTK на Linux). Модель и экспорт — Rust (pony-core, resvg).'),
      h('p', { class: 'hint' }, 'Подробности — README.md в репозитории.')),
    foot: [button('Закрыть', () => Modal.close(w), { primary: true })],
  });
}
