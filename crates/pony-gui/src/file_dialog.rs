//! Встроенный файловый проводник (Open / Save As / импорт / экспорт).
//!
//! Нативный диалог ОС (rfd и т.п.) тянет GTK/xdg-portal на Linux и
//! отдельный путь на Windows/macOS — вместо этого окно egui с тем же
//! набором возможностей, одинаковое на всех системах:
//! - навигация: двойной клик по папке, «Наверх», «Назад», хлебные крошки,
//!   редактируемая строка пути (Enter — перейти);
//! - быстрые места: домашняя папка, Рабочий стол, Документы, папка
//!   проекта, корень/диски, недавние папки (запоминаются между запусками);
//! - фильтр по типу файла и поиск по имени, показ скрытых файлов;
//! - размер и дата изменения, сортировка: папки сверху, по имени;
//! - Save: поле имени, автодобавление расширения, подтверждение
//!   перезаписи, «Новая папка»;
//! - клавиши: Enter — подтвердить, Esc — отмена, Backspace (вне поля
//!   ввода) — наверх, стрелки — выбор.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Зачем открыт проводник — от этого зависят фильтр, кнопка и что делать
/// с выбранным путём (см. обработку `DialogResult` в main).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    /// Открыть документ: `.asset` или Adobe Animate (`.fla`/`.xfl`).
    OpenDocument,
    /// Сохранить персонажа как `.asset`.
    SaveDocument,
    /// Импорт картинки как слоя (PNG/SVG/PSD/KRA).
    ImportLayer,
    /// Выбрать `.fla`/`.xfl` или папку XFL для окна импорта Animate.
    ImportAnimate,
    ExportGif,
    ExportSpritesheet,
}

impl Purpose {
    fn title(self) -> &'static str {
        match self {
            Purpose::OpenDocument => "Открыть",
            Purpose::SaveDocument => "Сохранить как",
            Purpose::ImportLayer => "Импорт картинки как слоя",
            Purpose::ImportAnimate => "Открыть файл Adobe Animate",
            Purpose::ExportGif => "Экспорт GIF",
            Purpose::ExportSpritesheet => "Экспорт спрайт-листа",
        }
    }

    fn confirm_label(self) -> &'static str {
        match self {
            Purpose::OpenDocument | Purpose::ImportAnimate => "Открыть",
            Purpose::SaveDocument => "Сохранить",
            Purpose::ImportLayer => "Импортировать",
            Purpose::ExportGif | Purpose::ExportSpritesheet => "Экспортировать",
        }
    }

    /// Диалог сохранения: имя вводится, файл может ещё не существовать.
    fn is_save(self) -> bool {
        matches!(self, Purpose::SaveDocument | Purpose::ExportGif | Purpose::ExportSpritesheet)
    }

    /// Варианты фильтра: (подпись, расширения). Первый — по умолчанию.
    fn filters(self) -> Vec<(&'static str, &'static [&'static str])> {
        match self {
            Purpose::OpenDocument => vec![
                ("Все поддерживаемые (.asset, .fla, .xfl)", &["asset", "fla", "xfl"]),
                ("Персонаж (.asset)", &["asset"]),
                ("Adobe Animate (.fla, .xfl)", &["fla", "xfl"]),
            ],
            Purpose::SaveDocument => vec![("Персонаж (.asset)", &["asset"])],
            Purpose::ImportLayer => vec![
                ("Картинки (.png, .svg, .psd, .kra)", &["png", "svg", "psd", "kra"]),
                ("PNG", &["png"]),
                ("SVG", &["svg"]),
                ("Photoshop (.psd)", &["psd"]),
                ("Krita (.kra)", &["kra"]),
            ],
            Purpose::ImportAnimate => vec![("Adobe Animate (.fla, .xfl)", &["fla", "xfl"])],
            Purpose::ExportGif => vec![("GIF", &["gif"])],
            Purpose::ExportSpritesheet => vec![("PNG", &["png"])],
        }
    }
}

/// Что выбрал пользователь.
pub struct DialogResult {
    pub purpose: Purpose,
    pub path: PathBuf,
}

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

pub struct FileDialog {
    pub open: bool,
    purpose: Purpose,
    dir: PathBuf,
    history: Vec<PathBuf>,
    entries: Vec<Entry>,
    selected: Option<usize>,
    file_name: String,
    path_edit: String,
    filter_index: usize,
    search: String,
    show_hidden: bool,
    error: Option<String>,
    confirm_overwrite: Option<PathBuf>,
    new_folder: Option<String>,
    recent: Vec<PathBuf>,
    scroll_to_selected: bool,
}

const MAX_RECENT: usize = 10;
/// Размер окна проводника (содержимое). Фиксированный: при автоподгонке
/// под содержимое egui на первом кадре растягивал окно на весь экран, и
/// кнопки внизу уезжали за его край.
const DIALOG_W: f32 = 780.0;
const DIALOG_H: f32 = 470.0;
const BODY_H: f32 = 320.0;
const PLACES_W: f32 = 150.0;

impl Default for FileDialog {
    fn default() -> Self {
        let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            open: false,
            purpose: Purpose::OpenDocument,
            path_edit: dir.display().to_string(),
            dir,
            history: Vec::new(),
            entries: Vec::new(),
            selected: None,
            file_name: String::new(),
            filter_index: 0,
            search: String::new(),
            show_hidden: false,
            error: None,
            confirm_overwrite: None,
            new_folder: None,
            recent: load_recent(),
            scroll_to_selected: false,
        }
    }
}

impl FileDialog {
    /// Открыть проводник. `start` — файл или папка, с которой начать
    /// (например, текущий документ); иначе последняя папка.
    pub fn show_for(&mut self, purpose: Purpose, start: Option<&Path>, default_name: &str) {
        self.purpose = purpose;
        self.open = true;
        self.filter_index = 0;
        self.search.clear();
        self.error = None;
        self.confirm_overwrite = None;
        self.new_folder = None;
        self.file_name = default_name.to_string();
        let start_dir = start
            .and_then(|p| if p.is_dir() { Some(p.to_path_buf()) } else { p.parent().filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf) })
            .filter(|d| d.is_dir())
            .or_else(|| self.recent.first().cloned().filter(|d| d.is_dir()))
            .unwrap_or_else(|| self.dir.clone());
        if let Some(name) = start.filter(|p| p.is_file()).and_then(|p| p.file_name()).and_then(|n| n.to_str()) {
            if purpose.is_save() {
                self.file_name = name.to_string();
            }
        }
        self.navigate(start_dir, false);
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn navigate(&mut self, dir: PathBuf, push_history: bool) {
        let dir = std::fs::canonicalize(&dir).map(strip_verbatim).unwrap_or(dir);
        match std::fs::read_dir(&dir) {
            Ok(rd) => {
                if push_history && dir != self.dir {
                    self.history.push(self.dir.clone());
                }
                let mut entries: Vec<Entry> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        let path = e.path();
                        // metadata() идёт по симлинкам: ссылка на папку — папка.
                        let meta = std::fs::metadata(&path).ok();
                        Entry {
                            name: e.file_name().to_string_lossy().into_owned(),
                            is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                            modified: meta.and_then(|m| m.modified().ok()),
                            path,
                        }
                    })
                    .collect();
                entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
                self.entries = entries;
                self.dir = dir;
                self.path_edit = self.dir.display().to_string();
                self.selected = None;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Не удалось открыть {}: {e}", dir.display())),
        }
    }

    fn refresh(&mut self) {
        let dir = self.dir.clone();
        let keep = self.selected.and_then(|i| self.entries.get(i)).map(|e| e.name.clone());
        self.navigate(dir, false);
        if let Some(name) = keep {
            self.selected = self.entries.iter().position(|e| e.name == name);
        }
    }

    fn extensions(&self) -> Option<&'static [&'static str]> {
        // Последний пункт фильтра — «Все файлы».
        self.purpose.filters().get(self.filter_index).map(|(_, e)| *e)
    }

    /// Показывать ли запись (папки — всегда, файлы — по фильтру/поиску).
    fn visible(&self, e: &Entry) -> bool {
        if !self.show_hidden && e.name.starts_with('.') {
            return false;
        }
        if !self.search.is_empty() && !e.name.to_lowercase().contains(&self.search.to_lowercase()) {
            return false;
        }
        if e.is_dir {
            return true;
        }
        match self.extensions() {
            Some(exts) => has_ext(&e.path, exts),
            None => true,
        }
    }

    fn remember(&mut self, dir: &Path) {
        self.recent.retain(|d| d != dir);
        self.recent.insert(0, dir.to_path_buf());
        self.recent.truncate(MAX_RECENT);
        save_recent(&self.recent);
    }

    /// Итоговый путь из поля имени (с расширением по умолчанию для Save).
    fn target_path(&self) -> Option<PathBuf> {
        let name = self.file_name.trim();
        if name.is_empty() {
            return None;
        }
        let mut path = if Path::new(name).is_absolute() { PathBuf::from(name) } else { self.dir.join(name) };
        if self.purpose.is_save() {
            let exts = self.purpose.filters()[0].1;
            if !has_ext(&path, exts) {
                let mut s = path.into_os_string();
                s.push(".");
                s.push(exts[0]);
                path = PathBuf::from(s);
            }
        }
        Some(path)
    }

    /// Подтвердить выбор (кнопка, Enter, двойной клик по файлу).
    fn confirm(&mut self) -> Option<DialogResult> {
        let path = self.target_path()?;
        if path.is_dir() {
            // Введено имя папки — заходим в неё, а не "открываем".
            if self.purpose == Purpose::ImportAnimate && path.join("DOMDocument.xml").is_file() {
                return self.finish(path);
            }
            self.file_name.clear();
            self.navigate(path, true);
            return None;
        }
        if self.purpose.is_save() {
            if path.exists() && self.confirm_overwrite.as_ref() != Some(&path) {
                self.confirm_overwrite = Some(path);
                return None;
            }
            if !path.parent().is_some_and(|d| d.is_dir()) {
                self.error = Some(format!("Папки {} не существует", path.parent().unwrap_or(Path::new("")).display()));
                return None;
            }
        } else if !path.is_file() {
            self.error = Some(format!("Файл не найден: {}", path.display()));
            return None;
        }
        self.finish(path)
    }

    fn finish(&mut self, path: PathBuf) -> Option<DialogResult> {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.dir.clone());
        self.remember(&dir);
        self.open = false;
        self.confirm_overwrite = None;
        Some(DialogResult { purpose: self.purpose, path })
    }

    fn select(&mut self, i: usize) {
        self.selected = Some(i);
        if let Some(e) = self.entries.get(i) {
            if !e.is_dir {
                self.file_name = e.name.clone();
            } else if !self.purpose.is_save() {
                self.file_name.clear();
            }
        }
        self.confirm_overwrite = None;
    }

    fn activate(&mut self, i: usize) -> Option<DialogResult> {
        let e = self.entries.get(i)?;
        if e.is_dir {
            if self.purpose == Purpose::ImportAnimate && e.path.join("DOMDocument.xml").is_file() {
                let p = e.path.clone();
                return self.finish(p);
            }
            let p = e.path.clone();
            if !self.purpose.is_save() {
                self.file_name.clear();
            }
            self.navigate(p, true);
            None
        } else {
            self.file_name = e.name.clone();
            self.confirm()
        }
    }

    /// Нарисовать окно; `Some` — пользователь выбрал путь.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<DialogResult> {
        if !self.open {
            return None;
        }
        let mut result = None;
        let mut open = true;
        egui::Window::new(self.purpose.title())
            .id(egui::Id::new("pony_file_dialog"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_size(egui::vec2(DIALOG_W, DIALOG_H))
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.screen_rect().center())
            .show(ctx, |ui| result = self.contents(ui));
        if !open {
            self.open = false;
        }
        result
    }

    fn contents(&mut self, ui: &mut egui::Ui) -> Option<DialogResult> {
        let mut result = None;
        let typing = ui.ctx().wants_keyboard_input();

        // --- панель навигации ---
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.history.is_empty(), egui::Button::new("◀")).on_hover_text("Назад").clicked() {
                if let Some(prev) = self.history.pop() {
                    self.navigate(prev, false);
                }
            }
            let parent = self.dir.parent().map(Path::to_path_buf);
            if ui.add_enabled(parent.is_some(), egui::Button::new("⬆")).on_hover_text("Наверх (Backspace)").clicked() {
                if let Some(p) = parent {
                    self.navigate(p, true);
                }
            }
            if ui.button("⟳").on_hover_text("Обновить").clicked() {
                self.refresh();
            }
            let r = ui.add(egui::TextEdit::singleline(&mut self.path_edit).desired_width(ui.available_width() - 210.0).hint_text("путь к папке"));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let p = PathBuf::from(self.path_edit.trim());
                if p.is_dir() {
                    self.navigate(p, true);
                } else if p.is_file() {
                    if let Some(dir) = p.parent() {
                        self.navigate(dir.to_path_buf(), true);
                    }
                    self.file_name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                } else {
                    self.error = Some(format!("Нет такой папки: {}", p.display()));
                }
            }
            ui.add(egui::TextEdit::singleline(&mut self.search).desired_width(130.0).hint_text("🔍 поиск"));
            ui.checkbox(&mut self.show_hidden, "скрытые").on_hover_text("Показывать файлы и папки, начинающиеся с точки");
        });

        // --- хлебные крошки ---
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let mut acc = PathBuf::new();
            let mut jump = None;
            for comp in self.dir.components() {
                acc.push(comp.as_os_str());
                let label = match comp {
                    std::path::Component::RootDir => "/".to_string(),
                    other => other.as_os_str().to_string_lossy().into_owned(),
                };
                if ui.small_button(label).clicked() {
                    jump = Some(acc.clone());
                }
                ui.weak("›");
            }
            if let Some(p) = jump {
                self.navigate(p, true);
            }
        });
        ui.separator();

        let body_h = BODY_H;
        ui.horizontal_top(|ui| {
            // --- быстрые места ---
            ui.vertical(|ui| {
                ui.set_width(PLACES_W);
                egui::ScrollArea::vertical().id_source("fd_places").max_height(body_h).show(ui, |ui| {
                    ui.strong("Места");
                    let mut go = None;
                    for (label, path) in places() {
                        if ui.selectable_label(self.dir == path, label).on_hover_text(path.display().to_string()).clicked() {
                            go = Some(path);
                        }
                    }
                    if !self.recent.is_empty() {
                        ui.add_space(6.0);
                        ui.strong("Недавние");
                        for path in self.recent.clone() {
                            let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
                            if ui.selectable_label(self.dir == path, format!("🕘 {label}")).on_hover_text(path.display().to_string()).clicked() {
                                go = Some(path);
                            }
                        }
                    }
                    if let Some(p) = go {
                        self.navigate(p, true);
                    }
                });
            });
            ui.separator();

            // --- список файлов ---
            ui.vertical(|ui| {
                let visible: Vec<usize> = (0..self.entries.len()).filter(|&i| self.visible(&self.entries[i])).collect();
                // Стрелки — выбор (когда фокус не в поле ввода).
                if !typing && !visible.is_empty() {
                    let pos = self.selected.and_then(|s| visible.iter().position(|&v| v == s));
                    let (down, up) = ui.input(|i| (i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowUp)));
                    let next = if down {
                        Some(pos.map_or(0, |p| (p + 1).min(visible.len() - 1)))
                    } else if up {
                        Some(pos.map_or(0, |p| p.saturating_sub(1)))
                    } else {
                        None
                    };
                    if let Some(n) = next {
                        self.select(visible[n]);
                        self.scroll_to_selected = true;
                    }
                }
                egui::ScrollArea::vertical().id_source("fd_files").max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
                    if visible.is_empty() {
                        ui.weak("(пусто — нет подходящих файлов)");
                    }
                    let mut clicked = None;
                    let mut activated = None;
                    // Строки рисуются вручную: имя слева, размер и дата — в
                    // колонках справа фиксированной ширины (через виджеты
                    // egui строки центрировались и вылезали за окно).
                    let row_h = 20.0;
                    let font = egui::FontId::proportional(13.0);
                    let small = egui::FontId::proportional(11.5);
                    for &i in &visible {
                        let e = &self.entries[i];
                        let icon = if e.is_dir {
                            "🗀"
                        } else if has_ext(&e.path, &["png", "gif", "psd", "kra"]) {
                            "🖼"
                        } else if has_ext(&e.path, &["svg", "fla", "xfl"]) {
                            "✏"
                        } else {
                            "🗋"
                        };
                        let is_sel = self.selected == Some(i);
                        let width = ui.available_width();
                        let (rect, r) = ui.allocate_exact_size(egui::vec2(width, row_h), egui::Sense::click());
                        let visuals = ui.visuals();
                        if is_sel {
                            ui.painter().rect_filled(rect, 3.0, visuals.selection.bg_fill);
                        } else if r.hovered() {
                            ui.painter().rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
                        }
                        let text_color = if is_sel { visuals.selection.stroke.color } else { visuals.text_color() };
                        let weak = visuals.weak_text_color();
                        let date_x = rect.right() - 6.0;
                        let size_x = rect.right() - 128.0;
                        let name_clip = egui::Rect::from_min_max(rect.min, egui::pos2(size_x - 70.0, rect.max.y));
                        ui.painter().with_clip_rect(name_clip).text(
                            egui::pos2(rect.left() + 6.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            format!("{icon}  {}", e.name),
                            font.clone(),
                            text_color,
                        );
                        if !e.is_dir {
                            ui.painter().text(egui::pos2(size_x, rect.center().y), egui::Align2::RIGHT_CENTER, human_size(e.size), small.clone(), weak);
                        }
                        if let Some(m) = e.modified {
                            ui.painter().text(egui::pos2(date_x, rect.center().y), egui::Align2::RIGHT_CENTER, format_time(m), small.clone(), weak);
                        }
                        let r = r.on_hover_text(e.path.display().to_string());
                        if is_sel && self.scroll_to_selected {
                            r.scroll_to_me(None);
                        }
                        if r.double_clicked() {
                            activated = Some(i);
                        } else if r.clicked() {
                            clicked = Some(i);
                        }
                    }
                    self.scroll_to_selected = false;
                    if let Some(i) = clicked {
                        self.select(i);
                    }
                    if let Some(i) = activated {
                        result = self.activate(i);
                    }
                });
            });
        });

        ui.separator();
        // --- имя файла, фильтр, кнопки ---
        let mut submit = false;
        ui.horizontal(|ui| {
            ui.label(if self.purpose.is_save() { "Имя файла:" } else { "Файл:" });
            let r = ui.add(egui::TextEdit::singleline(&mut self.file_name).desired_width(ui.available_width() - 260.0));
            if r.changed() {
                self.confirm_overwrite = None;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            let filters = self.purpose.filters();
            let mut labels: Vec<&str> = filters.iter().map(|(l, _)| *l).collect();
            labels.push("Все файлы");
            egui::ComboBox::from_id_source("fd_filter").width(240.0).selected_text(labels[self.filter_index.min(labels.len() - 1)]).show_ui(ui, |ui| {
                for (i, l) in labels.iter().enumerate() {
                    ui.selectable_value(&mut self.filter_index, i, *l);
                }
            });
        });
        ui.horizontal(|ui| {
            if self.purpose.is_save() {
                match &mut self.new_folder {
                    None => {
                        if ui.button("📁+ Новая папка").clicked() {
                            self.new_folder = Some(String::new());
                        }
                    }
                    Some(name) => {
                        let r = ui.add(egui::TextEdit::singleline(name).desired_width(160.0).hint_text("имя папки"));
                        let create = ui.button("Создать").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        if create {
                            let path = self.dir.join(name.trim());
                            match std::fs::create_dir(&path) {
                                Ok(()) => {
                                    self.new_folder = None;
                                    self.navigate(path, true);
                                }
                                Err(e) => self.error = Some(format!("Не удалось создать папку: {e}")),
                            }
                        } else if ui.button("✕").clicked() {
                            self.new_folder = None;
                        }
                    }
                }
            }
            if self.purpose == Purpose::ImportAnimate && self.dir.join("DOMDocument.xml").is_file() {
                if ui.button("Открыть эту папку XFL").clicked() {
                    let p = self.dir.clone();
                    result = self.finish(p);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Отмена").clicked() {
                    self.open = false;
                }
                let can = !self.file_name.trim().is_empty();
                if ui.add_enabled(can, egui::Button::new(self.purpose.confirm_label())).clicked() {
                    submit = true;
                }
                if let Some(p) = &self.confirm_overwrite {
                    ui.colored_label(egui::Color32::from_rgb(240, 180, 90), format!("«{}» уже есть — нажми ещё раз, чтобы заменить", p.file_name().unwrap_or_default().to_string_lossy()));
                } else if let Some(err) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(240, 110, 110), err);
                }
            });
        });

        // Клавиши окна: Enter — подтвердить, Esc — закрыть, Backspace — наверх.
        if !typing {
            let (enter, esc, back) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Backspace)));
            if esc {
                self.open = false;
            } else if back {
                if let Some(p) = self.dir.parent().map(Path::to_path_buf) {
                    self.navigate(p, true);
                }
            } else if enter {
                match self.selected.and_then(|i| self.entries.get(i)).map(|e| e.is_dir) {
                    Some(true) => result = self.activate(self.selected.unwrap()),
                    _ => submit = true,
                }
            }
        }
        if submit && result.is_none() {
            result = self.confirm();
        }
        result
    }
}

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// `\\?\C:\...` от canonicalize на Windows — некрасиво и ломает крошки.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).filter(|p| p.is_dir())
}

/// Быстрые места: что реально существует на этой машине.
fn places() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        out.push(("🐴 Проект".to_string(), cwd));
    }
    if let Some(home) = home_dir() {
        out.push(("🏠 Домашняя".to_string(), home.clone()));
        for (label, names) in [("🖥 Рабочий стол", ["Desktop", "Рабочий стол"]), ("📄 Документы", ["Documents", "Документы"]), ("⬇ Загрузки", ["Downloads", "Загрузки"])] {
            if let Some(p) = names.iter().map(|n| home.join(n)).find(|p| p.is_dir()) {
                out.push((label.to_string(), p));
            }
        }
    }
    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let p = PathBuf::from(format!("{}:\\", letter as char));
            if p.is_dir() {
                out.push((format!("🖴 {}:", letter as char), p));
            }
        }
    } else {
        out.push(("🖴 /".to_string(), PathBuf::from("/")));
    }
    out
}

fn recent_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home_dir().map(|h| h.join(".config")))
    })?;
    Some(base.join("pony-engine").join("recent_dirs.txt"))
}

fn load_recent() -> Vec<PathBuf> {
    recent_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|s| s.lines().map(PathBuf::from).filter(|p| p.is_dir()).take(MAX_RECENT).collect())
        .unwrap_or_default()
}

fn save_recent(dirs: &[PathBuf]) {
    if cfg!(test) {
        return; // тесты не трогают настройки пользователя
    }
    if let Some(f) = recent_file() {
        if let Some(parent) = f.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text: String = dirs.iter().map(|d| format!("{}\n", d.display())).collect();
        let _ = std::fs::write(f, text);
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["Б", "КБ", "МБ", "ГБ"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Дата изменения "ГГГГ-ММ-ДД ЧЧ:ММ" (UTC) без внешних зависимостей.
fn format_time(t: SystemTime) -> String {
    let Ok(d) = t.duration_since(SystemTime::UNIX_EPOCH) else { return String::new() };
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Гражданский календарь из числа дней (алгоритм Хиннанта).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_target_gets_default_extension_once() {
        let mut d = FileDialog::default();
        d.purpose = Purpose::SaveDocument;
        d.dir = PathBuf::from("/tmp");
        d.file_name = "pony".into();
        assert_eq!(d.target_path(), Some(PathBuf::from("/tmp/pony.asset")));
        d.file_name = "pony.asset".into();
        assert_eq!(d.target_path(), Some(PathBuf::from("/tmp/pony.asset")));
        d.file_name = "  ".into();
        assert_eq!(d.target_path(), None);
    }

    #[test]
    fn filter_shows_only_matching_files_but_all_folders() {
        let mut d = FileDialog::default();
        d.purpose = Purpose::OpenDocument;
        let file = |n: &str, dir: bool| Entry { name: n.into(), path: PathBuf::from(n), is_dir: dir, size: 0, modified: None };
        assert!(d.visible(&file("a.asset", false)));
        assert!(d.visible(&file("b.FLA", false)));
        assert!(!d.visible(&file("c.png", false)));
        assert!(d.visible(&file("folder", true)));
        assert!(!d.visible(&file(".hidden", true)));
        d.filter_index = 3; // «Все файлы»
        assert!(d.visible(&file("c.png", false)));
    }

    #[test]
    fn saving_over_existing_file_asks_for_confirmation_first() {
        let dir = std::env::temp_dir().join(format!("pony_fd_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.asset"), "old").unwrap();
        let mut d = FileDialog::default();
        d.show_for(Purpose::SaveDocument, Some(&dir), "x.asset");
        assert!(d.confirm().is_none(), "первое нажатие — только предупреждение");
        assert!(d.confirm_overwrite.is_some());
        let r = d.confirm().expect("второе — подтверждение");
        assert_eq!(r.path.file_name().unwrap(), "x.asset");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn time_and_size_formatting() {
        assert_eq!(format_time(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)), "2023-11-14 22:13");
        assert_eq!(human_size(512), "512 Б");
        assert_eq!(human_size(1536), "1.5 КБ");
    }
}
