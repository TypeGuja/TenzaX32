//! Настоящее окно (не консоль): winit + egui + egui-wgpu, собраны вручную
//! без `eframe` (см. Cargo.toml). Интерфейс воспроизводит структуру Adobe
//! Animate/Moho, а не просто "панель со слайдером":
//!
//! - **Меню сверху** (File/Edit/View/.../Help) + вторичный тулбар
//!   (New/Open/Save/Undo/Redo/Publish/Zoom/Workspace).
//! - **Палитра инструментов слева** (Selection/Transform/Lasso, Pen/Pencil/
//!   Brush/Line/Rectangle/Oval/PolyStar, PaintBucket/InkBottle/Eyedropper,
//!   Hand/Zoom, Stroke/Fill) — большинство визуальные заглушки: движок не
//!   рисует произвольную векторную графику, только части персонажа. Реально
//!   работают Hand (пан Stage) и колесо мыши (зум Stage).
//! - **Stage по центру**: холст с рамкой экспортной области, линейками,
//!   панорамированием и зумом — не просто картинка на весь экран.
//! - **Timeline снизу**: слои встроены В Timeline (как в Animate — это одна
//!   панель, не Layers отдельно), сетка кадров (не время), ключевые кадры
//!   как маркеры в ячейках, плейхед.
//! - **Вкладки справа**: Properties/Library/Color/Align/Transform/Info —
//!   ровно как в описании; плюс Script — движковая вкладка сверх набора
//!   Animate (у Animate скриптовой консоли rhai быть не может, это наше).
//! - **Playback bar** (кадр/FPS/сцена/плейбек/зум) и **Status bar**
//!   (инструмент/выделение/размер документа/зум) внизу.
//!
//! Единственная точка запуска — этот бинарник (`cargo run` без `-p`,
//! см. `default-members` в корневом Cargo.toml).
//!
//! Для автоматизированного скриншота под Xvfb: `PONY_GUI_AUTOEXIT_MS=<мс>`.

mod file_dialog;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use pony_core::animation::{AnimTarget, AnimValue, Animation, BoneChannel, Interpolation, Keyframe, Track};
use pony_core::part::{Part, PartKind, PartSource};
use pony_core::skeleton::default_pony_skeleton;
use pony_core::{AnimationPlayer, Character};
use pony_render::{export_gif, export_spritesheet, GpuContext, Renderer};
use pony_system::gpu::{GpuAdapterInfo, GpuDeviceType};
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;
use winit::window::WindowBuilder;

// Раньше 480x360 — при обычном использовании (Stage не на весь экран,
// зум > 100% для точной работы с мелкими частями/узлами) разрешение
// быстро становилось заметно "пиксельным". Увеличено вдвое — вместе с
// MSAA (см. pony-render) и LINEAR-фильтрацией (см. ниже) даёт заметно
// более гладкую картинку. Цена — вдвое больше пикселей на GPU-рендер и
// CPU-readback каждый кадр; для сцены с квадами частей персонажа (не
// сложная 3D-геометрия) это остаётся дешёвым.
const SCENE_WIDTH: u32 = 960;
const SCENE_HEIGHT: u32 = 720;
const FPS: f32 = 24.0;
const MIN_STAGE_ZOOM: f32 = 0.05;
const MAX_STAGE_ZOOM: f32 = 64.0;

/// Пустой персонаж для нового документа: только корневая кость, без
/// частей и анимаций. Редактор открывается с ним (раньше — с демо-пони).
fn empty_character() -> Character {
    let mut character = Character::new("Новый персонаж");
    character.skeleton.add_bone(pony_core::skeleton::Bone {
        id: "Root".into(),
        parent: None,
        local_transform: pony_core::skeleton::Transform2D::default(),
        length: 0.0,
    });
    character
}

/// Кость, к которой цеплять новые части (импорт, пустой слой, рисунок):
/// "Body", если она есть (демо-пони), иначе корневая — у пустого
/// персонажа и паппетов из Animate кости "Body" нет.
fn default_bone(character: &Character) -> String {
    if character.skeleton.find("Body").is_some() {
        return "Body".into();
    }
    character
        .skeleton
        .bones
        .iter()
        .find(|b| b.parent.is_none())
        .map(|b| b.id.clone())
        .unwrap_or_else(|| "Root".into())
}

/// Демо-персонаж (File > Демо-пони): части из assets/ и анимация "Walk".
fn build_walking_pony() -> Character {
    let mut character = Character::new("GuiPony");
    character.skeleton = default_pony_skeleton();

    character
        .add_part(Part::new("body", PartKind::Body, PartSource::Png { path: "assets/pony/body.png".into() }).with_bone("Body").with_layer(0))
        .add_part(Part::new("head", PartKind::Head, PartSource::Png { path: "assets/pony/head.png".into() }).with_bone("Head").with_layer(1))
        .add_part(Part::new("horn", PartKind::Horn, PartSource::Png { path: "assets/pony/horn.png".into() }).with_bone("Horn").with_layer(2))
        .add_part(Part::new("ear_l", PartKind::Ear, PartSource::Png { path: "assets/pony/ear.png".into() }).with_bone("EarL").with_layer(2))
        .add_part(Part::new("ear_r", PartKind::Ear, PartSource::Png { path: "assets/pony/ear.png".into() }).with_bone("EarR").with_layer(2))
        .add_part(Part::new("eye_l", PartKind::Eyes, PartSource::Png { path: "assets/pony/eye.png".into() }).with_bone("Head").with_layer(2))
        .add_part(Part::new("leg_fl", PartKind::LegFL, PartSource::Png { path: "assets/pony/leg.png".into() }).with_bone("LowerLegFL").with_layer(-1))
        .add_part(Part::new("leg_fr", PartKind::LegFR, PartSource::Png { path: "assets/pony/leg.png".into() }).with_bone("LowerLegFR").with_layer(-1))
        // Крыло поверх тела, хвост и ноги — за ним. Раньше всё это было на
        // одном слое 0 с телом, и порядок между ними решал случай.
        .add_part(Part::new("wing_l", PartKind::Wing, PartSource::Vector { path: "assets/pony_svg/wing.svg".into() }).with_bone("Body").with_layer(1))
        .add_part(Part::new("tail", PartKind::Tail, PartSource::Vector { path: "assets/pony_svg/tail.svg".into() }).with_bone("Body").with_layer(-1))
        .add_part(Part::new("mouth", PartKind::Mouth, PartSource::Vector { path: "assets/pony_svg/mouth.svg".into() }).with_bone("Head").with_layer(2));

    character.add_animation(Animation {
        name: "Walk".into(),
        duration: 0.8,
        looping: true,
        tracks: vec![Track {
            target: AnimTarget::Bone { id: "Head".into(), channel: BoneChannel::PositionY },
            keyframes: vec![
                Keyframe { time: 0.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
                Keyframe { time: 0.4, value: AnimValue::Float(-6.0), interpolation: Interpolation::Linear },
                Keyframe { time: 0.8, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
            ],
        }],
    });

    character
}

/// Инструмент активной палитры. Названия — короткие текстовые метки, не
/// иконки: у нас нет набора иконок-ассетов, а рисовать штриховые пиктограммы
/// вручную в egui::Painter ради косметики — не стоило времени. Честная
/// текстовая замена вместо притворства, что это "настоящие" иконки Animate.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Selection,
    SubSelection,
    FreeTransform,
    Lasso,
    Pen,
    Pencil,
    Brush,
    Line,
    Rectangle,
    Oval,
    PolyStar,
    PaintBucket,
    InkBottle,
    Eyedropper,
    Hand,
    Zoom,
}

impl Tool {
    fn label(self) -> &'static str {
        match self {
            Tool::Selection => "Sel",
            Tool::SubSelection => "SubSel",
            Tool::FreeTransform => "Xform",
            Tool::Lasso => "Lasso",
            Tool::Pen => "Pen",
            Tool::Pencil => "Pencil",
            Tool::Brush => "Brush",
            Tool::Line => "Line",
            Tool::Rectangle => "Rect",
            Tool::Oval => "Oval",
            Tool::PolyStar => "Poly",
            Tool::PaintBucket => "Fill",
            Tool::InkBottle => "Ink",
            Tool::Eyedropper => "Drop",
            Tool::Hand => "Hand",
            Tool::Zoom => "Zoom",
        }
    }

    /// Подсказка для тултипа палитры и статус-бара: что делает инструмент
    /// и его горячая клавиша (как в Animate).
    fn hint(self) -> &'static str {
        match self {
            Tool::Selection => "Selection (V): клик — выбрать часть, тащить — двигать её",
            Tool::SubSelection => "SubSelection (A): клик — выбрать фигуру холста, тащить узлы, Delete — удалить",
            Tool::FreeTransform => "Free Transform (Q): тащить — двигать кость части, Shift — вращать вокруг её точки, Alt+клик — кость-родитель (вся голова)",
            Tool::Lasso => "Lasso (L): рамкой выделить несколько частей (меню Edit — групповые операции)",
            Tool::Pen => "Pen (P): клик — точка, клик по первой — замкнуть, Enter — закончить, Esc — отмена",
            Tool::Pencil => "Pencil (Y): рисовать тонкой линией",
            Tool::Brush => "Brush (B): рисовать толстой линией",
            Tool::Line => "Line (N): тащить — отрезок",
            Tool::Rectangle => "Rectangle (R): тащить — прямоугольник",
            Tool::Oval => "Oval (O): тащить — эллипс",
            Tool::PolyStar => "PolyStar: тащить от центра — правильный многоугольник",
            Tool::PaintBucket => "Paint Bucket (K): клик по фигуре холста — залить цветом Fill",
            Tool::InkBottle => "Ink Bottle (S): клик по фигуре холста — обводка цветом Stroke",
            Tool::Eyedropper => "Eyedropper (I): клик — взять цвет со Stage в Fill",
            Tool::Hand => "Hand (H): тащить — панорама (средняя кнопка работает в любом инструменте)",
            Tool::Zoom => "Zoom (Z): клик — приблизить, правый клик — отдалить",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RightTab {
    Properties,
    Library,
    Color,
    Align,
    Transform,
    Info,
    Script,
    Lighting,
    Particles,
    Bones,
    SvgInspector,
    Symbols,
    Groups,
    Gradients,
    Boolean,
}

impl RightTab {
    fn label(self) -> &'static str {
        match self {
            RightTab::Properties => "Properties",
            RightTab::Library => "Library",
            RightTab::Color => "Color",
            RightTab::Align => "Align",
            RightTab::Transform => "Transform",
            RightTab::Info => "Info",
            RightTab::Script => "Script",
            RightTab::Lighting => "Lighting",
            RightTab::Particles => "Particles",
            RightTab::Bones => "Bones",
            RightTab::SvgInspector => "SVG",
            RightTab::Symbols => "Symbols",
            RightTab::Groups => "Groups",
            RightTab::Gradients => "Gradients",
            RightTab::Boolean => "Combine Paths",
        }
    }
}

/// Timeline как в Animate: слои встроены прямо в панель (левая колонка —
/// имя + видимость + блокировка, ЗАФИКСИРОВАНА и не скроллится по
/// горизонтали), справа — прокручиваемая сетка КАДРОВ (не времени),
/// ключевые кадры — кружки в ячейках, плейхед. Один слой может не иметь
/// собственной дорожки анимации (нормально, как в Animate — статичные слои).
///
/// Взаимодействия с сеткой:
/// - Клик по клетке — перейти на этот кадр (и выбрать слой строки). Раньше
///   ЛЮБОЙ клик по пустой клетке создавал ключевой кадр — просто
///   переставить плейхед, не насоряв ключами, было нельзя.
/// - Двойной клик по клетке (или F6 / Insert > Keyframe) — ключевой кадр
///   с текущим значением выбранного канала кости слоя.
/// - Клик по существующему ключу — выбрать его; перетаскивание — сдвинуть
///   по времени (значение сохраняется; на занятый кадр ключ не встаёт —
///   раньше он молча затирал чужой ключ).
/// - Delete/Backspace — удалить выбранный ключ (только когда фокус не в
///   текстовом поле — раньше Backspace в редакторе скрипта удалял ключи).
/// - Сетка показывает кадры и ПОСЛЕ конца анимации: двойной клик там
///   ставит ключ и удлиняет анимацию (раньше клики зажимались на последнем
///   кадре и удлинить анимацию с таймлайна было невозможно).
#[allow(clippy::too_many_arguments)]
fn timeline_widget(
    ui: &mut egui::Ui,
    character: &mut Character,
    player: &mut AnimationPlayer,
    playing: &mut bool,
    hidden: &mut HashSet<String>,
    locked: &mut HashSet<String>,
    selected: &mut Option<String>,
    timeline_zoom: &mut f32,
    selected_keyframe: &mut Option<(String, i64)>,
    timeline_channel: &mut BoneChannel,
    selected_bone: &Option<String>,
) {
    // --- шапка: какая анимация, её длительность/цикл, зум, канал ---
    ui.horizontal(|ui| {
        let mut names: Vec<String> = character.animations.keys().cloned().collect();
        names.sort();
        let current = player.current_name().map(str_to_owned);
        ui.label("Анимация:");
        let mut pick: Option<String> = None;
        egui::ComboBox::from_id_source("timeline_anim").selected_text(current.clone().unwrap_or_else(|| "(нет)".into())).show_ui(ui, |ui| {
            for name in &names {
                if ui.selectable_label(current.as_deref() == Some(name.as_str()), name).clicked() {
                    pick = Some(name.clone());
                }
            }
        });
        if let Some(name) = pick {
            player.play(&name);
            player.apply(character);
            *selected_keyframe = None;
        }
        if ui.button("+").on_hover_text("Новая пустая анимация").clicked() {
            let name = unique_animation_name(character);
            character.add_animation(Animation { name: name.clone(), duration: 1.0, looping: true, tracks: Vec::new() });
            player.play(&name);
            *playing = false;
            *selected_keyframe = None;
        }
        if let Some(anim) = current.as_deref().and_then(|n| character.animations.get_mut(n)) {
            ui.add(egui::DragValue::new(&mut anim.duration).speed(0.01).clamp_range(1.0 / FPS..=600.0).suffix(" с").max_decimals(2))
                .on_hover_text("Длительность анимации");
            ui.checkbox(&mut anim.looping, "Loop");
        }
        ui.separator();
        ui.label("Zoom:");
        ui.add(egui::Slider::new(timeline_zoom, 0.25..=6.0).fixed_decimals(2));
        ui.separator();
        ui.label("Канал:");
        egui::ComboBox::from_id_source("timeline_channel").selected_text(channel_label(*timeline_channel)).show_ui(ui, |ui| {
            for ch in [BoneChannel::PositionX, BoneChannel::PositionY, BoneChannel::Rotation, BoneChannel::ScaleX, BoneChannel::ScaleY] {
                ui.selectable_value(timeline_channel, ch, channel_label(ch));
            }
        });
    });
    ui.small("клик — перейти на кадр · двойной клик / F6 — ключ · тащить кружок — сдвинуть · Delete — удалить ключ · Space — play/pause · стрелки ◀/▶ — кадр");

    let anim_name = player.current_name().map(str_to_owned);
    let anim_data: Option<(f32, Vec<Track>)> = anim_name.as_deref().and_then(|n| character.animations.get(n)).map(|a| (a.duration, a.tracks.clone()));
    let (duration, tracks) = anim_data.unwrap_or((1.0, Vec::new()));
    let duration = duration.max(0.01);
    let total_frames = (duration * FPS).round().max(1.0) as i64;

    let mut layer_ids: Vec<String> = character.parts.keys().cloned().collect();
    layer_ids.sort_by_key(|id| (character.parts[id].layer, id.clone()));

    // Дорожка привязывается к слою, если её AnimTarget::Bone указывает на
    // кость этого слоя. Дорожки без такой привязки (Morph/EyeParam/Camera)
    // собираются в один дополнительный ряд "Прочее" внизу.
    //
    // НЕСКОЛЬКО частей могут ссылаться на ОДНУ кость (в демо-персонаже
    // "tail" и "body" обе висят на "Body") — дорожка привязана к кости, а не
    // к части, поэтому показывается на ВСЕХ строках, чьи части используют
    // эту кость: это и есть правда о том, что реально анимируется.
    //
    // Показываем ТОЛЬКО дорожки выбранного канала (селектор "Канал").
    // Плюс строки для костей БЕЗ своих частей, если их анимируют или они
    // выбраны: у импортированных из Animate паппетов это кости-контейнеры
    // (вся голова) — иначе их ключи попадали в "Прочее" и не редактировались.
    let used_bones: HashSet<&str> = character.parts.values().filter_map(|p| p.bone.as_deref()).collect();
    let animated_bones: HashSet<&str> = tracks.iter().filter_map(|t| match &t.target {
        AnimTarget::Bone { id, .. } => Some(id.as_str()),
        _ => None,
    }).collect();
    for bone in &character.skeleton.bones {
        if bone.parent.is_some() && !used_bones.contains(bone.id.as_str()) && (animated_bones.contains(bone.id.as_str()) || selected_bone.as_deref() == Some(bone.id.as_str())) {
            layer_ids.push(format!("{BONE_ROW_PREFIX}{}", bone.id));
        }
    }
    let mut layer_frames: Vec<(String, Vec<i64>)> = layer_ids.iter().map(|id| (id.clone(), Vec::new())).collect();
    let mut misc_frames: Vec<i64> = Vec::new();
    for track in &tracks {
        let bone_id = match &track.target {
            AnimTarget::Bone { id, channel } if *channel == *timeline_channel => Some(id.as_str()),
            AnimTarget::Bone { .. } => continue, // другой канал той же кости — не в этом виде
            _ => None,
        };
        let frames: Vec<i64> = track.keyframes.iter().map(|kf| (kf.time * FPS).round() as i64).collect();
        match bone_id {
            Some(bid) => {
                let mut any_matched = false;
                for (lid, entry_frames) in layer_frames.iter_mut() {
                    if row_bone(character, lid).as_deref() == Some(bid) {
                        entry_frames.extend(frames.iter().copied());
                        any_matched = true;
                    }
                }
                if !any_matched {
                    misc_frames.extend(frames);
                }
            }
            None => misc_frames.extend(frames),
        }
    }

    let row_height = 20.0;
    let ruler_height = 18.0;
    let label_col_width = 190.0;
    let has_misc = !misc_frames.is_empty();
    let row_count = layer_ids.len() + if has_misc { 1 } else { 0 };
    let total_height = ruler_height + row_height * row_count.max(1) as f32;

    let mut row_labels: Vec<(String, Option<Vec<i64>>)> = layer_ids
        .iter()
        .zip(layer_frames.into_iter().map(|(_, frames)| frames))
        .map(|(id, frames)| (id.clone(), Some(frames)))
        .collect();
    if has_misc {
        row_labels.push(("(Прочее: камера/морфы)".to_string(), None));
    }

    let base_frame_w = 8.0;
    let frame_w = (base_frame_w * *timeline_zoom).max(1.5);
    let delete_pressed = !ui.ctx().wants_keyboard_input() && ui.ctx().input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace));

    // Переход на кадр — общий для клика по ruler/клетке и скраба драгом.
    // `seek`, а не `advance(target - current)`: у зацикленной анимации
    // advance на ровно `duration` оборачивался в кадр 0.
    let seek_to_frame = |character: &mut Character, player: &mut AnimationPlayer, playing: &mut bool, frame: i64| {
        *playing = false;
        player.seek(character, (frame.max(0) as f32 / FPS).min(duration));
        player.apply(character);
    };

    // Вертикальная прокрутка — у персонажа может быть больше слоёв, чем
    // помещается по высоте панели; раньше нижние строки просто обрезались.
    egui::ScrollArea::vertical().id_source("timeline_rows_scroll").auto_shrink([false, false]).show(ui, |ui| {
    ui.horizontal_top(|ui| {
        // --- Фиксированная колонка меток слоёв: не в горизонтальной
        // ScrollArea, поэтому остаётся на месте при прокрутке сетки кадров. ---
        let (label_rect, label_response) = ui.allocate_exact_size(egui::vec2(label_col_width, total_height), egui::Sense::click());
        let lp = ui.painter_at(label_rect);
        lp.rect_filled(label_rect, 0.0, egui::Color32::from_gray(20));
        lp.text(
            egui::pos2(label_rect.left() + 4.0, label_rect.top() + ruler_height / 2.0),
            egui::Align2::LEFT_CENTER,
            "👁  🔒  слой",
            egui::FontId::proportional(10.0),
            egui::Color32::from_gray(130),
        );
        for (i, (label, frames_opt)) in row_labels.iter().enumerate() {
            let row_top = label_rect.top() + ruler_height + row_height * i as f32;
            let row_rect = egui::Rect::from_min_max(egui::pos2(label_rect.left(), row_top), egui::pos2(label_rect.right(), row_top + row_height));
            let bg = if i % 2 == 0 { egui::Color32::from_gray(26) } else { egui::Color32::from_gray(22) };
            lp.rect_filled(row_rect, 0.0, bg);

            let is_layer_row = frames_opt.is_some() && character.parts.contains_key(label);
            if !is_layer_row && frames_opt.is_some() {
                // строка кости-контейнера
                if selected_bone.as_deref() == label.strip_prefix(BONE_ROW_PREFIX) {
                    lp.rect_filled(row_rect, 0.0, egui::Color32::from_rgb(70, 60, 30));
                }
                lp.text(
                    egui::pos2(label_rect.left() + 6.0, row_top + row_height / 2.0),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_rgb(230, 200, 120),
                );
            } else if is_layer_row {
                if selected.as_deref() == Some(label.as_str()) {
                    lp.rect_filled(row_rect, 0.0, egui::Color32::from_rgb(45, 65, 90));
                }
                let cb_rect = egui::Rect::from_min_size(egui::pos2(label_rect.left() + 4.0, row_top + row_height / 2.0 - 6.0), egui::vec2(12.0, 12.0));
                let visible = !hidden.contains(label);
                lp.rect_stroke(cb_rect, 2.0, egui::Stroke::new(1.0_f32, egui::Color32::from_gray(150)));
                if visible {
                    lp.rect_filled(cb_rect.shrink(2.5), 1.0, egui::Color32::from_rgb(120, 170, 230));
                }
                let lock_rect = egui::Rect::from_min_size(egui::pos2(label_rect.left() + 20.0, row_top + row_height / 2.0 - 6.0), egui::vec2(12.0, 12.0));
                let is_locked = locked.contains(label);
                lp.rect_stroke(lock_rect, 2.0, egui::Stroke::new(1.0_f32, egui::Color32::from_gray(150)));
                if is_locked {
                    lp.rect_filled(lock_rect.shrink(2.5), 1.0, egui::Color32::from_rgb(230, 170, 90));
                }
                lp.text(
                    egui::pos2(label_rect.left() + 38.0, row_top + row_height / 2.0),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::proportional(11.0),
                    if visible { egui::Color32::from_gray(210) } else { egui::Color32::from_gray(110) },
                );
            } else {
                lp.text(
                    egui::pos2(label_rect.left() + 6.0, row_top + row_height / 2.0),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_gray(150),
                );
            }
        }
        if label_response.clicked() {
            if let Some(pos) = label_response.interact_pointer_pos() {
                let row_i = ((pos.y - (label_rect.top() + ruler_height)) / row_height).floor() as i64;
                if row_i >= 0 && (row_i as usize) < row_labels.len() {
                    let (label, frames_opt) = &row_labels[row_i as usize];
                    if frames_opt.is_some() && character.parts.contains_key(label) {
                        let local_x = pos.x - label_rect.left();
                        if (2.0..18.0).contains(&local_x) {
                            if hidden.contains(label) {
                                hidden.remove(label);
                            } else {
                                hidden.insert(label.clone());
                            }
                        } else if (18.0..34.0).contains(&local_x) {
                            if locked.contains(label) {
                                locked.remove(label);
                            } else {
                                locked.insert(label.clone());
                            }
                        } else {
                            *selected = Some(label.clone());
                        }
                    }
                }
            }
        }

        // --- Прокручиваемая сетка кадров ---
        egui::ScrollArea::horizontal().id_source("timeline_grid_scroll").auto_shrink([false, true]).show(ui, |ui| {
            // Сетка шире самой анимации: +1 секунда после конца (или до
            // края панели) — туда можно поставить ключ и удлинить анимацию.
            let min_frames = (ui.available_width() / frame_w).ceil() as i64;
            let display_frames = (total_frames + FPS as i64).max(min_frames);
            let grid_width = display_frames as f32 * frame_w;
            let (grid_rect, grid_response) = ui.allocate_exact_size(egui::vec2(grid_width, total_height), egui::Sense::click_and_drag());
            let gp = ui.painter_at(grid_rect);
            gp.rect_filled(grid_rect, 0.0, egui::Color32::from_gray(20));

            let ruler_rect = egui::Rect::from_min_max(grid_rect.min, egui::pos2(grid_rect.right(), grid_rect.top() + ruler_height));
            gp.rect_filled(ruler_rect, 0.0, egui::Color32::from_gray(32));
            let tick_every = if frame_w >= 14.0 { 1 } else { (14.0 / frame_w).ceil() as i64 }.max(1);
            for f in (0..=display_frames).step_by(tick_every as usize) {
                let x = grid_rect.left() + f as f32 * frame_w;
                gp.line_segment([egui::pos2(x, ruler_rect.top()), egui::pos2(x, ruler_rect.bottom())], egui::Stroke::new(1.0_f32, egui::Color32::from_gray(70)));
                gp.text(egui::pos2(x + 2.0, ruler_rect.top() + 1.0), egui::Align2::LEFT_TOP, format!("{f}"), egui::FontId::proportional(9.0), egui::Color32::from_gray(160));
            }

            let end_x = grid_rect.left() + total_frames as f32 * frame_w;
            for (i, (label, frames_opt)) in row_labels.iter().enumerate() {
                let row_top = grid_rect.top() + ruler_height + row_height * i as f32;
                let row_rect = egui::Rect::from_min_max(egui::pos2(grid_rect.left(), row_top), egui::pos2(grid_rect.right(), row_top + row_height));
                let bg = if i % 2 == 0 { egui::Color32::from_gray(26) } else { egui::Color32::from_gray(22) };
                gp.rect_filled(row_rect, 0.0, bg);
                if selected.as_deref() == Some(label.as_str()) {
                    gp.rect_filled(row_rect, 0.0, egui::Color32::from_rgba_unmultiplied(70, 100, 140, 50));
                }

                // Тонкая сетка кадров — видно, куда именно кликаешь.
                if frame_w >= 4.0 {
                    for f in 0..=display_frames {
                        let x = grid_rect.left() + f as f32 * frame_w;
                        gp.line_segment([egui::pos2(x, row_top), egui::pos2(x, row_top + row_height)], egui::Stroke::new(0.5_f32, egui::Color32::from_gray(30)));
                    }
                }

                let frames: &[i64] = frames_opt.as_deref().unwrap_or(&misc_frames);
                for &f in frames {
                    let x = grid_rect.left() + f as f32 * frame_w + frame_w / 2.0;
                    let cy = row_top + row_height / 2.0;
                    let is_sel = selected_keyframe.as_ref().map(|(l, ff)| l == label && *ff == f).unwrap_or(false);
                    let r = if is_sel { 5.5 } else { 4.0 };
                    let col = if is_sel { egui::Color32::from_rgb(255, 230, 140) } else { egui::Color32::from_rgb(230, 190, 90) };
                    gp.circle_filled(egui::pos2(x, cy), r, col);
                    gp.circle_stroke(egui::pos2(x, cy), r, egui::Stroke::new(if is_sel { 2.0_f32 } else { 1.0_f32 }, egui::Color32::from_rgb(120, 90, 30)));
                }
            }
            // Всё, что после конца анимации, затемнено — видно, где она
            // кончается, и что ключ там её удлинит.
            gp.rect_filled(egui::Rect::from_min_max(egui::pos2(end_x, ruler_rect.bottom()), grid_rect.max), 0.0, egui::Color32::from_black_alpha(90));
            gp.line_segment([egui::pos2(end_x, grid_rect.top()), egui::pos2(end_x, grid_rect.bottom())], egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(120, 120, 160)));

            let current_frame = (player.time() * FPS).round().clamp(0.0, total_frames as f32);
            let playhead_x = grid_rect.left() + current_frame * frame_w + frame_w / 2.0;
            gp.line_segment([egui::pos2(playhead_x, grid_rect.top()), egui::pos2(playhead_x, grid_rect.bottom())], egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(230, 80, 80)));

            let frame_at = |pos: egui::Pos2| -> i64 { (((pos.x - grid_rect.left()) / frame_w).floor() as i64).clamp(0, display_frames) };
            let row_at = |pos: egui::Pos2| -> Option<usize> {
                let row_i = ((pos.y - (grid_rect.top() + ruler_height)) / row_height).floor() as i64;
                (pos.y >= grid_rect.top() + ruler_height && row_i >= 0 && (row_i as usize) < row_labels.len()).then_some(row_i as usize)
            };

            // Ищет ключ (строка, номер кадра) под точкой — общая логика для
            // выбора и для решения "драг — это скраб или перетаскивание ключа".
            let keyframe_at = |pos: egui::Pos2| -> Option<(String, i64)> {
                let i = row_at(pos)?;
                let (label, frames_opt) = &row_labels[i];
                let frames = frames_opt.as_deref()?;
                // Hit-радиус заметно щедрее видимого кружка (4-5.5px).
                frames
                    .iter()
                    .find(|&&f| (pos.x - (grid_rect.left() + f as f32 * frame_w + frame_w / 2.0)).abs() < (frame_w / 2.0).clamp(7.0, 14.0))
                    .map(|&f| (label.clone(), f))
            };

            if grid_response.drag_started() {
                // `press_origin()`, а не `interact_pointer_pos()`: egui
                // распознаёт драг только после порога сдвига, и к этому
                // моменту позиция уже уехала от настоящего mousedown.
                let pos = ui.input(|i| i.pointer.press_origin());
                *selected_keyframe = pos.and_then(keyframe_at);
            }

            if grid_response.dragged() {
                if let (Some((label, orig_frame)), Some(pos)) = (selected_keyframe.clone(), grid_response.interact_pointer_pos()) {
                    // Тащим существующий ключ по времени — значение сохраняем.
                    let new_frame = frame_at(pos);
                    if new_frame != orig_frame {
                        if let (Some(bone_id), Some(name)) = (row_bone(character, &label), &anim_name) {
                            if let Some(anim) = character.animations.get_mut(name) {
                                let new_t = new_frame as f32 / FPS;
                                // На занятый кадр не встаём — иначе insert
                                // заменил бы чужой ключ, и он бы пропал.
                                if !has_keyframe_at(anim, &bone_id, *timeline_channel, new_t) {
                                    if let Some(value) = remove_keyframe(anim, &bone_id, *timeline_channel, orig_frame as f32 / FPS) {
                                        insert_keyframe(anim, &bone_id, *timeline_channel, new_t, value);
                                        *selected_keyframe = Some((label, new_frame));
                                    }
                                }
                            }
                        }
                    }
                } else if let Some(pos) = grid_response.interact_pointer_pos() {
                    // Не над ключом — перетаскивание скрабит плейхед.
                    seek_to_frame(character, player, playing, frame_at(pos));
                }
            } else if grid_response.double_clicked() {
                if let Some(pos) = grid_response.interact_pointer_pos() {
                    if let Some(row_i) = row_at(pos) {
                        let label = row_labels[row_i].0.clone();
                        if row_labels[row_i].1.is_some() {
                            if character.parts.contains_key(&label) {
                                *selected = Some(label.clone());
                            }
                            match row_bone(character, &label) {
                                Some(bone_id) => {
                                    let frame = frame_at(pos);
                                    let t = frame as f32 / FPS;
                                    let value = character.skeleton.find(&bone_id).map(|b| bone_channel_value(b, *timeline_channel)).unwrap_or(0.0);
                                    let name = ensure_animation(character, player);
                                    if let Some(anim) = character.animations.get_mut(&name) {
                                        insert_keyframe(anim, &bone_id, *timeline_channel, t, value);
                                        *selected_keyframe = Some((label, frame));
                                    }
                                    seek_to_frame(character, player, playing, frame);
                                }
                                None => {}
                            }
                        }
                    }
                }
            } else if grid_response.clicked() {
                if let Some(pos) = grid_response.interact_pointer_pos() {
                    match keyframe_at(pos) {
                        Some(hit) => {
                            let frame = hit.1;
                            if character.parts.contains_key(&hit.0) {
                                *selected = Some(hit.0.clone());
                            }
                            *selected_keyframe = Some(hit);
                            seek_to_frame(character, player, playing, frame);
                        }
                        None => {
                            *selected_keyframe = None;
                            if let Some(row_i) = row_at(pos) {
                                if row_labels[row_i].1.is_some() && character.parts.contains_key(&row_labels[row_i].0) {
                                    *selected = Some(row_labels[row_i].0.clone());
                                }
                            }
                            seek_to_frame(character, player, playing, frame_at(pos));
                        }
                    }
                }
            }

            if delete_pressed {
                if let Some((label, frame)) = selected_keyframe.clone() {
                    if let (Some(bone_id), Some(name)) = (row_bone(character, &label), &anim_name) {
                        if let Some(anim) = character.animations.get_mut(name) {
                            if remove_keyframe(anim, &bone_id, *timeline_channel, frame as f32 / FPS).is_some() {
                                *selected_keyframe = None;
                            }
                        }
                    }
                }
            }
        });
    });
    });
}

/// Префикс строки таймлайна для кости без собственной части.
const BONE_ROW_PREFIX: &str = "кость: ";

/// Кость, к которой относятся ключи строки таймлайна: у части — её кость,
/// у строки-кости — сама кость.
fn row_bone(character: &Character, label: &str) -> Option<String> {
    match character.parts.get(label) {
        Some(part) => part.bone.clone(),
        None => label.strip_prefix(BONE_ROW_PREFIX).map(str::to_string),
    }
}

/// Имя для новой анимации, не занятое у персонажа: "Animation",
/// "Animation 2", ...
fn unique_animation_name(character: &Character) -> String {
    let mut n = 1;
    loop {
        let name = if n == 1 { "Animation".to_string() } else { format!("Animation {n}") };
        if !character.animations.contains_key(&name) {
            return name;
        }
        n += 1;
    }
}

/// Имя текущей анимации; если играть нечего (у загруженного персонажа нет
/// анимаций или ни одна не выбрана) — создаёт пустую и делает текущей.
/// Без этого ключевые кадры молча не ставились: таймлайн выглядел живым,
/// а клики по нему ничего не делали.
fn ensure_animation(character: &mut Character, player: &mut AnimationPlayer) -> String {
    if let Some(name) = player.current_name().filter(|n| character.animations.contains_key(*n)) {
        return name.to_string();
    }
    let name = unique_animation_name(character);
    character.add_animation(Animation { name: name.clone(), duration: 1.0, looping: true, tracks: Vec::new() });
    player.play(&name);
    name
}

/// Выбрать анимацию, с которой открывать персонажа: "Walk", если есть,
/// иначе первая по алфавиту. Плеер ставится на кадр 0, без воспроизведения.
fn pick_default_animation(character: &Character, player: &mut AnimationPlayer) {
    *player = AnimationPlayer::new();
    let name = if character.animations.contains_key("Walk") {
        Some("Walk".to_string())
    } else {
        let mut names: Vec<&String> = character.animations.keys().collect();
        names.sort();
        names.first().map(|n| n.to_string())
    };
    if let Some(name) = name {
        player.play(name);
    }
}

fn color32_to_rgb(c: egui::Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

fn str_to_owned(s: &str) -> String {
    s.to_string()
}

/// Глубина истории отмены. Снимок — это полный клон `Character` + холста,
/// поэтому стек ограничен: 100 шагов на персонажа из десятка частей —
/// единицы мегабайт, а не «пока не кончится память».
const MAX_UNDO_DEPTH: usize = 100;

/// Снимок документа для Undo/Redo: персонаж + холст рисования (VectorDoc).
/// Раньше в историю попадал только персонаж — нарисованную фигуру
/// отменить было нельзя.
#[derive(Clone, PartialEq)]
struct DocState {
    character: Character,
    vector_doc: pony_core::VectorDoc,
}

/// История правок. Работает АВТОМАТИЧЕСКИ: в конце кадра, когда кнопки
/// мыши отпущены, текущее состояние сравнивается с последним
/// зафиксированным, и если оно другое — старое уходит в undo. Один жест
/// (перетаскивание, правка поля) = один шаг отмены.
///
/// Раньше `push_undo` вызывался вручную в отдельных местах, и большая
/// часть правок (ключи на таймлайне, поля Transform/Properties/Bones,
/// перетаскивание кости, рисование, скрипт из вкладки Script) в историю не
/// попадала вовсе — Ctrl+Z "работал через раз".
///
/// Поза, которую плеер пишет в кости при воспроизведении/скрабе, правкой
/// НЕ считается: база перед сравнением прогоняется через тот же плеер.
struct History {
    undo: Vec<DocState>,
    redo: Vec<DocState>,
    committed: DocState,
    /// Растёт на каждом изменении документа — по нему считается "есть
    /// несохранённые правки" (звёздочка в заголовке окна).
    revision: u64,
}

impl History {
    fn new(character: &Character, vector_doc: &pony_core::VectorDoc) -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), committed: DocState { character: character.clone(), vector_doc: vector_doc.clone() }, revision: 0 }
    }

    /// Зафиксировать текущее состояние, если оно реально изменилось.
    /// Возвращает true, если в историю добавлен шаг.
    fn commit_if_changed(&mut self, character: &Character, vector_doc: &pony_core::VectorDoc, player: &AnimationPlayer) -> bool {
        if self.committed.character == *character && self.committed.vector_doc == *vector_doc {
            return false;
        }
        if self.committed.vector_doc == *vector_doc {
            let mut posed = self.committed.character.clone();
            player.apply(&mut posed);
            if posed == *character {
                // Изменилась только поза из анимации — подтягиваем базу,
                // но шагом отмены это не является.
                self.committed.character = character.clone();
                return false;
            }
        }
        let previous = std::mem::replace(&mut self.committed, DocState { character: character.clone(), vector_doc: vector_doc.clone() });
        self.undo.push(previous);
        if self.undo.len() > MAX_UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.revision += 1;
        true
    }

    fn undo(&mut self, character: &Character, vector_doc: &pony_core::VectorDoc) -> Option<DocState> {
        let prev = self.undo.pop()?;
        self.redo.push(DocState { character: character.clone(), vector_doc: vector_doc.clone() });
        self.committed = prev.clone();
        self.revision += 1;
        Some(prev)
    }

    fn redo(&mut self, character: &Character, vector_doc: &pony_core::VectorDoc) -> Option<DocState> {
        let next = self.redo.pop()?;
        self.undo.push(DocState { character: character.clone(), vector_doc: vector_doc.clone() });
        self.committed = next.clone();
        self.revision += 1;
        Some(next)
    }

    /// Начать историю заново (переключение на другого персонажа сцены):
    /// иначе Ctrl+Z подставил бы активному слоту состояние ДРУГОГО персонажа.
    fn reset(&mut self, character: &Character, vector_doc: &pony_core::VectorDoc) {
        self.undo.clear();
        self.redo.clear();
        self.committed = DocState { character: character.clone(), vector_doc: vector_doc.clone() };
        self.revision += 1;
    }
}

/// Действия, которые можно вызвать из меню, тулбара И горячих клавиш —
/// выполняются в одном месте (см. `run_action` в main), чтобы кнопка
/// "Open" на тулбаре и пункт меню File > Open не расходились в поведении
/// (раньше тулбарные New/Open/Save молча игнорировали ошибки и не
/// сбрасывали выделение/скрытые слои).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    New,
    Open,
    Save,
    Undo,
    Redo,
    TogglePlay,
    Rewind,
    GoToEnd,
    PrevFrame,
    NextFrame,
    InsertKeyframe,
    ExportGif,
    ExportSpritesheet,
    SaveAs,
    DemoPony,
    /// Загрузить `.asset` из `asset_path` (после выбора в проводнике).
    OpenPath,
    /// Импорт выбранного в окне "Adobe Animate" символа/кадра как нового
    /// персонажа сцены.
    ImportXfl,
}

/// Папка для SVG-частей, импортированных из Adobe Animate:
/// `imported/<документ>/<символ>_f<кадр>/`. В `.gitignore` — исходники
/// (особенно чужие, как MLP-паппеты) в репозиторий попадать не должны.
fn xfl_import_dir(doc_name: &str, symbol: Option<&str>, frame: usize) -> std::path::PathBuf {
    let clean = |s: &str| -> String { s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect() };
    std::path::Path::new("imported").join(clean(doc_name)).join(format!("{}_f{frame}", clean(symbol.unwrap_or("main"))))
}

/// Символ, который разумно показать сразу после открытия .fla: у паппетов
/// MLP это контейнер персонажа (`~RD_Character`), иначе главная шкала.
fn default_xfl_symbol(doc: &pony_core::xfl::XflDocument) -> Option<String> {
    let names = doc.symbol_names();
    names
        .iter()
        .find(|n| n.starts_with('~') && n.to_lowercase().contains("character"))
        .or_else(|| names.iter().find(|n| n.starts_with('~')))
        .map(|n| n.to_string())
}

/// Как подогнать вид Stage (см. View > Fit ...).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FitView {
    /// Персонаж целиком, крупно — так открывается окно.
    Character,
    /// Вся экспортная область документа (SCENE_WIDTH x SCENE_HEIGHT).
    Document,
}

/// Ассеты подключены относительными путями (`assets/pony/...`). Если
/// бинарник запущен не из корня workspace (из IDE, из `crates/pony-gui`,
/// двойным кликом по exe) — персонаж рисовался цветными заглушками без
/// единой ошибки. Ищем папку `assets` рядом: сначала текущая папка, потом
/// корень workspace, известный на этапе сборки, потом предки exe.
fn find_workspace_root() -> Option<std::path::PathBuf> {
    let has_assets = |dir: &std::path::Path| dir.join("assets").join("pony").is_dir();
    let cwd = std::env::current_dir().ok();
    if cwd.as_deref().is_some_and(has_assets) {
        return None; // уже на месте
    }
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    candidates.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."));
    if let Ok(exe) = std::env::current_exe() {
        candidates.extend(exe.ancestors().skip(1).map(|p| p.to_path_buf()));
    }
    if let Some(cwd) = cwd {
        candidates.extend(cwd.ancestors().skip(1).map(|p| p.to_path_buf()));
    }
    candidates.into_iter().find(|dir| has_assets(dir))
}

/// Часть персонажа под точкой (мировые координаты) — верхняя по порядку
/// отрисовки. Скрытые и заблокированные слои пропускаются. Та же
/// геометрия, что у рендера (`part_world_position`/`part_render_size` +
/// 2.5D-поворот), без учёта поворота самой кости (габарит — AABB).
fn part_at(character: &Character, hidden: &HashSet<String>, locked: &HashSet<String>, world: glam::Vec2) -> Option<String> {
    let mut best: Option<(String, i32)> = None;
    for (id, part) in &character.parts {
        if locked.contains(id) || hidden.contains(id) || character.is_part_hidden_by_group(id) {
            continue;
        }
        let Some((center, half)) = part_screen_box(character, part) else { continue };
        if (center.x - world.x).abs() <= half.x.max(2.0) && (center.y - world.y).abs() <= half.y.max(2.0) {
            // Тот же порядок, что у рендера: (layer, id) — сверху то,
            // что нарисовано последним.
            let better = best.as_ref().map(|(bid, l)| (part.layer, id) > (*l, bid)).unwrap_or(true);
            if better {
                best = Some((id.clone(), part.layer));
            }
        }
    }
    best.map(|(id, _)| id)
}

/// Центр и половины габарита части в мировых координатах — как её видно
/// на Stage (с 2.5D-поворотом персонажа).
fn part_screen_box(character: &Character, part: &Part) -> Option<(glam::Vec2, glam::Vec2)> {
    let world = character.skeleton.world_transform_with_ik(part.bone.as_ref()?)?;
    let part_pos = pony_render::part_world_position(part, &world);
    let depth_z = pony_render::part_depth(character, part);
    let (yawed_x, foreshorten) = pony_core::apply_yaw_2_5d(part_pos.x, depth_z, character.facing_yaw);
    let size = pony_render::part_render_size(part);
    let morph = pony_render::part_morph_scale(character, part);
    let half = glam::Vec2::new(size.x * world.scale.x.abs() * foreshorten * morph.x / 2.0, size.y * world.scale.y.abs() * morph.y / 2.0);
    Some((glam::Vec2::new(yawed_x, part_pos.y), half))
}

/// Габарит всех видимых частей персонажа в мировых координатах — для
/// View > Fit Character.
fn character_bounds(character: &Character, hidden: &HashSet<String>) -> Option<(glam::Vec2, glam::Vec2)> {
    let mut acc: Option<(glam::Vec2, glam::Vec2)> = None;
    for (id, part) in &character.parts {
        if hidden.contains(id) {
            continue;
        }
        let Some((center, half)) = part_screen_box(character, part) else { continue };
        let (lo, hi) = (center - half, center + half);
        acc = Some(match acc {
            Some((a, b)) => (a.min(lo), b.max(hi)),
            None => (lo, hi),
        });
    }
    acc
}

/// То, из чего собран последний отрендеренный кадр Stage — если ничего из
/// этого не изменилось, кадр не перерисовывается (GPU-рендер + чтение
/// пикселей обратно на CPU на КАЖДЫЙ кадр раньше грузили процессор на
/// 100%, даже когда в окне ничего не происходило).
#[derive(PartialEq)]
struct RenderKey {
    character: Character,
    hidden: HashSet<String>,
    camera: pony_core::Camera,
    lighting: pony_core::Lighting,
    size: (u32, u32),
}

/// Первое свободное имя вида `base`, `base_2`, `base_3`... Счётчики вида
/// "drawn_{N}" начинались с 1 при каждом запуске, и после открытия
/// сохранённого персонажа новая фигура получала id уже существующей части
/// (та молча заменялась, а её .svg перезаписывался), второй "Duplicate
/// layer" затирал первую копию, новая кость получала id существующей.
fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}_{n}")).find(|c| !taken(c)).expect("бесконечный ряд кандидатов")
}

/// Длительность моргания от `pony.Blink()`.
const BLINK_SECONDS: f32 = 0.25;

/// Скрипт выставил морф Blink — превращаем его в короткое моргание (см.
/// `blink_timer` в main), а не в навсегда закрытые глаза.
fn take_blink(character: &mut Character, blink_timer: &mut f32) {
    if character.default_morph.get("Blink") > 0.0 {
        character.default_morph.set("Blink", 0.0);
        *blink_timer = BLINK_SECONDS;
    }
}

/// Имя файла по умолчанию из имени персонажа/анимации: без символов,
/// которые запрещены в именах файлов на Windows.
fn file_stem_for(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    if s.is_empty() {
        "pony".into()
    } else {
        s
    }
}

fn selected_bone_id(character: &pony_core::Character, selected: &Option<String>) -> Option<String> {
    selected.as_ref().and_then(|id| character.parts.get(id)).and_then(|p| p.bone.clone())
}

/// Вставить (или заменить) ключевой кадр позиции по Y для указанной кости
/// в момент `time`. Если дорожки на эту кость ещё нет — создаём её.
/// Ключ ровно на том же времени заменяется, а не дублируется — иначе
/// сэмплирование зависело бы от порядка вставки.
fn insert_keyframe(anim: &mut pony_core::animation::Animation, bone_id: &str, channel: BoneChannel, time: f32, value: f32) {
    let target_matches = |t: &AnimTarget| matches!(t, AnimTarget::Bone { id, channel: c } if id == bone_id && *c == channel);

    let track = match anim.tracks.iter_mut().find(|t| target_matches(&t.target)) {
        Some(t) => t,
        None => {
            anim.tracks.push(Track { target: AnimTarget::Bone { id: bone_id.to_string(), channel }, keyframes: Vec::new() });
            anim.tracks.last_mut().expect("just pushed")
        }
    };

    let kf = Keyframe { time, value: AnimValue::Float(value), interpolation: Interpolation::Linear };
    match track.keyframes.iter().position(|k| (k.time - time).abs() < 1e-4) {
        Some(i) => track.keyframes[i] = kf,
        None => {
            track.keyframes.push(kf);
            // Дорожка должна оставаться отсортированной по времени —
            // интерполяция полагается на это.
            track.keyframes.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        }
    }
    if time > anim.duration {
        anim.duration = time;
    }
}

/// Убрать ключевой кадр (для перетаскивания по времени — старый убираем,
/// новый ставим через `insert_keyframe`, значение сохраняем, а не
/// сбрасываем к текущей позиции кости) — возвращает значение убранного
/// ключа, `None`, если дорожки/ключа с таким временем не нашлось.
fn remove_keyframe(anim: &mut pony_core::animation::Animation, bone_id: &str, channel: BoneChannel, time: f32) -> Option<f32> {
    let target_matches = |t: &AnimTarget| matches!(t, AnimTarget::Bone { id, channel: c } if id == bone_id && *c == channel);
    let track = anim.tracks.iter_mut().find(|t| target_matches(&t.target))?;
    // Совпадение с точностью до полукадра: ключ, поставленный не ровно на
    // границу кадра (например, через Insert > Keyframe в произвольный
    // момент), всё равно рисуется в ячейке своего кадра — и удаляться/
    // перетаскиваться должен по клику на эту ячейку.
    let idx = track.keyframes.iter().position(|k| (k.time - time).abs() < 0.5 / FPS)?;
    let kf = track.keyframes.remove(idx);
    match kf.value {
        AnimValue::Float(v) => Some(v),
        _ => Some(0.0),
    }
}

/// Есть ли уже ключ на этом кадре (с точностью до полукадра) у дорожки
/// канала кости — чтобы перетаскиваемый ключ не затёр чужой.
fn has_keyframe_at(anim: &pony_core::animation::Animation, bone_id: &str, channel: BoneChannel, time: f32) -> bool {
    anim.tracks
        .iter()
        .filter(|t| matches!(&t.target, AnimTarget::Bone { id, channel: c } if id == bone_id && *c == channel))
        .any(|t| t.keyframes.iter().any(|k| (k.time - time).abs() < 0.5 / FPS))
}

/// Текущее значение выбранного канала кости — им затравливается новый
/// ключевой кадр (клик по пустой клетке ставит "как сейчас", не 0).
fn bone_channel_value(bone: &pony_core::skeleton::Bone, channel: BoneChannel) -> f32 {
    match channel {
        BoneChannel::PositionX => bone.local_transform.position.x,
        BoneChannel::PositionY => bone.local_transform.position.y,
        BoneChannel::Rotation => bone.local_transform.rotation,
        BoneChannel::ScaleX => bone.local_transform.scale.x,
        BoneChannel::ScaleY => bone.local_transform.scale.y,
    }
}

/// Раздел 33 ТЗ — таймлайн как линия с делениями и точками-ключами:
/// ```text
/// 0       12       24       36
/// │--------│--------│--------│
/// ●                 ●
/// ```
/// Используется в обзорном режиме "Все": одна такая строка на персонажа
/// сцены, вместо простого пункта списка — сразу видно длительность
/// анимации, плотность ключей и где сейчас плейхед, не открывая полный
/// таймлайн этого персонажа. Обзорная, не подробная: точки — по ВСЕМ
/// дорожкам всех каналов сразу, без разбивки по слоям (для этого есть
/// режим "Персонаж" — полный `timeline_widget`). Клик по строке
/// (возвращённый `Response`) — сделать этого персонажа активным.
fn character_timeline_strip(ui: &mut egui::Ui, character: &Character, player: &AnimationPlayer, is_active: bool) -> egui::Response {
    let height = 30.0;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let painter = ui.painter_at(rect);

    let bg = if is_active {
        egui::Color32::from_rgb(38, 52, 72)
    } else if response.hovered() {
        egui::Color32::from_gray(34)
    } else {
        egui::Color32::from_gray(26)
    };
    painter.rect_filled(rect, 3.0, bg);
    if is_active {
        painter.rect_stroke(rect, 3.0, egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(90, 140, 200)));
    }

    let anim = player.current_name().and_then(|n| character.animations.get(n));
    let duration = anim.map(|a| a.duration).unwrap_or(1.0).max(0.01);
    let total_frames = (duration * FPS).round().max(1.0) as i64;

    let line_y = rect.bottom() - 9.0;
    let line_left = rect.left() + 10.0;
    let line_right = rect.right() - 10.0;
    let span = (line_right - line_left).max(1.0);

    // Деления — 4 промежутка, как в примере из ТЗ (0/12/24/36 — там шаг
    // подобран под конкретную длину; здесь считаем от реальной
    // длительности, чтобы деления оставались осмысленными на любой
    // анимации, а не только на демонстрационной из ТЗ).
    painter.line_segment([egui::pos2(line_left, line_y), egui::pos2(line_right, line_y)], egui::Stroke::new(1.5_f32, egui::Color32::from_gray(130)));
    let tick_every = (total_frames / 4).max(1);
    let mut f = 0;
    while f <= total_frames {
        let x = line_left + (f as f32 / total_frames as f32) * span;
        painter.line_segment([egui::pos2(x, line_y - 3.0), egui::pos2(x, line_y + 3.0)], egui::Stroke::new(1.0_f32, egui::Color32::from_gray(95)));
        f += tick_every;
    }

    if let Some(anim) = anim {
        for track in &anim.tracks {
            for kf in &track.keyframes {
                let frac = (kf.time / duration).clamp(0.0, 1.0);
                let x = line_left + frac * span;
                painter.circle_filled(egui::pos2(x, line_y), 3.0, egui::Color32::from_rgb(230, 190, 90));
                painter.circle_stroke(egui::pos2(x, line_y), 3.0, egui::Stroke::new(0.8_f32, egui::Color32::from_rgb(120, 90, 30)));
            }
        }
    }

    // Плейхед — текущее время воспроизведения этого персонажа (у каждого
    // своё, независимое — см. own AnimationPlayer в CharacterSlot).
    let play_frac = (player.time() / duration).clamp(0.0, 1.0);
    let px = line_left + play_frac * span;
    painter.line_segment([egui::pos2(px, rect.top() + 3.0), egui::pos2(px, line_y + 5.0)], egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(230, 90, 90)));

    response
}

fn channel_label(channel: BoneChannel) -> &'static str {
    match channel {
        BoneChannel::PositionX => "Position X",
        BoneChannel::PositionY => "Position Y",
        BoneChannel::Rotation => "Rotation",
        BoneChannel::ScaleX => "Scale X",
        BoneChannel::ScaleY => "Scale Y",
    }
}

/// Открыть уже сохранённую SVG-часть на редактирование: читает файл,
/// разбирает через `VectorDoc::from_svg_str`, загружает в холст рисования
/// и переключает инструмент на SubSelection — тот же путь для кнопки
/// "Редактировать SVG" что в Properties (для выбранной части), что в
/// панели SVG (для любой части списком, см. `RightTab::SvgInspector`) —
/// одна функция, чтобы поведение не могло разойтись между двумя местами.
/// Возвращает готовое сообщение для статус-бара (успех или ошибка).
fn start_editing_svg_part(
    part_id: &str,
    path: &str,
    vector_doc: &mut pony_core::VectorDoc,
    editing_part: &mut Option<String>,
    active_tool: &mut Tool,
    selected_shape_index: &mut Option<usize>,
) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => match pony_core::VectorDoc::from_svg_str(&text) {
            Ok(doc) => {
                let shape_count = doc.shapes.len();
                // Раздел 29 ТЗ: неизвестные элементы (текст, градиенты как
                // отдельные объекты, use/symbol и т.п. — см. `unsupported`
                // в vector.rs) не должны молча теряться из вида — редактор
                // открывает всё, что смог разобрать, но честно называет,
                // что пропущено, вместо тихой потери данных.
                let unsupported_note = if doc.unsupported.is_empty() {
                    String::new()
                } else {
                    format!(" (пропущены нередактируемые элементы: {})", doc.unsupported.join(", "))
                };
                *vector_doc = doc;
                *editing_part = Some(part_id.to_string());
                *active_tool = Tool::SubSelection;
                *selected_shape_index = None;
                format!("Редактирую '{part_id}' ({path}) — {shape_count} фигур(ы), инструмент SubSel{unsupported_note}")
            }
            Err(err) => format!("Не удалось разобрать SVG '{part_id}' ({path}): {err}"),
        },
        Err(err) => format!("Не удалось прочитать {path}: {err}"),
    }
}

fn color32_to_rgba(c: egui::Color32) -> pony_core::RgbaColor {
    pony_core::RgbaColor::new(c.r(), c.g(), c.b(), c.a())
}

fn rgba_to_color32(c: pony_core::RgbaColor) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// Короткая человекочитаемая метка типа фигуры — для списков выбора в
/// панелях (Boolean/Combine Paths), где нужно показать пользователю, что
/// он вообще выбирает, без полного дампа полей фигуры.
fn shape_kind_label(shape: &pony_core::VectorShape) -> &'static str {
    match shape {
        pony_core::VectorShape::Rect { .. } => "Rect",
        pony_core::VectorShape::Ellipse { .. } => "Ellipse",
        pony_core::VectorShape::Polygon { .. } => "Polygon",
        pony_core::VectorShape::Path { closed, .. } => {
            if *closed {
                "Path (closed)"
            } else {
                "Path (open)"
            }
        }
        pony_core::VectorShape::Line { .. } => "Line",
        pony_core::VectorShape::Polyline { .. } => "Polyline",
        pony_core::VectorShape::Instance { .. } => "Instance",
    }
}

fn boolean_op_label(op: pony_core::BooleanOp) -> &'static str {
    match op {
        pony_core::BooleanOp::Union => "Union",
        pony_core::BooleanOp::Difference => "Difference",
        pony_core::BooleanOp::Intersection => "Intersection",
        pony_core::BooleanOp::Xor => "XOR",
        pony_core::BooleanOp::Divide => "Divide",
    }
}

/// Раздел 60 ТЗ (Gradients): строит функцию "цвет фигуры в точке
/// document-space координат", единую для всех fill-вариантов превью
/// (Rect/Ellipse/Polygon/Path) — либо всегда один и тот же плоский `fill`
/// (обычная заливка), либо честный per-vertex градиент, если `fill_gradient`
/// резолвится в `vector_doc.gradients` (см. `GradientDef::sample`/
/// `gradient_t_at`). Висячая/пустая ссылка — тихий откат на `fill`, тот же
/// принцип, что и у `resolved_fill_paint` в `pony-core` (сериализация в SVG).
fn shape_fill_color_fn<'a>(fill: pony_core::RgbaColor, fill_gradient: Option<&str>, vector_doc: &'a pony_core::VectorDoc) -> impl Fn(f32, f32) -> egui::Color32 + 'a {
    let resolved: Option<pony_core::GradientDef> = fill_gradient.and_then(|name| vector_doc.find_gradient(name)).cloned();
    move |x: f32, y: f32| -> egui::Color32 {
        match &resolved {
            Some(g) => rgba_to_color32(g.sample(pony_core::gradient_t_at(&g.kind, x, y))),
            None => rgba_to_color32(fill),
        }
    }
}

/// Отрисовать одну сохранённую в `VectorDoc` фигуру поверх Stage — экранные
/// координаты вычисляет `world_to_screen`. Хранение фигур — в SVG-системе
/// координат (Y вниз), а весь остальной Stage — в мировой (Y вверх), поэтому
/// здесь Y инвертируется обратно перед вызовом `world_to_screen`. `vector_doc`
/// нужен только для резолва градиентов (`fill_gradient` ссылается на него по
/// имени — раздел 60 ТЗ), фигура сама по себе доступа к документу не имеет.
fn draw_vector_shape_preview(painter: &egui::Painter, shape: &pony_core::VectorShape, vector_doc: &pony_core::VectorDoc, world_to_screen: &impl Fn(glam::Vec2) -> egui::Pos2) {
    use pony_core::VectorShape;
    match shape {
        VectorShape::Rect { x, y, w, h, fill, stroke, stroke_width, fill_gradient } => {
            let color_at = shape_fill_color_fn(*fill, fill_gradient.as_deref(), vector_doc);
            // Раздел 60 ТЗ: заливка через собственный меш с per-vertex
            // цветом на 4 углах — единственный способ показать НАСТОЯЩИЙ
            // (не приближённый одним плоским цветом) градиент в egui,
            // у которого `painter.rect` берёт только один цвет на всю
            // фигуру. Обводка по-прежнему рисуется отдельным `rect_stroke`
            // поверх — Rect не поддерживает градиентную обводку в этой
            // версии (см. `VectorShape::set_fill_gradient` — только fill).
            let corners_doc = [(*x, *y), (x + w, *y), (x + w, y + h), (*x, y + h)];
            let corners_screen: Vec<egui::Pos2> = corners_doc.iter().map(|(cx, cy)| world_to_screen(glam::Vec2::new(*cx, -*cy))).collect();
            let colors: Vec<egui::Color32> = corners_doc.iter().map(|(cx, cy)| color_at(*cx, *cy)).collect();
            if let Some(mesh) = triangulate_fill(&corners_screen, &colors) {
                painter.add(egui::Shape::mesh(mesh));
            }
            let rect = egui::Rect::from_two_pos(corners_screen[0], corners_screen[2]);
            painter.rect_stroke(rect, 0.0, egui::Stroke::new(*stroke_width, rgba_to_color32(*stroke)));
        }
        VectorShape::Ellipse { cx, cy, rx, ry, fill, stroke, stroke_width, fill_gradient } => {
            // Превью аппроксимирует эллипс правильным N-угольником — точнее,
            // чем прежний "рисуем круг радиуса rx" (который искажал форму
            // при rx != ry) и единственный практичный способ дать per-vertex
            // градиент по кругу в egui (нет отдельной команды "залитый
            // эллипс с градиентом одной командой"). Сохранённый .svg честно
            // хранит rx/ry и рендерится точным эллипсом через resvg — это
            // ограничение только Stage-оверлея.
            const SEGMENTS: usize = 32;
            let color_at = shape_fill_color_fn(*fill, fill_gradient.as_deref(), vector_doc);
            let mut ring_doc: Vec<(f32, f32)> = Vec::with_capacity(SEGMENTS);
            for i in 0..SEGMENTS {
                let theta = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                ring_doc.push((cx + rx * theta.cos(), cy + ry * theta.sin()));
            }
            let ring_screen: Vec<egui::Pos2> = ring_doc.iter().map(|(px, py)| world_to_screen(glam::Vec2::new(*px, -*py))).collect();
            let ring_colors: Vec<egui::Color32> = ring_doc.iter().map(|(px, py)| color_at(*px, *py)).collect();
            let center_screen = world_to_screen(glam::Vec2::new(*cx, -*cy));
            let center_color = color_at(*cx, *cy);
            let mut mesh = egui::epaint::Mesh::default();
            mesh.vertices.push(egui::epaint::Vertex { pos: center_screen, uv: egui::Pos2::ZERO, color: center_color });
            for (p, c) in ring_screen.iter().zip(ring_colors.iter()) {
                mesh.vertices.push(egui::epaint::Vertex { pos: *p, uv: egui::Pos2::ZERO, color: *c });
            }
            for i in 0..SEGMENTS as u32 {
                let a = 1 + i;
                let b = 1 + (i + 1) % SEGMENTS as u32;
                mesh.indices.extend_from_slice(&[0, a, b]);
            }
            painter.add(egui::Shape::mesh(mesh));
            painter.add(egui::Shape::closed_line(ring_screen, egui::Stroke::new(*stroke_width, rgba_to_color32(*stroke))));
        }
        VectorShape::Line { x1, y1, x2, y2, stroke, stroke_width } => {
            let p0 = world_to_screen(glam::Vec2::new(*x1, -*y1));
            let p1 = world_to_screen(glam::Vec2::new(*x2, -*y2));
            painter.line_segment([p0, p1], egui::Stroke::new(*stroke_width, rgba_to_color32(*stroke)));
        }
        VectorShape::Polyline { points, stroke, stroke_width } => {
            let screen_pts: Vec<egui::Pos2> = points.iter().map(|(x, y)| world_to_screen(glam::Vec2::new(*x, -*y))).collect();
            painter.add(egui::Shape::line(screen_pts, egui::Stroke::new(*stroke_width, rgba_to_color32(*stroke))));
        }
        VectorShape::Polygon { points, fill, stroke, stroke_width, fill_gradient } => {
            // Честная оговорка: собственная ear-clipping триангуляция (см.
            // `triangulate_fill`) корректно рисует любой ПРОСТОЙ (без
            // самопересечений) многоугольник, выпуклый или нет — раньше
            // здесь был `egui::Shape::convex_polygon`, который для невыпуклых
            // фигур мог исказить превью; теперь используется тот же путь,
            // что и Path ниже, заодно даёт градиентную заливку бесплатно.
            // Сохранённый .svg при этом всегда рисуется правильно — это
            // ограничение затрагивало только Stage-оверлей.
            let color_at = shape_fill_color_fn(*fill, fill_gradient.as_deref(), vector_doc);
            let screen_pts: Vec<egui::Pos2> = points.iter().map(|(x, y)| world_to_screen(glam::Vec2::new(*x, -*y))).collect();
            let colors: Vec<egui::Color32> = points.iter().map(|(x, y)| color_at(*x, *y)).collect();
            if let Some(mesh) = triangulate_fill(&screen_pts, &colors) {
                painter.add(egui::Shape::mesh(mesh));
            }
            painter.add(egui::Shape::closed_line(screen_pts, egui::Stroke::new(*stroke_width, rgba_to_color32(*stroke))));
        }
        VectorShape::Path { nodes, closed, fill, stroke, stroke_width, fill_gradient, extra_contours } => {
            // Превью тесселирует каждый сегмент (прямой или кубическую
            // Безье) в ломаную из фиксированного числа отрезков — только
            // для оверлея на Stage. Сам сохранённый .svg хранит настоящие
            // C/L-команды и рендерится точной кривой через resvg, эта
            // тесселяция никак на него не влияет.
            //
            // Заливка: для замкнутого path с >=3 точками тесселированный
            // контур триангулируется ear-clipping'ом (работает для любого
            // ПРОСТОГО многоугольника — выпуклого или вогнутого, без
            // самопересечений; ровно так же, как это делает resvg при
            // финальном рендере). У путей с самопересечениями (редкость на
            // практике — руками нарисовать сложно) заливка может визуально
            // отличаться от финального .svg, но отсутствие заливки вообще
            // (как было раньше) вводило в заблуждение сильнее в абсолютном
            // большинстве реальных файлов — оба демо-ассета (tail/wing)
            // используют обычные простые контуры. Раздел 60 ТЗ: каждая
            // вершина тесселяции несёт СВОЙ document-space цвет — и прямой,
            // и кривой сегмент дают достаточно плотную выборку вдоль
            // контура, чтобы градиент выглядел гладким, не гранёным.
            //
            // Дополнительные контуры (`extra_contours`, `M…Z M…Z` в одном
            // `d`): отдельный кусок заливается, а дырка (контур внутри
            // основного) в превью только обводится — egui-меш не умеет
            // вычитать. В сохранённом .svg (resvg) дырка настоящая.
            const SEGMENTS: usize = 16;
            let tessellate = |nodes: &[pony_core::PathNode], closed: bool| -> (Vec<egui::Pos2>, Vec<(f32, f32)>) {
                let mut screen_pts: Vec<egui::Pos2> = Vec::new();
                let mut doc_pts: Vec<(f32, f32)> = Vec::new();
                if !nodes.is_empty() {
                    screen_pts.push(world_to_screen(glam::Vec2::new(nodes[0].position.0, -nodes[0].position.1)));
                    doc_pts.push(nodes[0].position);
                    for i in 1..nodes.len() {
                        tessellate_segment(&nodes[i - 1], &nodes[i], SEGMENTS, world_to_screen, &mut screen_pts, &mut doc_pts);
                    }
                    if closed && nodes.len() > 1 {
                        tessellate_segment(&nodes[nodes.len() - 1], &nodes[0], SEGMENTS, world_to_screen, &mut screen_pts, &mut doc_pts);
                    }
                }
                (screen_pts, doc_pts)
            };
            let color_at = shape_fill_color_fn(*fill, fill_gradient.as_deref(), vector_doc);
            let fill_contour = |screen_pts: &[egui::Pos2], doc_pts: &[(f32, f32)]| {
                if fill.a > 0 && screen_pts.len() >= 3 {
                    let colors: Vec<egui::Color32> = doc_pts.iter().map(|(x, y)| color_at(*x, *y)).collect();
                    if let Some(mesh) = triangulate_fill(screen_pts, &colors) {
                        painter.add(egui::Shape::mesh(mesh));
                    }
                }
            };
            let (main_screen, main_doc) = tessellate(nodes, *closed);
            if *closed {
                fill_contour(&main_screen, &main_doc);
            }
            let stroke_style = egui::Stroke::new(*stroke_width, rgba_to_color32(*stroke));
            for contour in extra_contours {
                let (screen_pts, doc_pts) = tessellate(&contour.nodes, contour.closed);
                let is_hole = *closed && doc_pts.first().is_some_and(|p| point_in_polygon(*p, &main_doc));
                if contour.closed && !is_hole {
                    fill_contour(&screen_pts, &doc_pts);
                }
                painter.add(egui::Shape::line(screen_pts, stroke_style));
            }
            painter.add(egui::Shape::line(main_screen, stroke_style));
        }
        VectorShape::Instance { .. } => {
            // Никогда не должен дойти сюда: вызывающая сторона (см. цикл
            // отрисовки `vector_doc.shapes` на Stage) резолвит Instance в
            // конкретные фигуры символа ДО вызова этой функции — Instance
            // сам по себе не знает, как выглядит геометрия, на которую
            // ссылается, у него нет доступа к `VectorDoc::symbols` здесь.
            debug_assert!(false, "draw_vector_shape_preview получил Instance напрямую — вызывающая сторона должна была резолвить его через resolve_symbol_instance");
        }
    }
}

/// Точка внутри многоугольника (чётность пересечений луча).
fn point_in_polygon(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}

/// Разбить один сегмент path (`from` -> `to`, кривая если у концов есть
/// ручки, иначе прямая) на `SEGMENTS` отрезков через кубическую формулу
/// Безье, добавляя экранные точки в `out_screen` (первая точка сегмента уже
/// есть в обоих выходных векторах от предыдущего вызова — добавляем только
/// последующие). `out_doc` — параллельный вектор ТЕХ ЖЕ точек в исходных
/// document-space координатах (до Y-флипа/`world_to_screen`) — нужен раздел
/// 60 ТЗ (Gradients): сэмплирование градиента идёт в document space,
/// экранные координаты для этого не годятся (Stage может быть повёрнут/
/// отмасштабирован/запанорамлен независимо от геометрии фигуры).
fn tessellate_segment(
    from: &pony_core::PathNode,
    to: &pony_core::PathNode,
    segments: usize,
    world_to_screen: &impl Fn(glam::Vec2) -> egui::Pos2,
    out_screen: &mut Vec<egui::Pos2>,
    out_doc: &mut Vec<(f32, f32)>,
) {
    let p0 = from.position;
    let p3 = to.position;
    match (from.out_handle, to.in_handle) {
        (None, None) => {
            out_screen.push(world_to_screen(glam::Vec2::new(p3.0, -p3.1)));
            out_doc.push(p3);
        }
        (out_h, in_h) => {
            let p1 = out_h.unwrap_or(p0);
            let p2 = in_h.unwrap_or(p3);
            for s in 1..=segments {
                let t = s as f32 / segments as f32;
                let mt = 1.0 - t;
                let x = mt * mt * mt * p0.0 + 3.0 * mt * mt * t * p1.0 + 3.0 * mt * t * t * p2.0 + t * t * t * p3.0;
                let y = mt * mt * mt * p0.1 + 3.0 * mt * mt * t * p1.1 + 3.0 * mt * t * t * p2.1 + t * t * t * p3.1;
                out_screen.push(world_to_screen(glam::Vec2::new(x, -y)));
                out_doc.push((x, y));
            }
        }
    }
}

/// Триангулирует замкнутый простой (без самопересечений) многоугольник
/// методом ear clipping и возвращает готовый `egui::epaint::Mesh` для
/// заливки — используется превью Rect/Polygon/Path на Stage (см.
/// `draw_vector_shape_preview`). `colors[i]` — цвет вершины `points[i]`
/// (та же длина, что и `points`) — раздел 60 ТЗ: per-vertex цвет, не один
/// общий, чтобы градиентная заливка не сводилась к плоскому приближению.
/// Возвращает `None`, если после дедупликации соседних совпадающих точек
/// вершин осталось меньше 3 (вырожденный контур — рисовать нечего).
fn triangulate_fill(points: &[egui::Pos2], colors: &[egui::Color32]) -> Option<egui::epaint::Mesh> {
    debug_assert_eq!(points.len(), colors.len(), "triangulate_fill: points и colors должны быть одной длины");
    // Убираем повторяющуюся последнюю точку, если тесселяция дала её
    // совпадающей с первой (замкнутый контур), и вообще любые соседние
    // дубликаты — ear clipping ломается на нулевых по площади треугольниках.
    let mut pts: Vec<egui::Pos2> = Vec::with_capacity(points.len());
    let mut cols: Vec<egui::Color32> = Vec::with_capacity(points.len());
    for (i, &p) in points.iter().enumerate() {
        if pts.last().map_or(true, |last: &egui::Pos2| (*last - p).length_sq() > 1e-8) {
            pts.push(p);
            cols.push(colors.get(i).copied().unwrap_or(egui::Color32::WHITE));
        }
    }
    if pts.len() >= 2 && (pts[0] - *pts.last().unwrap()).length_sq() <= 1e-8 {
        pts.pop();
        cols.pop();
    }
    if pts.len() < 3 {
        return None;
    }

    // Знаковая площадь определяет обход (CW/CCW) — ear clipping ниже
    // проверяет "выпуклость уха" через знак векторного произведения и
    // должен знать ориентацию контура, иначе выберет уши с обратной стороны.
    let signed_area: f32 = {
        let mut sum = 0.0;
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            sum += a.x * b.y - b.x * a.y;
        }
        sum * 0.5
    };
    let ccw = signed_area > 0.0;

    let cross = |o: egui::Pos2, a: egui::Pos2, b: egui::Pos2| -> f32 { (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x) };

    let point_in_triangle = |p: egui::Pos2, a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| -> bool {
        let d1 = cross(a, b, p);
        let d2 = cross(b, c, p);
        let d3 = cross(c, a, p);
        let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(has_neg && has_pos)
    };

    let mut indices: Vec<u32> = (0..pts.len() as u32).collect();
    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(pts.len().saturating_sub(2));
    // Защита от бесконечного цикла на вырожденных/самопересекающихся
    // контурах, где ни одно "ухо" не находится: ограничиваем число попыток
    // и, если триангуляция застряла, отдаём то, что успели собрать (лучше
    // частичная заливка, чем зависание кадра).
    let mut guard = indices.len() * indices.len() + 8;
    while indices.len() > 3 && guard > 0 {
        guard -= 1;
        let n = indices.len();
        let mut ear_found = false;
        for i in 0..n {
            let i_prev = (i + n - 1) % n;
            let i_next = (i + 1) % n;
            let a = pts[indices[i_prev] as usize];
            let b = pts[indices[i] as usize];
            let c = pts[indices[i_next] as usize];
            let cr = cross(a, b, c);
            let is_convex = if ccw { cr > 0.0 } else { cr < 0.0 };
            if !is_convex {
                continue;
            }
            let mut any_inside = false;
            for &idx in &indices {
                if idx == indices[i_prev] || idx == indices[i] || idx == indices[i_next] {
                    continue;
                }
                if point_in_triangle(pts[idx as usize], a, b, c) {
                    any_inside = true;
                    break;
                }
            }
            if any_inside {
                continue;
            }
            triangles.push([indices[i_prev], indices[i], indices[i_next]]);
            indices.remove(i);
            ear_found = true;
            break;
        }
        if !ear_found {
            // Не нашли валидное ухо (самопересекающийся контур) — обрываем
            // триангуляцию тем, что уже накопили, вместо зависания.
            break;
        }
    }
    if indices.len() == 3 {
        triangles.push([indices[0], indices[1], indices[2]]);
    }
    if triangles.is_empty() {
        return None;
    }

    let mut mesh = egui::epaint::Mesh::default();
    for (i, &p) in pts.iter().enumerate() {
        mesh.vertices.push(egui::epaint::Vertex { pos: p, uv: egui::Pos2::new(0.0, 0.0), color: cols[i] });
    }
    for tri in triangles {
        mesh.indices.push(tri[0]);
        mesh.indices.push(tri[1]);
        mesh.indices.push(tri[2]);
    }
    Some(mesh)
}

const EXPORT_FPS: f32 = 24.0;

/// Проигрывает анимацию `anim_name` с нуля до конца на отдельном клоне
/// персонажа/плеера (не трогает то, что сейчас реально показано на Stage)
/// и возвращает отрендеренные кадры — общая часть для GIF и спрайт-листа.
fn render_animation_frames(
    renderer: &mut Renderer,
    ctx: &GpuContext,
    character: &pony_core::Character,
    anim_name: Option<&str>,
    camera: &pony_core::Camera,
    lighting: &pony_core::Lighting,
) -> Result<Vec<pony_render::FrameOutput>, String> {
    let name = anim_name.ok_or_else(|| "нет активной анимации — сначала запусти воспроизведение".to_string())?;
    let anim = character.animations.get(name).ok_or_else(|| format!("анимация '{name}' не найдена"))?;
    let total_frames = (anim.duration * EXPORT_FPS).round().max(1.0) as usize;
    let dt = 1.0 / EXPORT_FPS;

    let mut temp_character = character.clone();
    let mut temp_player = pony_core::AnimationPlayer::new();
    temp_player.play(name);

    let mut frames = Vec::with_capacity(total_frames);
    for _ in 0..total_frames {
        temp_player.apply(&mut temp_character);
        let frame = renderer.render_character(ctx, &temp_character, SCENE_WIDTH, SCENE_HEIGHT, camera, temp_player.time(), lighting, None);
        frames.push(frame);
        temp_player.advance(&temp_character, dt);
    }
    Ok(frames)
}

fn export_animation_gif(
    renderer: &mut Renderer,
    ctx: &GpuContext,
    character: &pony_core::Character,
    anim_name: Option<&str>,
    camera: &pony_core::Camera,
    lighting: &pony_core::Lighting,
    path: &str,
) -> String {
    let frames = match render_animation_frames(renderer, ctx, character, anim_name, camera, lighting) {
        Ok(f) => f,
        Err(msg) => return format!("Экспорт GIF не выполнен: {msg}"),
    };
    let delay_cs = (100.0 / EXPORT_FPS).round() as u16;
    match export_gif(path, &frames, delay_cs) {
        Ok(()) => format!("GIF сохранён: {path} ({} кадров)", frames.len()),
        Err(err) => format!("Ошибка экспорта GIF: {err}"),
    }
}

fn export_animation_spritesheet(
    renderer: &mut Renderer,
    ctx: &GpuContext,
    character: &pony_core::Character,
    anim_name: Option<&str>,
    camera: &pony_core::Camera,
    lighting: &pony_core::Lighting,
    path: &str,
) -> String {
    let frames = match render_animation_frames(renderer, ctx, character, anim_name, camera, lighting) {
        Ok(f) => f,
        Err(msg) => return format!("Экспорт спрайт-листа не выполнен: {msg}"),
    };
    match export_spritesheet(path, &frames, 6) {
        Ok(layout) => format!("Спрайт-лист сохранён: {path} ({}x{} кадров)", layout.columns, layout.rows),
        Err(err) => format!("Ошибка экспорта спрайт-листа: {err}"),
    }
}

fn main() {
    if let Some(root) = find_workspace_root() {
        if std::env::set_current_dir(&root).is_ok() {
            eprintln!("pony-gui: рабочая папка -> {} (там лежат assets/)", root.display());
        }
    }

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let window = WindowBuilder::new()
        .with_title("pony-engine")
        .with_inner_size(winit::dpi::LogicalSize::new(1200.0, 780.0))
        .build(&event_loop)
        .expect("failed to create window");

    let window = std::sync::Arc::new(window);

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
    let surface = instance.create_surface(window.clone()).expect("failed to create surface");
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))
    .expect("no compatible GPU adapter found for this surface");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
        .expect("failed to create wgpu device");

    let adapter_info = adapter.get_info();
    let surface_caps = surface.get_capabilities(&adapter);
    let surface_format = surface_caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(surface_caps.formats[0]);
    let size = window.inner_size();
    let mut surface_config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: surface_caps.present_modes[0],
        alpha_mode: surface_caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };

    let gpu_ctx = GpuContext {
        info: GpuAdapterInfo {
            index: 0,
            name: adapter_info.name.clone(),
            backend: format!("{:?}", adapter_info.backend),
            device_type: GpuDeviceType::from(adapter_info.device_type),
            vendor: adapter_info.vendor,
            device: adapter_info.device,
        },
        weight: 1.0,
        device,
        queue,
    };
    surface.configure(&gpu_ctx.device, &surface_config);
    // Бюджет памяти под кэш текстур берём из реального профиля системы
    // (pony-system считает его от фактически доступной памяти), а не из
    // захардкоженной константы по умолчанию.
    //
    // ВАЖНО: именно `detect_without_gpus()`, а не `detect()`. Полный
    // `detect()` создаёт второй `wgpu::Instance` и перечисляет все
    // бэкенды — а здесь Instance/Device/Surface уже созданы выше, и на
    // Windows это роняло приложение при старте с STATUS_ACCESS_VIOLATION
    // (0xC0000005). Бюджет считается только из доступной памяти, GPU для
    // него не нужны — см. WorkloadPolicy::from_profile.
    // Бюджет считается от ОПЕРАТИВНОЙ памяти, а текстуры живут в видеопамяти:
    // 75% от 32 ГБ ОЗУ при 4 ГБ на видеокарте — кэш никогда бы не вытеснялся.
    // Потолок 1 ГиБ — с запасом для десятков импортированных паппетов.
    let texture_budget = pony_system::WorkloadPolicy::from_profile(&pony_system::SystemProfile::detect_without_gpus()).memory_budget_bytes.min(1 << 30);
    let mut pony_renderer = Renderer::new_with_budget(&gpu_ctx, texture_budget);

    let egui_ctx = egui::Context::default();
    let mut egui_state = egui_winit::State::new(egui_ctx.clone(), egui::ViewportId::ROOT, &window, None, None);
    let mut egui_renderer = egui_wgpu::Renderer::new(&gpu_ctx.device, surface_format, None, 1);

    let mut character = empty_character();
    let mut player = AnimationPlayer::new();

    // --- сцена из нескольких персонажей ---
    // Намеренно НЕ переписываю все существующие обращения к `character`/
    // `player` на структуру со множеством персонажей — их сотни по всему
    // файлу, и слепая замена — верный способ внести десятки тонких багов.
    // Вместо этого: `character`/`player` остаются АКТИВНЫМ персонажем (как
    // и раньше, весь существующий код работает без изменений), а
    // ОСТАЛЬНЫЕ персонажи сцены лежат в `other_characters`. Переключение
    // активного — явный своп: текущее состояние уходит в список, оттуда же
    // достаётся новое. Дороже, чем прямая ссылка, зато не рискует
    // сломать то, что уже работает.
    struct CharacterSlot {
        character: Character,
        player: AnimationPlayer,
        name: String,
    }
    let mut other_characters: Vec<CharacterSlot> = Vec::new();
    let mut active_character_name = String::from("Pony 1");
    let mut new_character_count: u32 = 1;
    // Режимы таймлайна: "Character" — как и раньше, тайлайн активного
    // персонажа (части/кости/ключи); "All" — обзорный, один ряд на
    // персонажа сцены, клик по ряду делает его активным (drill-down).
    let mut timeline_mode_all = false;

    let mut camera = pony_core::Camera::default();
    let script_engine = pony_script::ScriptEngine::new();
    let mut script_text = String::from("// Пример — жми \"Выполнить\"\npony.Smile(0.8);\npony.Blink();\ncamera.Zoom(1.2);");
    let mut script_log = String::new();
    // На паузе при старте, как в любом редакторе анимации: пока анимация
    // играет, плеер каждый кадр переписывает анимируемые каналы костей, и
    // правка, скажем, позиции головы тут же "отскакивала" обратно.
    let mut playing = false;
    let mut scene_texture: Option<egui::TextureHandle> = None;
    let mut last_frame = Instant::now();
    let mut elapsed_time: f32 = 0.0;
    let mut hidden_layers: HashSet<String> = HashSet::new();
    let mut locked_layers: HashSet<String> = HashSet::new();
    let mut selected_layer: Option<String> = None;
    let mut active_tool = Tool::Selection;
    let mut right_tab = RightTab::Properties;
    // 0.5, не 1.0 — SCENE_WIDTH/HEIGHT удвоены (см. выше), а экранный
    // размер Stage = SCENE_WIDTH*stage_zoom; без компенсации персонаж по
    // умолчанию занимал бы вдвое больше места на экране, чем раньше.
    let mut stage_zoom: f32 = 0.5;
    let mut stage_pan = egui::Vec2::ZERO;
    let mut mouse_stage_pos: Option<egui::Pos2> = None;
    let mut fill_color = egui::Color32::from_rgb(222, 150, 195);
    let mut stroke_color = egui::Color32::from_rgb(90, 60, 70);
    let mut show_timeline = true;
    let mut show_right_panel = true;
    let mut status_message = String::from("Готово");

    // --- реально работающее освещение (раздел 12 ТЗ) ---
    let mut lighting = pony_core::Lighting::default();
    let mut sun_enabled = false;
    let mut sun_color = egui::Color32::from_rgb(255, 230, 180);
    let mut sun_intensity: f32 = 0.3;
    let mut point_enabled = false;
    let mut point_pos = egui::Vec2::ZERO;
    let mut point_color = egui::Color32::from_rgb(255, 140, 40);
    let mut point_intensity: f32 = 1.5;
    let mut point_radius: f32 = 120.0;

    // --- частицы (раздел 13 ТЗ) ---
    let mut particles_enabled = false;
    let mut particle_kind = pony_core::ParticleKind::Snow;
    let mut particle_emitter = pony_core::ParticleEmitter::new(particle_kind, glam::Vec2::new(0.0, 90.0), 8.0);


    // --- рисование/редактирование SVG прямо на Stage (раздел 16 ТЗ) ---
    let mut vector_doc = pony_core::VectorDoc::new();
    let mut history = History::new(&character, &vector_doc);
    let mut saved_revision: u64 = 0;
    let mut window_title = String::new();
    // Начало текущего перетаскивания (мировые координаты) для Rect/Oval/Line —
    // фигура коммитится в vector_doc только когда drag реально закончился.
    let mut draw_start: Option<glam::Vec2> = None;
    // Точки текущего свободного мазка (Pencil/Brush) — копятся все кадры
    // одного drag'а, коммитятся одной Polyline в конце.
    let mut pencil_points: Vec<(f32, f32)> = Vec::new();
    let mut svg_save_count: u32 = 0;
    // Импорт ассетов из GUI (см. меню File > Import asset as layer).
    let mut import_count: u32 = 0;
    // Путь текущего документа (.asset); пусто — документ ещё не сохранён,
    // и Save откроет проводник (Save As).
    let mut asset_path = String::new();
    let mut file_dialog = file_dialog::FileDialog::default();
    // Выбранный в проводнике .asset, который ещё предстоит загрузить.
    let mut pending_open = String::new();
    // Очередь действий из меню/тулбара/горячих клавиш — см. `Action`.
    let mut actions: Vec<Action> = Vec::new();
    // Запрос подогнать вид Stage — выполняется в CentralPanel, где
    // известен размер холста. При старте — крупно на персонажа (раньше
    // он занимал ~30px посреди огромного голубого поля).
    let mut fit_request: Option<FitView> = Some(FitView::Character);
    // --- импорт Adobe Animate (.fla/.xfl): окно с выбором символа и кадра ---
    let mut xfl_window_open = false;
    let mut xfl_path = String::new();
    let mut xfl_doc: Option<pony_core::xfl::XflDocument> = None;
    let mut xfl_symbol: Option<String> = None; // None — главная шкала документа
    let mut xfl_frame: usize = 0;
    let mut xfl_filter = String::new();
    let mut xfl_opts = pony_core::xfl::ImportOptions { scale: 1.0, expand_containers: true };
    let mut xfl_preview: Option<egui::TextureHandle> = None;
    let mut xfl_preview_key: Option<(Option<String>, usize)> = None;
    let mut xfl_info = String::new();
    // Загрузка документа отложена на конец кадра (может занять секунду на
    // больших .fla) — запрос ставят кнопка "Загрузить" и File > Open.
    let mut xfl_load_request = false;
    let mut blank_layer_count: u32 = 0;
    let mut show_rulers = true;

    // --- Pen (клик-по-точке путь, в отличие от Pencil, который тянут) и
    // PolyStar (drag центр+радиус -> правильный n-угольник) ---
    let mut pen_points: Vec<(f32, f32)> = Vec::new();
    let mut poly_sides: u32 = 6;

    // --- редактирование скелета (кости) ---
    let mut selected_bone: Option<String> = Some("Body".to_string());
    let mut placing_bone = false;
    let mut show_skeleton = true;
    let mut bone_rename_buf = String::new();
    let mut bone_count: u32 = 0;

    // --- IK (раздел 41 ТЗ): состояние формы создания нового констрейнта
    // и отложенное удаление (нельзя мутировать character.skeleton.
    // ik_constraints, пока по нему же идёт итерация в блоке ui выше). ---
    let mut ik_new_root: Option<String> = None;
    let mut ik_new_mid: Option<String> = None;
    let mut ik_new_tip: Option<String> = None;
    let mut ik_to_remove: Option<String> = None;

    // --- SubSelection: редактирование точек уже нарисованной (ещё не
    // сохранённой как часть) фигуры на холсте ---
    let mut selected_shape_index: Option<usize> = None;
    let mut dragging_point_index: Option<usize> = None;

    // --- Symbols (раздел 28 + 95 ТЗ): имя для нового Symbol Definition при
    // "Convert to Symbol" — отдельный текстовый буфер ввода на вкладке
    // Symbols, тот же приём, что и `bone_rename_buf` у костей. ---
    let mut symbol_name_buf = String::new();

    // --- Gradients (раздел 60 ТЗ): форма редактирования одного
    // GradientDef за раз — те же поля, что у GradientDef/GradientKind в
    // pony-core, плюс `gradient_editing_existing`, которое отличает "новый
    // градиент" (None — "Сохранить" создаёт новую запись) от "правим уже
    // существующий" (Some(name) — "Сохранить" делает upsert под тем же
    // именем, обновляя все фигуры, которые на него ссылаются). Кнопка
    // "Загрузить в форму" на карточке в списке заполняет эти поля из
    // выбранного GradientDef, тем же приёмом, что redo/undo не нужен —
    // сама форма не мутирует документ, пока не нажат "Сохранить".
    let mut gradient_editing_existing: Option<String> = None;
    let mut gradient_name_buf = String::new();
    let mut gradient_is_radial = false;
    let mut gradient_lin_x1: f32 = 0.0;
    let mut gradient_lin_y1: f32 = 0.0;
    let mut gradient_lin_x2: f32 = 100.0;
    let mut gradient_lin_y2: f32 = 0.0;
    let mut gradient_rad_cx: f32 = 50.0;
    let mut gradient_rad_cy: f32 = 50.0;
    let mut gradient_rad_r: f32 = 50.0;
    let mut gradient_stops: Vec<pony_core::GradientStop> = vec![
        pony_core::GradientStop { offset: 0.0, color: pony_core::RgbaColor::new(255, 255, 255, 255) },
        pony_core::GradientStop { offset: 1.0, color: pony_core::RgbaColor::new(0, 0, 0, 255) },
    ];

    // --- Boolean / Combine Paths (раздел 44 ТЗ, меню Modify): индексы
    // выбранных "верхней"/"нижней" фигур в vector_doc.shapes и текущая
    // операция — форма на вкладке Boolean, тот же приём, что и у формы
    // Gradients выше (панель не мутирует документ, пока не нажата кнопка
    // "Выполнить"). ---
    let mut boolean_front_idx: Option<usize> = None;
    let mut boolean_back_idx: Option<usize> = None;
    let mut boolean_op_kind = pony_core::BooleanOp::Union;

    // --- Lasso: рамкой выделить несколько частей персонажа сразу, для
    // групповых операций (скрыть/показать/удалить группой) ---
    let mut lasso_start: Option<glam::Vec2> = None;
    let mut multi_selected: std::collections::HashSet<String> = std::collections::HashSet::new();

    // --- редактирование уже созданной SVG-части (не только рисование с
    // нуля) — id части, чей .svg сейчас загружен в vector_doc на
    // редактирование; при сохранении обновляет ЕЁ, а не создаёт новую.
    let mut editing_part: Option<String> = None;

    // --- таймлайн: зум, выбранный ключевой кадр (для перетаскивания/Delete),
    // и какой канал кости сейчас редактируется в сетке (PositionX/Y,
    // Rotation, ScaleX/Y) — раньше был жёстко зашит на PositionY, из-за
    // чего анимировать поворот или горизонтальное движение было вообще
    // нельзя было через таймлайн, хотя модель данных это поддерживала.
    let mut timeline_zoom: f32 = 1.0;
    let mut selected_keyframe: Option<(String, i64)> = None;
    let mut timeline_channel: BoneChannel = BoneChannel::PositionY;

    // --- рендер Stage: кадр рендерится в разрешении ЭКРАНА, а не
    // фиксированного документа — поэтому персонаж остаётся чётким на
    // любом зуме Stage (раньше 960x720-картинка растягивалась, и при
    // увеличении всё размывалось). Перерисовывается только когда
    // что-то изменилось (см. `RenderKey`).
    let mut last_render_key: Option<RenderKey> = None;
    let mut last_frame_output: Option<pony_render::FrameOutput> = None;
    // Экранный прямоугольник (в точках egui), который покрывает
    // `scene_texture`, и масштаб точки->пиксель — для пипетки.
    let mut scene_image_rect = egui::Rect::NOTHING;
    let mut force_render = false;
    // Предыдущий кадр: был ли повод перерисоваться ещё раз (анимация,
    // частицы, тряска, изменение документа) — иначе цикл спит (Wait).
    let mut needs_redraw = true;
    // Моргание — короткое действие, а не состояние: `pony.Blink()` ставит
    // таймер, глаз закрывается и открывается за BLINK_SECONDS. В самого
    // персонажа это не пишется (иначе каждый кадр моргания был бы шагом
    // Undo, а глаза после скрипта оставались закрытыми навсегда).
    let mut blink_timer: f32 = 0.0;
    // Selection: смещение от точки захвата до центра части — чтобы часть
    // ехала за мышью с того места, за которое её взяли, а не прыгала центром.
    let mut grab_offset: Option<glam::Vec2> = None;

    let autoexit_at = std::env::var("PONY_GUI_AUTOEXIT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|ms| Instant::now() + Duration::from_millis(ms));

    event_loop
        .run(move |event, elwt| {
            if let Some(deadline) = autoexit_at {
                if Instant::now() >= deadline {
                    elwt.exit();
                    return;
                }
            }

            match event {
                Event::WindowEvent { window_id, event } if window_id == window.id() => {
                    let response = egui_state.on_window_event(&window, &event);
                    // RedrawRequested egui-winit тоже помечает repaint=true — на него
                    // не отвечаем новым запросом, иначе цикл никогда не уснёт.
                    if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
                        window.request_redraw();
                    }
                    if response.consumed {
                        return;
                    }
                    match event {
                        WindowEvent::CloseRequested => elwt.exit(),
                        WindowEvent::Resized(new_size) => {
                            surface_config.width = new_size.width.max(1);
                            surface_config.height = new_size.height.max(1);
                            surface.configure(&gpu_ctx.device, &surface_config);
                            window.request_redraw();
                        }
                        WindowEvent::RedrawRequested => {
                            // dt зажат: цикл теперь спит, пока ничего не
                            // происходит, и первый кадр после простоя иначе
                            // получил бы dt в несколько секунд — анимация
                            // прыгала бы вперёд при нажатии Play.
                            let dt = last_frame.elapsed().as_secs_f32().min(0.1);
                            last_frame = Instant::now();
                            elapsed_time += dt;
                            if playing {
                                player.advance(&character, dt);
                                player.apply(&mut character);
                                // Незацикленная анимация доиграла — останавливаемся
                                // сами, а не крутим пустые кадры.
                                if player.is_finished(&character) {
                                    playing = false;
                                }
                            }
                            // Тряска затухает со временем — сама Camera не хранит
                            // часы и не решает это за нас (см. camera.rs), поэтому
                            // затухание — здесь, в игровом цикле GUI.
                            camera.shake_intensity = (camera.shake_intensity - dt * 0.6).max(0.0);
                            if blink_timer > 0.0 {
                                blink_timer = (blink_timer - dt).max(0.0);
                                // Последний кадр моргания: персонаж не менялся, и без
                                // принудительной перерисовки на экране оставался
                                // полузакрытый глаз.
                                if blink_timer == 0.0 {
                                    force_render = true;
                                }
                            }

                            // Освещение реально пересчитывается из состояния UI-переключателей
                            // каждый кадр — не заглушка Lighting::default().
                            lighting.sun = if sun_enabled {
                                Some(pony_core::SunLight { color: color32_to_rgb(sun_color), intensity: sun_intensity })
                            } else {
                                None
                            };
                            lighting.points = if point_enabled {
                                vec![pony_core::PointLight {
                                    position: glam::Vec2::new(point_pos.x, point_pos.y),
                                    color: color32_to_rgb(point_color),
                                    intensity: point_intensity,
                                    radius: point_radius,
                                }]
                            } else {
                                Vec::new()
                            };

                            if particles_enabled {
                                particle_emitter.update(dt);
                            }

                            let raw_input = egui_state.take_egui_input(&window);
                            let mut dialog_result: Option<file_dialog::DialogResult> = None;
                            let full_output = egui_ctx.run(raw_input, |ctx| {
                                // --- горячие клавиши (раньше их не было вовсе, даже
                                // Ctrl+Z/Ctrl+S). Пока фокус в текстовом поле
                                // (скрипт, имя кости, путь) — не перехватываем,
                                // чтобы Space/буквы/Backspace печатались. ---
                                // Пока открыт проводник, клавиши принадлежат ему.
                                if !ctx.wants_keyboard_input() && !file_dialog.is_open() {
                                    use egui::{Key, KeyboardShortcut, Modifiers};
                                    let shortcuts = [
                                        (KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::S), Action::SaveAs),
                                        (KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z), Action::Redo),
                                        (KeyboardShortcut::new(Modifiers::COMMAND, Key::Z), Action::Undo),
                                        (KeyboardShortcut::new(Modifiers::COMMAND, Key::Y), Action::Redo),
                                        (KeyboardShortcut::new(Modifiers::COMMAND, Key::S), Action::Save),
                                        (KeyboardShortcut::new(Modifiers::COMMAND, Key::O), Action::Open),
                                        (KeyboardShortcut::new(Modifiers::COMMAND, Key::N), Action::New),
                                        (KeyboardShortcut::new(Modifiers::NONE, Key::Space), Action::TogglePlay),
                                        (KeyboardShortcut::new(Modifiers::NONE, Key::Home), Action::Rewind),
                                        (KeyboardShortcut::new(Modifiers::NONE, Key::End), Action::GoToEnd),
                                        (KeyboardShortcut::new(Modifiers::NONE, Key::ArrowLeft), Action::PrevFrame),
                                        (KeyboardShortcut::new(Modifiers::NONE, Key::Comma), Action::PrevFrame),
                                        (KeyboardShortcut::new(Modifiers::NONE, Key::ArrowRight), Action::NextFrame),
                                        (KeyboardShortcut::new(Modifiers::NONE, Key::Period), Action::NextFrame),
                                        (KeyboardShortcut::new(Modifiers::NONE, Key::F6), Action::InsertKeyframe),
                                    ];
                                    for (shortcut, action) in shortcuts {
                                        if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                                            actions.push(action);
                                        }
                                    }
                                    // Масштаб вида: Ctrl+1 — 100%, Ctrl+2 — весь документ,
                                    // Ctrl+3 — персонаж; Ctrl+= / Ctrl+- — шаг зума.
                                    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Num1))) {
                                        stage_zoom = 1.0;
                                    }
                                    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Num2))) {
                                        fit_request = Some(FitView::Document);
                                    }
                                    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Num3))) {
                                        fit_request = Some(FitView::Character);
                                    }
                                    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals)) || i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Plus))) {
                                        stage_zoom = (stage_zoom * 1.25).min(MAX_STAGE_ZOOM);
                                    }
                                    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Minus))) {
                                        stage_zoom = (stage_zoom / 1.25).max(MIN_STAGE_ZOOM);
                                    }
                                    // Инструменты — буквы как в Adobe Animate.
                                    let no_mods = ctx.input(|i| i.modifiers.is_none());
                                    if no_mods {
                                        for (key, tool) in [
                                            (Key::V, Tool::Selection),
                                            (Key::A, Tool::SubSelection),
                                            (Key::Q, Tool::FreeTransform),
                                            (Key::L, Tool::Lasso),
                                            (Key::P, Tool::Pen),
                                            (Key::Y, Tool::Pencil),
                                            (Key::B, Tool::Brush),
                                            (Key::N, Tool::Line),
                                            (Key::R, Tool::Rectangle),
                                            (Key::O, Tool::Oval),
                                            (Key::K, Tool::PaintBucket),
                                            (Key::S, Tool::InkBottle),
                                            (Key::I, Tool::Eyedropper),
                                            (Key::H, Tool::Hand),
                                            (Key::Z, Tool::Zoom),
                                        ] {
                                            if ctx.input(|i| i.key_pressed(key)) {
                                                active_tool = tool;
                                            }
                                        }
                                        // Escape — отменить незаконченное: путь Pen,
                                        // установку кости, групповое выделение.
                                        if ctx.input(|i| i.key_pressed(Key::Escape)) {
                                            if !pen_points.is_empty() {
                                                pen_points.clear();
                                                status_message = "Путь Pen отменён".into();
                                            } else if placing_bone {
                                                placing_bone = false;
                                            } else if active_tool == Tool::FreeTransform && selected_bone.is_some() {
                                                // Выйти из выбранной кости (например, контейнера
                                                // головы) — следующий клик возьмёт кость части.
                                                selected_bone = None;
                                                status_message = "Xform: выбор кости снят".into();
                                            } else if !multi_selected.is_empty() {
                                                multi_selected.clear();
                                            } else {
                                                selected_layer = None;
                                                selected_shape_index = None;
                                                selected_keyframe = None;
                                            }
                                        }
                                        // Enter — закончить открытый путь Pen ломаной
                                        // (замкнуть — клик по первой точке).
                                        if ctx.input(|i| i.key_pressed(Key::Enter)) && pen_points.len() >= 2 {
                                            vector_doc.add(pony_core::VectorShape::Polyline {
                                                points: std::mem::take(&mut pen_points),
                                                stroke: color32_to_rgba(stroke_color),
                                                stroke_width: 1.5,
                                            });
                                            status_message = "Открытый путь Pen добавлен".into();
                                        }
                                        // Delete в SubSelection — удалить выбранную фигуру холста
                                        // (ключи таймлайна удаляются там же, в timeline_widget).
                                        if active_tool == Tool::SubSelection && ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
                                            if let Some(idx) = selected_shape_index.take() {
                                                if idx < vector_doc.shapes.len() {
                                                    vector_doc.shapes.remove(idx);
                                                    status_message = "Фигура удалена".into();
                                                }
                                            }
                                        }
                                    }
                                }

                                egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
                                    egui::menu::bar(ui, |ui| {
                                        ui.menu_button("File", |ui| {
                                            ui.set_min_width(300.0);
                                            if ui.add(egui::Button::new("New").shortcut_text("Ctrl+N")).clicked() {
                                                actions.push(Action::New);
                                                ui.close_menu();
                                            }
                                            if ui.add(egui::Button::new("Open…").shortcut_text("Ctrl+O")).on_hover_text(".asset — персонаж; .fla/.xfl — импорт Adobe Animate").clicked() {
                                                actions.push(Action::Open);
                                                ui.close_menu();
                                            }
                                            if ui.add(egui::Button::new("Save").shortcut_text("Ctrl+S")).clicked() {
                                                actions.push(Action::Save);
                                                ui.close_menu();
                                            }
                                            if ui.add(egui::Button::new("Save As…").shortcut_text("Ctrl+Shift+S")).clicked() {
                                                actions.push(Action::SaveAs);
                                                ui.close_menu();
                                            }
                                            ui.weak(if asset_path.is_empty() { "Документ ещё не сохранён".to_string() } else { format!("Файл: {asset_path}") });
                                            ui.separator();
                                            if ui.button("Демо-пони (пример)").on_hover_text("Открыть встроенного демо-персонажа с анимацией Walk").clicked() {
                                                actions.push(Action::DemoPony);
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("Import Adobe Animate (.fla/.xfl)…").clicked() {
                                                xfl_window_open = true;
                                                if xfl_doc.is_none() {
                                                    file_dialog.show_for(file_dialog::Purpose::ImportAnimate, None, "");
                                                }
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("Import image as layer…").on_hover_text("PNG, SVG, PSD, KRA — новый слой на текущем персонаже").clicked() {
                                                file_dialog.show_for(file_dialog::Purpose::ImportLayer, None, "");
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("Export GIF… (текущая анимация)").clicked() {
                                                actions.push(Action::ExportGif);
                                                ui.close_menu();
                                            }
                                            if ui.button("Export Spritesheet… (текущая анимация)").clicked() {
                                                actions.push(Action::ExportSpritesheet);
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("Exit").clicked() {
                                                elwt.exit();
                                            }
                                        });
                                        ui.menu_button("Edit", |ui| {
                                            let can_undo = !history.undo.is_empty();
                                            let can_redo = !history.redo.is_empty();
                                            if ui.add_enabled(can_undo, egui::Button::new(format!("Undo ({})", history.undo.len())).shortcut_text("Ctrl+Z")).clicked() {
                                                actions.push(Action::Undo);
                                                ui.close_menu();
                                            }
                                            if ui.add_enabled(can_redo, egui::Button::new(format!("Redo ({})", history.redo.len())).shortcut_text("Ctrl+Shift+Z")).clicked() {
                                                actions.push(Action::Redo);
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            let has_sel = selected_layer.is_some();
                                            if ui.add_enabled(has_sel, egui::Button::new("Delete layer")).clicked() {
                                                if let Some(id) = selected_layer.clone() {
                                                    character.parts.remove(&id);
                                                    hidden_layers.remove(&id);
                                                    locked_layers.remove(&id);
                                                    selected_layer = None;
                                                    status_message = format!("Слой '{id}' удалён");
                                                }
                                                ui.close_menu();
                                            }
                                            if ui.add_enabled(has_sel, egui::Button::new("Duplicate layer")).clicked() {
                                                if let Some(id) = selected_layer.clone() {
                                                    if let Some(src) = character.parts.get(&id).cloned() {
                                                        let new_id = unique_name(&format!("{id}_copy"), |c| character.parts.contains_key(c));
                                                        let mut copy = src.clone();
                                                        copy.id = new_id.clone();
                                                        copy.layer = src.layer + 1;
                                                        character.parts.insert(new_id.clone(), copy);
                                                        selected_layer = Some(new_id.clone());
                                                        status_message = format!("Создана копия '{new_id}'");
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.add_enabled(has_sel, egui::Button::new("Deselect")).clicked() {
                                                selected_layer = None;
                                                ui.close_menu();
                                            }
                                            let has_group = !multi_selected.is_empty();
                                            if has_group {
                                                ui.separator();
                                                ui.weak(format!("Групповое выделение (Lasso): {} частей", multi_selected.len()));
                                                if ui.button("Скрыть группу").clicked() {
                                                    hidden_layers.extend(multi_selected.iter().cloned());
                                                    ui.close_menu();
                                                }
                                                if ui.button("Показать группу").clicked() {
                                                    for id in &multi_selected {
                                                        hidden_layers.remove(id);
                                                    }
                                                    ui.close_menu();
                                                }
                                                if ui.button("Удалить группу").clicked() {
                                                    for id in multi_selected.drain() {
                                                        character.parts.remove(&id);
                                                        hidden_layers.remove(&id);
                                                        locked_layers.remove(&id);
                                                    }
                                                    status_message = "Группа удалена".into();
                                                    ui.close_menu();
                                                }
                                                if ui.button("Снять групповое выделение").clicked() {
                                                    multi_selected.clear();
                                                    ui.close_menu();
                                                }
                                                ui.separator();
                                                // Раздел 27/44 ТЗ ("Group" в меню Modify) — превратить
                                                // ЭФЕМЕРНОЕ Lasso-выделение (`multi_selected`, живёт
                                                // только пока не снято/не заменено) в ПОСТОЯННУЮ
                                                // группу (`Character::groups`, переживает сохранение
                                                // .asset) — та же организационная роль, что `<g>` в SVG.
                                                if ui.button("Group (создать постоянную группу)").clicked() {
                                                    let ids: Vec<String> = multi_selected.iter().cloned().collect();
                                                    let group_id = character.group_parts(format!("Group {}", character.groups.groups.len() + 1), &ids);
                                                    status_message = format!("Группа '{group_id}' создана из {} частей", ids.len());
                                                    multi_selected.clear();
                                                    ui.close_menu();
                                                }
                                            }
                                        });

                                        ui.menu_button("View", |ui| {
                                            if ui.add(egui::Button::new("Zoom In").shortcut_text("Ctrl+=")).clicked() {
                                                stage_zoom = (stage_zoom * 1.25).min(MAX_STAGE_ZOOM);
                                                ui.close_menu();
                                            }
                                            if ui.add(egui::Button::new("Zoom Out").shortcut_text("Ctrl+-")).clicked() {
                                                stage_zoom = (stage_zoom / 1.25).max(MIN_STAGE_ZOOM);
                                                ui.close_menu();
                                            }
                                            if ui.add(egui::Button::new("Zoom 100%").shortcut_text("Ctrl+1")).clicked() {
                                                stage_zoom = 1.0;
                                                ui.close_menu();
                                            }
                                            if ui.add(egui::Button::new("Fit Document").shortcut_text("Ctrl+2")).clicked() {
                                                fit_request = Some(FitView::Document);
                                                ui.close_menu();
                                            }
                                            if ui.add(egui::Button::new("Fit Character").shortcut_text("Ctrl+3")).clicked() {
                                                fit_request = Some(FitView::Character);
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            ui.checkbox(&mut show_rulers, "Линейки");
                                            ui.checkbox(&mut show_timeline, "Timeline");
                                            ui.checkbox(&mut show_right_panel, "Правые панели");
                                            ui.checkbox(&mut show_skeleton, "Скелет");
                                        });

                                        ui.menu_button("Insert", |ui| {
                                            if ui
                                                .add_enabled(selected_layer.is_some(), egui::Button::new(format!("Keyframe ({})", channel_label(timeline_channel))).shortcut_text("F6"))
                                                .on_disabled_hover_text("Сначала выбери слой (на Stage или в Timeline)")
                                                .clicked()
                                            {
                                                actions.push(Action::InsertKeyframe);
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("Пустой слой (заглушка-цвет)").clicked() {
                                                blank_layer_count += 1;
                                                let id = unique_name(&format!("layer_{blank_layer_count}"), |c| character.parts.contains_key(c));
                                                let max_layer = character.parts.values().map(|p| p.layer).max().unwrap_or(0);
                                                character.add_part(
                                                    pony_core::part::Part::new(
                                                        &id,
                                                        pony_core::part::PartKind::Custom,
                                                        pony_core::part::PartSource::Png { path: String::new() },
                                                    )
                                                    .with_bone(default_bone(&character))
                                                    .with_layer(max_layer + 1),
                                                );
                                                selected_layer = Some(id.clone());
                                                status_message = format!("Добавлен пустой слой '{id}'");
                                                ui.close_menu();
                                            }
                                        });

                                        ui.menu_button("Modify", |ui| {
                                            let has_sel = selected_layer.is_some();
                                            if ui.add_enabled(has_sel, egui::Button::new("Отразить по горизонтали")).clicked() {
                                                if let Some(bone_id) = selected_bone_id(&character, &selected_layer) {
                                                    if let Some(b) = character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                                        b.local_transform.scale.x = -b.local_transform.scale.x;
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            if ui.add_enabled(has_sel, egui::Button::new("Отразить по вертикали")).clicked() {
                                                if let Some(bone_id) = selected_bone_id(&character, &selected_layer) {
                                                    if let Some(b) = character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                                        b.local_transform.scale.y = -b.local_transform.scale.y;
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            if ui.add_enabled(has_sel, egui::Button::new("Сбросить трансформацию")).clicked() {
                                                if let Some(bone_id) = selected_bone_id(&character, &selected_layer) {
                                                    if let Some(b) = character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                                        b.local_transform = pony_core::skeleton::Transform2D::default();
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.add_enabled(has_sel, egui::Button::new("Поднять слой выше")).clicked() {
                                                if let Some(id) = selected_layer.clone() {
                                                    if let Some(p) = character.parts.get_mut(&id) {
                                                        p.layer += 1;
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            if ui.add_enabled(has_sel, egui::Button::new("Опустить слой ниже")).clicked() {
                                                if let Some(id) = selected_layer.clone() {
                                                    if let Some(p) = character.parts.get_mut(&id) {
                                                        p.layer -= 1;
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                        });

                                        ui.menu_button("Text", |ui| {
                                            // Честно: в движке нет рендера текста вообще — ни
                                            // шрифтов как ассетов, ни текстовых частей персонажа.
                                            // Пункт оставлен, чтобы структура меню совпадала с
                                            // присланным описанием Animate, но притворяться, что
                                            // он что-то делает, смысла нет.
                                            ui.weak("В движке нет текстовых объектов:");
                                            ui.weak("персонаж состоит из частей-текстур и костей,");
                                            ui.weak("рендера шрифтов на Stage не существует.");
                                        });

                                        ui.menu_button("Commands", |ui| {
                                            if ui.button("Выполнить скрипт из вкладки Script").clicked() {
                                                match script_engine.run(&script_text) {
                                                    Ok(commands) => {
                                                        let n = commands.len();
                                                        pony_script::apply_commands(&mut character, &mut camera, &mut player, &commands);
                                                        take_blink(&mut character, &mut blink_timer);
                                                        script_log = format!("OK: выполнено {n} команд(ы)");
                                                        status_message = script_log.clone();
                                                    }
                                                    Err(err) => {
                                                        script_log = format!("Ошибка: {err}");
                                                        status_message = script_log.clone();
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            ui.weak("Готовые примеры:");
                                            for (label, src) in [
                                                ("Улыбнуться и моргнуть", "pony.Smile(0.9);\npony.Blink();"),
                                                ("Посмотреть вверх", "pony.Look(0.0, 1.0);"),
                                                ("Пойти", "pony.Walk();"),
                                                ("Тряска камеры", "camera.Shake(0.6);"),
                                            ] {
                                                if ui.button(label).clicked() {
                                                    script_text = src.to_string();
                                                    right_tab = RightTab::Script;
                                                    status_message = format!("Скрипт '{label}' загружен во вкладку Script");
                                                    ui.close_menu();
                                                }
                                            }
                                        });

                                        ui.menu_button("Control", |ui| {
                                            if ui.add(egui::Button::new(if playing { "Stop" } else { "Play" }).shortcut_text("Space")).clicked() {
                                                actions.push(Action::TogglePlay);
                                                ui.close_menu();
                                            }
                                            if ui.add(egui::Button::new("Rewind (в начало)").shortcut_text("Home")).clicked() {
                                                actions.push(Action::Rewind);
                                                ui.close_menu();
                                            }
                                            if ui.add(egui::Button::new("К концу анимации").shortcut_text("End")).clicked() {
                                                actions.push(Action::GoToEnd);
                                                ui.close_menu();
                                            }
                                            if ui.add(egui::Button::new("Кадр назад").shortcut_text("Left")).clicked() {
                                                actions.push(Action::PrevFrame);
                                            }
                                            if ui.add(egui::Button::new("Кадр вперёд").shortcut_text("Right")).clicked() {
                                                actions.push(Action::NextFrame);
                                            }
                                            ui.separator();
                                            ui.weak("Анимации персонажа:");
                                            let mut names: Vec<String> = character.animations.keys().cloned().collect();
                                            names.sort();
                                            for name in names {
                                                let is_current = player.current_name() == Some(name.as_str());
                                                if ui.selectable_label(is_current, &name).clicked() {
                                                    player.play(&name);
                                                    player.apply(&mut character);
                                                    selected_keyframe = None;
                                                    playing = true;
                                                    status_message = format!("Играет '{name}'");
                                                    ui.close_menu();
                                                }
                                            }
                                        });

                                        ui.menu_button("Debug", |ui| {
                                            ui.label(format!("GPU: {}", gpu_ctx.info.name));
                                            ui.label(format!("Бэкенд: {}", gpu_ctx.info.backend));
                                            ui.separator();
                                            let used = pony_renderer.texture_memory_used_bytes();
                                            let budget = pony_renderer.texture_memory_budget_bytes();
                                            ui.label(format!("Кэш текстур: {:.2} МиБ", used as f64 / (1024.0 * 1024.0)));
                                            ui.label(format!("Бюджет: {:.2} МиБ", budget as f64 / (1024.0 * 1024.0)));
                                            ui.separator();
                                            ui.label(format!("Частей у персонажа: {}", character.parts.len()));
                                            ui.label(format!("Костей: {}", character.skeleton.bones.len()));
                                            ui.label(format!("Анимаций: {}", character.animations.len()));
                                            ui.label(format!("Фигур на холсте: {}", vector_doc.shapes.len()));
                                            ui.label(format!("Живых частиц: {}", particle_emitter.particles.len()));
                                            ui.separator();
                                            ui.label(format!("dt последнего кадра: {:.1} мс", dt * 1000.0));
                                            ui.label(format!("Undo/Redo: {} / {}", history.undo.len(), history.redo.len()));
                                        });

                                        ui.menu_button("Window", |ui| {
                                            ui.checkbox(&mut show_timeline, "Timeline");
                                            ui.checkbox(&mut show_right_panel, "Panels");
                                            ui.checkbox(&mut show_rulers, "Линейки");
                                        });
                                        ui.menu_button("Help", |ui| {
                                            ui.strong("Горячие клавиши");
                                            for line in [
                                                "Ctrl+N / Ctrl+O / Ctrl+S — новый / открыть / сохранить",
                                                "Ctrl+Z / Ctrl+Shift+Z (Ctrl+Y) — отменить / вернуть",
                                                "Space — play/pause, стрелки Left/Right — кадр, Home/End — начало/конец",
                                                "F6 — ключевой кадр для выбранного слоя",
                                                "Колесо — зум к курсору, средняя кнопка — пан",
                                                "Ctrl+1 / Ctrl+2 / Ctrl+3 — 100% / документ / персонаж",
                                                "V A Q L — Sel/SubSel/Xform/Lasso",
                                                "Xform: тащить — двигать кость, Shift — вращать, Alt+клик — кость-родитель, Esc — снять",
                                                "File > Import Adobe Animate — открыть .fla/.xfl, выбрать символ и кадр",
                                                "P Y B N R O — Pen/Pencil/Brush/Line/Rect/Oval",
                                                "K S I — Fill/Ink/Eyedropper, H Z — Hand/Zoom",
                                                "Pen: клик — точка, клик по первой — замкнуть, Enter — закончить, Esc — отмена",
                                                "Delete — удалить выбранный ключ (Timeline) / фигуру (SubSel)",
                                            ] {
                                                ui.label(line);
                                            }
                                            ui.separator();
                                            ui.weak("pony-engine — см. README.md в репозитории");
                                        });
                                    });
                                });

                                egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
                                    ui.horizontal(|ui| {
                                        // Те же действия, что в меню File/Edit (раньше
                                        // тулбарные Undo/Redo были навсегда выключены, а
                                        // New/Open/Save молча глотали ошибки).
                                        if ui.button("New").on_hover_text("Ctrl+N").clicked() {
                                            actions.push(Action::New);
                                        }
                                        if ui.button("Open").on_hover_text("Ctrl+O").clicked() {
                                            actions.push(Action::Open);
                                        }
                                        if ui.button("Save").on_hover_text(if asset_path.is_empty() { "Ctrl+S — выбрать, куда сохранить".to_string() } else { format!("Ctrl+S — {asset_path}") }).clicked() {
                                            actions.push(Action::Save);
                                        }
                                        ui.separator();
                                        if ui.add_enabled(!history.undo.is_empty(), egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
                                            actions.push(Action::Undo);
                                        }
                                        if ui.add_enabled(!history.redo.is_empty(), egui::Button::new("Redo")).on_hover_text("Ctrl+Shift+Z / Ctrl+Y").clicked() {
                                            actions.push(Action::Redo);
                                        }
                                        ui.separator();
                                        if ui.button("Export GIF").on_hover_text("Сохранить текущую анимацию в GIF").clicked() {
                                            actions.push(Action::ExportGif);
                                        }
                                        ui.separator();
                                        ui.label("Zoom:");
                                        // Логарифмический и того же диапазона, что колесо мыши:
                                        // раньше слайдер (0.25..4) каждый кадр зажимал зум,
                                        // выставленный колесом, обратно в свой диапазон.
                                        ui.add(
                                            egui::Slider::new(&mut stage_zoom, MIN_STAGE_ZOOM..=MAX_STAGE_ZOOM)
                                                .logarithmic(true)
                                                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                                                .custom_parser(|t| t.trim().trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0)),
                                        );
                                        if ui.button("Fit").on_hover_text("Ctrl+3 — показать персонажа целиком").clicked() {
                                            fit_request = Some(FitView::Character);
                                        }
                                    });
                                });

                                egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(format!("Инструмент: {}", active_tool.label())).on_hover_text(active_tool.hint());
                                        ui.separator();
                                        ui.label(format!("Выделение: {}", selected_layer.as_deref().unwrap_or("нет")));
                                        ui.separator();
                                        ui.label(format!("Документ: {SCENE_WIDTH}×{SCENE_HEIGHT}"));
                                        ui.separator();
                                        ui.label(format!("Zoom: {:.0}%", stage_zoom * 100.0));
                                        ui.separator();
                                        ui.label(&status_message);
                                    });
                                });

                                egui::TopBottomPanel::bottom("playback_bar").show(ctx, |ui| {
                                    ui.horizontal(|ui| {
                                        if ui.button("⏮").on_hover_text("В начало (Home)").clicked() {
                                            actions.push(Action::Rewind);
                                        }
                                        if ui.button("◀|").on_hover_text("Кадр назад (Left)").clicked() {
                                            actions.push(Action::PrevFrame);
                                        }
                                        if ui.button(if playing { "⏸" } else { "▶" }).on_hover_text("Play/Pause (Space)").clicked() {
                                            actions.push(Action::TogglePlay);
                                        }
                                        if ui.button("|▶").on_hover_text("Кадр вперёд (Right)").clicked() {
                                            actions.push(Action::NextFrame);
                                        }
                                        if ui.button("⏭").on_hover_text("В конец (End)").clicked() {
                                            actions.push(Action::GoToEnd);
                                        }
                                        ui.separator();
                                        let current_frame = (player.time() * FPS).round() as i64;
                                        let total = player.current_name().and_then(|n| character.animations.get(n)).map(|a| (a.duration * FPS).round() as i64).unwrap_or(0);
                                        ui.label(format!("Кадр: {current_frame} / {total}"));
                                        ui.separator();
                                        ui.label(format!("FPS: {FPS:.0}"));
                                        ui.separator();
                                        ui.label("Scene 1");
                                        ui.separator();
                                        let loops = player
                                            .current_name()
                                            .and_then(|n| character.animations.get(n))
                                            .map(|a| a.looping)
                                            .unwrap_or(false);
                                        ui.label(format!("Loop: {}", if loops { "on" } else { "off" }));
                                        ui.separator();
                                        ui.label(format!("Zoom: {:.0}%", stage_zoom * 100.0));
                                    });
                                });

                                egui::SidePanel::left("tools").resizable(false).min_width(64.0).show(ctx, |ui| {
                                    ui.vertical_centered_justified(|ui| {
                                        for tool in [Tool::Selection, Tool::SubSelection, Tool::FreeTransform, Tool::Lasso] {
                                            if ui.selectable_label(active_tool == tool, tool.label()).on_hover_text(tool.hint()).clicked() {
                                                active_tool = tool;
                                            }
                                        }
                                        ui.separator();
                                        for tool in [Tool::Pen, Tool::Pencil, Tool::Brush, Tool::Line, Tool::Rectangle, Tool::Oval, Tool::PolyStar] {
                                            if ui.selectable_label(active_tool == tool, tool.label()).on_hover_text(tool.hint()).clicked() {
                                                active_tool = tool;
                                            }
                                        }
                                        ui.separator();
                                        for tool in [Tool::PaintBucket, Tool::InkBottle, Tool::Eyedropper] {
                                            if ui.selectable_label(active_tool == tool, tool.label()).on_hover_text(tool.hint()).clicked() {
                                                active_tool = tool;
                                            }
                                        }
                                        ui.separator();
                                        for tool in [Tool::Hand, Tool::Zoom] {
                                            if ui.selectable_label(active_tool == tool, tool.label()).on_hover_text(tool.hint()).clicked() {
                                                active_tool = tool;
                                            }
                                        }
                                        ui.separator();
                                        ui.label("Stroke");
                                        ui.color_edit_button_srgba(&mut stroke_color);
                                        ui.label("Fill");
                                        ui.color_edit_button_srgba(&mut fill_color);
                                        if active_tool == Tool::PolyStar {
                                            ui.add(egui::DragValue::new(&mut poly_sides).prefix("Углов: ").clamp_range(3..=12));
                                        }
                                        ui.separator();
                                        ui.label(format!("SVG: {} фигур(ы)", vector_doc.shapes.len()));
                                        if ui
                                            .add_enabled(!vector_doc.is_empty(), egui::Button::new("Сохранить как слой"))
                                            .on_disabled_hover_text("Сначала нарисуй что-нибудь (Rect/Oval/Pen/...)")
                                            .clicked()
                                        {
                                            let (min_x, min_y, max_x, max_y) = vector_doc.bounds().unwrap_or((0.0, 0.0, 1.0, 1.0));
                                            let draw_center = glam::Vec2::new((min_x + max_x) / 2.0, -(min_y + max_y) / 2.0);
                                            let draw_size = glam::Vec2::new((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));

                                            if let Some(edit_id) = editing_part.clone() {
                                                // Обновляем СУЩЕСТВУЮЩУЮ часть на месте — тот же
                                                // id, тот же файл на диске (перезаписываем), та же
                                                // кость. Файл должен быть переINCLUDE тем же путём —
                                                // иначе часть указывала бы на старый неотредактированный
                                                // .svg. И критично: инвалидируем кэш текстур для
                                                // этого пути, иначе персонаж продолжит рисоваться со
                                                // старой (закэшированной) картинкой, хотя файл на диске
                                                // уже другой.
                                                if let Some(part) = character.parts.get(&edit_id).cloned() {
                                                    if let pony_core::part::PartSource::Vector { path } = &part.source {
                                                        match std::fs::write(path, vector_doc.to_svg_string()) {
                                                            Ok(()) => {
                                                                pony_renderer.invalidate_texture(path);
                                                                force_render = true;
                                                                let bone_world = part.bone.as_ref().and_then(|b| character.skeleton.world_transform_with_ik(b)).unwrap_or_default();
                                                                let pivot = pony_render::pivot_for_world_position(draw_center, &bone_world);
                                                                if let Some(p) = character.parts.get_mut(&edit_id) {
                                                                    p.pivot = pivot;
                                                                    p.size = Some(draw_size);
                                                                }
                                                                vector_doc.clear();
                                                                editing_part = None;
                                                                status_message = format!("'{edit_id}' обновлён ({path})");
                                                            }
                                                            Err(err) => status_message = format!("Ошибка сохранения SVG: {err}"),
                                                        }
                                                    }
                                                } else {
                                                    // Часть, которую редактировали, успели удалить
                                                    // (например, через Edit > Delete layer) — не
                                                    // молчим об этом, честно сообщаем и сбрасываем
                                                    // режим редактирования, не создавая ничего левого.
                                                    editing_part = None;
                                                    status_message = "Редактируемая часть больше не существует — сохранение отменено".into();
                                                }
                                            } else {
                                                svg_save_count += 1;
                                                // Не занятый ни частью, ни файлом на диске (иначе
                                                // перезаписали бы .svg уже существующей части).
                                                let part_id = unique_name(&format!("drawn_{svg_save_count}"), |c| {
                                                    character.parts.contains_key(c) || std::path::Path::new(&format!("{c}.svg")).exists()
                                                });
                                                let path = format!("{part_id}.svg");
                                                match std::fs::write(&path, vector_doc.to_svg_string()) {
                                                    Ok(()) => {

                                                        // Часть должна встать ТУДА, где нарисована, и
                                                        // ТОГО размера, какого нарисована — иначе
                                                        // «нарисовал в углу» превращалось бы в
                                                        // «появилось в центре персонажа непонятного
                                                        // размера», и рисование было бы бесполезно.
                                                        // Габарит рисунка в SVG-координатах (Y вниз),
                                                        // мир — Y вверх, отсюда минус.
                                                        let bone_id = selected_layer
                                                            .as_ref()
                                                            .and_then(|id| character.parts.get(id))
                                                            .and_then(|p| p.bone.clone())
                                                            .unwrap_or_else(|| default_bone(&character));
                                                        let bone_world = character.skeleton.world_transform_with_ik(&bone_id).unwrap_or_default();
                                                        let pivot = pony_render::pivot_for_world_position(draw_center, &bone_world);

                                                        let max_layer = character.parts.values().map(|p| p.layer).max().unwrap_or(0);
                                                        character.add_part(
                                                            pony_core::part::Part::new(
                                                                &part_id,
                                                                pony_core::part::PartKind::Custom,
                                                                pony_core::part::PartSource::Vector { path: path.clone() },
                                                            )
                                                            .with_bone(&bone_id)
                                                            .with_pivot(pivot)
                                                            .with_size(draw_size)
                                                            .with_layer(max_layer + 1),
                                                        );
                                                        vector_doc.clear();
                                                        selected_layer = Some(part_id.clone());
                                                        status_message = format!("'{part_id}' -> {path}, кость '{bone_id}', {:.0}x{:.0}", draw_size.x, draw_size.y);
                                                    }
                                                    Err(err) => status_message = format!("Ошибка сохранения SVG: {err}"),
                                                }
                                            }
                                        }
                                        if editing_part.is_some() {
                                            ui.small(format!("Редактируется: {}", editing_part.as_deref().unwrap_or("")));
                                            if ui.button("Отменить редактирование").clicked() {
                                                editing_part = None;
                                                vector_doc.clear();
                                                status_message = "Редактирование отменено".into();
                                            }
                                        }
                                        if ui.add_enabled(!vector_doc.is_empty(), egui::Button::new("Очистить холст")).clicked() {
                                            vector_doc.clear();
                                        }
                                    });
                                });

                                if show_right_panel {
                                    egui::SidePanel::right("right_panel").min_width(300.0).show(ctx, |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            for tab in [
                                                RightTab::Properties,
                                                RightTab::Library,
                                                RightTab::Color,
                                                RightTab::Align,
                                                RightTab::Transform,
                                                RightTab::Info,
                                                RightTab::Script,
                                                RightTab::Lighting,
                                                RightTab::Particles,
                                                RightTab::Bones,
                                                RightTab::SvgInspector,
                                                RightTab::Symbols,
                                                RightTab::Groups,
                                                RightTab::Gradients,
                                                RightTab::Boolean,
                                            ] {
                                                if ui.selectable_label(right_tab == tab, tab.label()).clicked() {
                                                    right_tab = tab;
                                                }
                                            }
                                        });
                                        ui.separator();
                                        match right_tab {
                                            RightTab::Properties => match selected_layer.clone() {
                                                Some(id) => {
                                                    if let Some(part) = character.parts.get(&id).cloned() {
                                                        ui.label(format!("ID: {}", part.id));
                                                        ui.label(format!("Вид: {:?}", part.kind));

                                                        // "Использование" замкнутого цикла SVG:
                                                        // создание/редактирование/использование —
                                                        // часть, чей источник уже .svg, можно открыть
                                                        // обратно на редактирование теми же
                                                        // инструментами (Rect/SubSelection/...), что
                                                        // и при рисовании с нуля, а не только двигать
                                                        // как единое целое. Та же кнопка есть и в
                                                        // отдельной вкладке "SVG" (SVG Inspector) —
                                                        // там видны СРАЗУ все SVG-части персонажа,
                                                        // не только выбранная.
                                                        if let pony_core::part::PartSource::Vector { path } = &part.source {
                                                            if ui.button("Редактировать SVG").clicked() {
                                                                status_message = start_editing_svg_part(
                                                                    &id,
                                                                    path,
                                                                    &mut vector_doc,
                                                                    &mut editing_part,
                                                                    &mut active_tool,
                                                                    &mut selected_shape_index,
                                                                );
                                                            }
                                                        }
                                                        ui.separator();

                                                        // Перепривязка к другой кости. Смещение
                                                        // пересчитывается так, чтобы часть ОСТАЛАСЬ
                                                        // на месте на экране — иначе смена кости
                                                        // телепортировала бы её неизвестно куда, и
                                                        // пользоваться этим было бы страшно.
                                                        let current_bone = part.bone.clone().unwrap_or_else(|| "(нет)".into());
                                                        let mut new_bone: Option<String> = None;
                                                        egui::ComboBox::from_label("Кость").selected_text(&current_bone).show_ui(ui, |ui| {
                                                            for b in &character.skeleton.bones {
                                                                if ui.selectable_label(current_bone == b.id, &b.id).clicked() {
                                                                    new_bone = Some(b.id.clone());
                                                                }
                                                            }
                                                        });
                                                        if let Some(target_bone) = new_bone {
                                                            if Some(&target_bone) != part.bone.as_ref() {
                                                                let old_world = part
                                                                    .bone
                                                                    .as_ref()
                                                                    .and_then(|b| character.skeleton.world_transform_with_ik(b))
                                                                    .unwrap_or_default();
                                                                let keep_at = pony_render::part_world_position(&part, &old_world);
                                                                let new_world = character.skeleton.world_transform_with_ik(&target_bone).unwrap_or_default();
                                                                let new_pivot = pony_render::pivot_for_world_position(keep_at, &new_world);
                                                                if let Some(p) = character.parts.get_mut(&id) {
                                                                    p.bone = Some(target_bone.clone());
                                                                    p.pivot = new_pivot;
                                                                }
                                                                status_message = format!("'{id}' перепривязан к кости '{target_bone}'");
                                                            }
                                                        }

                                                        ui.separator();
                                                        ui.label("Смещение от кости:");
                                                        let mut pivot = part.pivot;
                                                        let px = ui.add(egui::DragValue::new(&mut pivot.x).prefix("X: ").speed(0.5));
                                                        let py = ui.add(egui::DragValue::new(&mut pivot.y).prefix("Y: ").speed(0.5));
                                                        if px.changed() || py.changed() {
                                                            if let Some(p) = character.parts.get_mut(&id) {
                                                                p.pivot = pivot;
                                                            }
                                                        }

                                                        ui.separator();
                                                        ui.label("Размер:");
                                                        let mut size = pony_render::part_render_size(&part);
                                                        let sw = ui.add(egui::DragValue::new(&mut size.x).prefix("Ш: ").speed(0.5).clamp_range(1.0..=1000.0));
                                                        let sh = ui.add(egui::DragValue::new(&mut size.y).prefix("В: ").speed(0.5).clamp_range(1.0..=1000.0));
                                                        if sw.changed() || sh.changed() {
                                                            if let Some(p) = character.parts.get_mut(&id) {
                                                                p.size = Some(size);
                                                            }
                                                        }
                                                        if part.size.is_some() && ui.button("Сбросить размер к виду части").clicked() {
                                                            if let Some(p) = character.parts.get_mut(&id) {
                                                                p.size = None;
                                                            }
                                                        }

                                                        ui.separator();
                                                        let mut layer = part.layer;
                                                        if ui.add(egui::DragValue::new(&mut layer).prefix("Порядок отрисовки: ")).changed() {
                                                            if let Some(p) = character.parts.get_mut(&id) {
                                                                p.layer = layer;
                                                            }
                                                        }

                                                        // Маска/Clipping (раздел 60 ТЗ) — часть.clip_by
                                                        // ссылается на ДРУГУЮ часть этого же
                                                        // персонажа, чья альфа режет альфу текущей
                                                        // части (растровая маска, как маскирующий
                                                        // слой в Adobe Animate). Сама часть-маска
                                                        // из этого списка исключена (нельзя
                                                        // назначить саму себя — resolve_clip_mask
                                                        // всё равно бы это отверг на рендере, но
                                                        // честнее не предлагать такой вариант в UI
                                                        // вообще), а части, УЖЕ являющиеся чьей-то
                                                        // маской (`clip_by.is_some()` у самих себя
                                                        // не проверяем — проверяем, что цепочка не
                                                        // получится глубже одного уровня, как и в
                                                        // resolve_clip_mask) — тоже исключены, чтобы
                                                        // не предлагать в UI комбинацию, которую
                                                        // рендер всё равно проигнорирует.
                                                        ui.separator();
                                                        let current_mask = part.clip_by.clone().unwrap_or_else(|| "(нет)".into());
                                                        let mut new_mask: Option<Option<String>> = None;
                                                        egui::ComboBox::from_label("Маска (Clip by)").selected_text(&current_mask).show_ui(ui, |ui| {
                                                            if ui.selectable_label(part.clip_by.is_none(), "(нет)").clicked() {
                                                                new_mask = Some(None);
                                                            }
                                                            for other in character.parts.values() {
                                                                if other.id == id || other.clip_by.is_some() {
                                                                    continue;
                                                                }
                                                                if ui.selectable_label(part.clip_by.as_deref() == Some(other.id.as_str()), &other.id).clicked() {
                                                                    new_mask = Some(Some(other.id.clone()));
                                                                }
                                                            }
                                                        });
                                                        if let Some(target_mask) = new_mask {
                                                            if target_mask != part.clip_by {
                                                                if let Some(p) = character.parts.get_mut(&id) {
                                                                    p.clip_by = target_mask.clone();
                                                                }
                                                                status_message = match &target_mask {
                                                                    Some(m) => format!("'{id}' теперь маскируется частью '{m}'"),
                                                                    None => format!("'{id}': маска снята"),
                                                                };
                                                            }
                                                        }
                                                        ui.small("Маска обрезает альфу по форме выбранной части. Часть-маска сама продолжает рисоваться как обычно.");

                                                        ui.small("Инструмент Sel: клик — выбрать, перетаскивание — двигать саму часть.");
                                                        ui.small("Инструмент Xform: двигает кость (и всё, что к ней прикреплено).");
                                                    }
                                                }
                                                None => {
                                                    ui.label("(слой не выбран — кликни по нему на Stage или в Timeline)");
                                                }
                                            },
                                            RightTab::Library => {
                                                ui.label("Ассеты персонажа:");
                                                let mut paths: Vec<String> = character
                                                    .parts
                                                    .values()
                                                    .map(|p| match &p.source {
                                                        PartSource::Png { path } => format!("[PNG] {path}"),
                                                        PartSource::Vector { path } => format!("[SVG] {path}"),
                                                        PartSource::Psd { path, layer } => match layer {
                                                            Some(name) => format!("[PSD] {path}#{name}"),
                                                            None => format!("[PSD] {path}"),
                                                        },
                                                        PartSource::Kra { path, layer_file } => match layer_file {
                                                            Some(name) => format!("[KRA] {path}!{name}"),
                                                            None => format!("[KRA] {path}"),
                                                        },
                                                        PartSource::Mesh { path } => format!("[MESH] {path}"),
                                                    })
                                                    .collect();
                                                paths.sort();
                                                paths.dedup();
                                                egui::ScrollArea::vertical().show(ui, |ui| {
                                                    for p in paths {
                                                        ui.label(p);
                                                    }
                                                });
                                            }
                                            RightTab::Color => {
                                                ui.label("Fill:");
                                                ui.color_edit_button_srgba(&mut fill_color);
                                                ui.label("Stroke:");
                                                ui.color_edit_button_srgba(&mut stroke_color);
                                                ui.add_space(6.0);
                                                ui.small("Пока не влияет на рендер — движок сэмплит готовые PNG/SVG-текстуры, а не заливает векторные фигуры цветом из этой панели. Хук под будущий tint-проход.");
                                            }
                                            RightTab::Align => {
                                                ui.small("Не реализовано: нет мультивыбора объектов на Stage (в этой версии выбор — один слой через Timeline).");
                                            }
                                            RightTab::Transform => {
                                                ui.label("Поворот персонажа (2.5D, раздел 8 ТЗ):");
                                                // Слайдер правит сам character.facing_yaw (раньше —
                                                // отдельную UI-переменную, которая каждый кадр
                                                // затирала значение из открытого .asset и из Undo).
                                                let mut facing_yaw_deg = character.facing_yaw.to_degrees();
                                                if ui.add(egui::Slider::new(&mut facing_yaw_deg, -89.0..=89.0).suffix("°")).changed() {
                                                    character.facing_yaw = facing_yaw_deg.to_radians();
                                                }
                                                ui.separator();
                                                match &selected_layer {
                                                    Some(id) => {
                                                        let bone_id = character.parts.get(id).and_then(|p| p.bone.clone());
                                                        match bone_id {
                                                            Some(bone_id) => {
                                                                if let Some(bone) = character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                                                    ui.label(format!("Кость: {bone_id}"));
                                                                    ui.add(egui::DragValue::new(&mut bone.local_transform.position.x).prefix("X: ").speed(0.5));
                                                                    ui.add(egui::DragValue::new(&mut bone.local_transform.position.y).prefix("Y: ").speed(0.5));
                                                                    let mut rot_deg = bone.local_transform.rotation.to_degrees();
                                                                    if ui.add(egui::DragValue::new(&mut rot_deg).prefix("Rotate: ").suffix("°").speed(1.0)).changed() {
                                                                        bone.local_transform.rotation = rot_deg.to_radians();
                                                                    }
                                                                    ui.add(egui::DragValue::new(&mut bone.local_transform.scale.x).prefix("Scale X: ").speed(0.01));
                                                                    ui.add(egui::DragValue::new(&mut bone.local_transform.scale.y).prefix("Scale Y: ").speed(0.01));
                                                                }
                                                            }
                                                            None => {
                                                                ui.label("(у выбранного слоя нет привязанной кости)");
                                                            }
                                                        }
                                                    }
                                                    None => {
                                                        ui.label("(слой не выбран)");
                                                    }
                                                }
                                            }
                                            RightTab::Info => {
                                                ui.label(format!(
                                                    "Курсор на Stage: {}",
                                                    mouse_stage_pos.map(|p| format!("{:.0}, {:.0}", p.x, p.y)).unwrap_or_else(|| "—".into())
                                                ));
                                                ui.label(format!("Zoom: {:.0}%", stage_zoom * 100.0));
                                                ui.label(format!("Инструмент: {}", active_tool.label())).on_hover_text(active_tool.hint());
                                                if let Some(id) = &selected_layer {
                                                    ui.label(format!("Выбран слой: {id}"));
                                                    if let Some(part) = character.parts.get(id) {
                                                        if let Some(bone_id) = &part.bone {
                                                            if let Some(world) = character.skeleton.world_transform_with_ik(bone_id) {
                                                                ui.label(format!("Мировая позиция: {:.1}, {:.1}", world.position.x, world.position.y));
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            RightTab::Script => {
                                                ui.label("pony.Move()/Look()/Blink()/Smile()/Walk(), camera.Move()/Rotate()/Zoom()/Shake()/Depth()/Blur()");
                                                ui.add(egui::TextEdit::multiline(&mut script_text).desired_rows(6).font(egui::TextStyle::Monospace));
                                                if ui.button("▶ Выполнить").clicked() {
                                                    match script_engine.run(&script_text) {
                                                        Ok(commands) => {
                                                            let n = commands.len();
                                                            pony_script::apply_commands(&mut character, &mut camera, &mut player, &commands);
                                                            take_blink(&mut character, &mut blink_timer);
                                                            script_log = format!("OK: выполнено {n} команд(ы)");
                                                        }
                                                        Err(err) => script_log = format!("Ошибка: {err}"),
                                                    }
                                                }
                                                ui.separator();
                                                ui.label(&script_log);
                                            }
                                            RightTab::Lighting => {
                                                ui.label("Ambient:");
                                                ui.add(egui::Slider::new(&mut lighting.ambient.intensity, 0.0..=2.0).text("Intensity"));
                                                let mut ambient_color = egui::Color32::from_rgb(
                                                    (lighting.ambient.color[0] * 255.0) as u8,
                                                    (lighting.ambient.color[1] * 255.0) as u8,
                                                    (lighting.ambient.color[2] * 255.0) as u8,
                                                );
                                                if ui.color_edit_button_srgba(&mut ambient_color).changed() {
                                                    lighting.ambient.color = color32_to_rgb(ambient_color);
                                                }
                                                ui.separator();
                                                ui.checkbox(&mut sun_enabled, "Sun");
                                                if sun_enabled {
                                                    ui.color_edit_button_srgba(&mut sun_color);
                                                    ui.add(egui::Slider::new(&mut sun_intensity, 0.0..=2.0).text("Intensity"));
                                                }
                                                ui.separator();
                                                ui.checkbox(&mut point_enabled, "Point light");
                                                if point_enabled {
                                                    ui.color_edit_button_srgba(&mut point_color);
                                                    ui.add(egui::Slider::new(&mut point_intensity, 0.0..=3.0).text("Intensity"));
                                                    ui.add(egui::Slider::new(&mut point_radius, 10.0..=300.0).text("Radius"));
                                                    ui.add(egui::DragValue::new(&mut point_pos.x).prefix("X: ").speed(1.0));
                                                    ui.add(egui::DragValue::new(&mut point_pos.y).prefix("Y: ").speed(1.0));
                                                }
                                                ui.separator();
                                                ui.small("Glow/Shadow (раздел 12 ТЗ) не реализованы — см. README.");
                                            }
                                            RightTab::Particles => {
                                                ui.checkbox(&mut particles_enabled, "Включить эмиттер");
                                                let kinds = [
                                                    pony_core::ParticleKind::Dust,
                                                    pony_core::ParticleKind::Snow,
                                                    pony_core::ParticleKind::Rain,
                                                    pony_core::ParticleKind::Spark,
                                                    pony_core::ParticleKind::Magic,
                                                    pony_core::ParticleKind::Smoke,
                                                    pony_core::ParticleKind::Cloud,
                                                ];
                                                egui::ComboBox::from_label("Вид").selected_text(format!("{particle_kind:?}")).show_ui(ui, |ui| {
                                                    for k in kinds {
                                                        if ui.selectable_value(&mut particle_kind, k, format!("{k:?}")).changed() {
                                                            // Смена вида -- новый эмиттер (гравитация/цвет зависят от Kind).
                                                            let pos = particle_emitter.position;
                                                            let rate = particle_emitter.rate;
                                                            particle_emitter = pony_core::ParticleEmitter::new(particle_kind, pos, rate);
                                                        }
                                                    }
                                                });
                                                ui.add(egui::Slider::new(&mut particle_emitter.rate, 0.0..=60.0).text("Rate, частиц/с"));
                                                ui.add(egui::Slider::new(&mut particle_emitter.lifetime, 0.1..=5.0).text("Lifetime, с"));
                                                ui.add(egui::Slider::new(&mut particle_emitter.spread, 0.0..=150.0).text("Spread"));
                                                ui.add(egui::DragValue::new(&mut particle_emitter.position.x).prefix("X: ").speed(1.0));
                                                ui.add(egui::DragValue::new(&mut particle_emitter.position.y).prefix("Y: ").speed(1.0));
                                                ui.separator();
                                                ui.label(format!("Живых частиц: {}", particle_emitter.particles.len()));
                                            }
                                            RightTab::Bones => {
                                                ui.checkbox(&mut show_skeleton, "Показывать скелет на Stage");
                                                ui.separator();
                                                ui.label("Кости:");
                                                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                                                    let mut ids: Vec<String> = character.skeleton.bones.iter().map(|b| b.id.clone()).collect();
                                                    ids.sort();
                                                    for bid in ids {
                                                        let is_sel = selected_bone.as_deref() == Some(bid.as_str());
                                                        if ui.selectable_label(is_sel, &bid).clicked() {
                                                            selected_bone = Some(bid.clone());
                                                            bone_rename_buf = bid;
                                                        }
                                                    }
                                                });
                                                ui.separator();

                                                if ui.button(if placing_bone { "Кликни на Stage..." } else { "+ Добавить кость" }).clicked() {
                                                    placing_bone = !placing_bone;
                                                }
                                                ui.small("Новая кость встанет ребёнком выбранной (или Root), в точке клика.");

                                                match selected_bone.clone() {
                                                    Some(bid) if character.skeleton.find(&bid).is_some() => {
                                                        ui.separator();
                                                        ui.label(format!("Выбрана: {bid}"));

                                                        let is_root = character.skeleton.find(&bid).and_then(|b| b.parent.clone()).is_none();
                                                        if ui.add_enabled(!is_root, egui::Button::new("Удалить кость (и поддерево)")).clicked() {
                                                            let parent_id = character.skeleton.find(&bid).and_then(|b| b.parent.clone());
                                                            // Сохраняем мировые позиции частей, которые
                                                            // висели на удаляемых костях, ДО удаления —
                                                            // иначе их нечем будет пересчитать после.
                                                            let doomed: Vec<String> = character.skeleton.bones.iter().filter(|b| character.skeleton.is_descendant_of(&b.id, &bid)).map(|b| b.id.clone()).collect();
                                                            let keep_at: Vec<(String, glam::Vec2)> = character
                                                                .parts
                                                                .values()
                                                                .filter(|p| p.bone.as_ref().is_some_and(|b| doomed.contains(b)))
                                                                .filter_map(|p| {
                                                                    let w = character.skeleton.world_transform_with_ik(p.bone.as_ref()?)?;
                                                                    Some((p.id.clone(), pony_render::part_world_position(p, &w)))
                                                                })
                                                                .collect();
                                                            let removed = character.skeleton.remove_subtree(&bid);
                                                            if let Some(parent_id) = parent_id {
                                                                // Части удалённых костей переходят к родителю и
                                                                // остаются там же на экране (раньше смещение
                                                                // переносилось как есть, и часть прыгала).
                                                                let parent_world = character.skeleton.world_transform_with_ik(&parent_id).unwrap_or_default();
                                                                for (part_id, world_pos) in keep_at {
                                                                    if let Some(part) = character.parts.get_mut(&part_id) {
                                                                        part.bone = Some(parent_id.clone());
                                                                        part.pivot = pony_render::pivot_for_world_position(world_pos, &parent_world);
                                                                    }
                                                                }
                                                            }
                                                            // Дорожки анимаций на удалённые кости больше
                                                            // ни на что не влияют — оставляем как есть (не
                                                            // роняет рендер, просто "мёртвая" дорожка), не
                                                            // усложняем удаление ещё и чисткой анимаций.
                                                            selected_bone = None;
                                                            status_message = format!("Удалено костей: {}", removed.len());
                                                        }

                                                        ui.separator();
                                                        ui.label("Переименовать:");
                                                        ui.add(egui::TextEdit::singleline(&mut bone_rename_buf).desired_width(150.0));
                                                        if ui.button("Применить имя").clicked() {
                                                            if bone_rename_buf != bid && !bone_rename_buf.trim().is_empty() {
                                                                let new_id = bone_rename_buf.trim().to_string();
                                                                if character.skeleton.rename_bone(&bid, &new_id) {
                                                                    for part in character.parts.values_mut() {
                                                                        if part.bone.as_deref() == Some(bid.as_str()) {
                                                                            part.bone = Some(new_id.clone());
                                                                        }
                                                                    }
                                                                    for anim in character.animations.values_mut() {
                                                                        for track in &mut anim.tracks {
                                                                            if let pony_core::animation::AnimTarget::Bone { id, .. } = &mut track.target {
                                                                                if id == &bid {
                                                                                    *id = new_id.clone();
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                    selected_bone = Some(new_id.clone());
                                                                    status_message = format!("Кость '{bid}' -> '{new_id}'");
                                                                } else {
                                                                    status_message = format!("Не удалось переименовать: имя '{new_id}' занято");
                                                                }
                                                            }
                                                        }

                                                        ui.separator();
                                                        let current_parent = character.skeleton.find(&bid).and_then(|b| b.parent.clone());
                                                        ui.label("Родитель:");
                                                        let mut new_parent: Option<String> = None;
                                                        egui::ComboBox::from_id_source("bone_reparent")
                                                            .selected_text(current_parent.clone().unwrap_or_else(|| "(корень)".into()))
                                                            .show_ui(ui, |ui| {
                                                                for other in &character.skeleton.bones {
                                                                    if other.id == bid {
                                                                        continue;
                                                                    }
                                                                    if ui.selectable_label(current_parent.as_deref() == Some(other.id.as_str()), &other.id).clicked() {
                                                                        new_parent = Some(other.id.clone());
                                                                    }
                                                                }
                                                            });
                                                        if let Some(np) = new_parent {
                                                            if Some(&np) != current_parent.as_ref() {
                                                                let world_before = character.skeleton.world_transform(&bid);
                                                                if character.skeleton.reparent(&bid, &np) {
                                                                    // Кость остаётся на месте на экране: локальная поза
                                                                    // пересчитывается под нового родителя (раньше
                                                                    // старая локальная поза улетала вместе с ним).
                                                                    if let (Some(before), Some(parent_world)) = (world_before, character.skeleton.world_transform(&np)) {
                                                                        if let Some(bone) = character.skeleton.bones.iter_mut().find(|b| b.id == bid) {
                                                                            bone.local_transform.position = pony_render::pivot_for_world_position(before.position, &parent_world);
                                                                            bone.local_transform.rotation = before.rotation - parent_world.rotation;
                                                                        }
                                                                    }
                                                                    status_message = format!("'{bid}' теперь дочерняя для '{np}'");
                                                                } else {
                                                                    status_message = "Нельзя: это создало бы цикл в иерархии".into();
                                                                }
                                                            }
                                                        }

                                                        ui.separator();
                                                        ui.label("Локальная трансформация:");
                                                        if let Some(bone) = character.skeleton.bones.iter_mut().find(|b| b.id == bid) {
                                                            ui.add(egui::DragValue::new(&mut bone.local_transform.position.x).prefix("X: ").speed(0.5));
                                                            ui.add(egui::DragValue::new(&mut bone.local_transform.position.y).prefix("Y: ").speed(0.5));
                                                            let mut rot_deg = bone.local_transform.rotation.to_degrees();
                                                            if ui.add(egui::DragValue::new(&mut rot_deg).prefix("Rotate: ").suffix("°").speed(1.0)).changed() {
                                                                bone.local_transform.rotation = rot_deg.to_radians();
                                                            }
                                                            ui.add(egui::DragValue::new(&mut bone.local_transform.scale.x).prefix("Scale X: ").speed(0.01));
                                                            ui.add(egui::DragValue::new(&mut bone.local_transform.scale.y).prefix("Scale Y: ").speed(0.01));
                                                            ui.add(egui::DragValue::new(&mut bone.length).prefix("Длина: ").speed(0.5).clamp_range(0.0..=200.0));
                                                        }
                                                    }
                                                    _ => {
                                                        ui.separator();
                                                        ui.label("(кость не выбрана)");
                                                    }
                                                }

                                                // IK (раздел 41 ТЗ): Two Bone IK-констрейнты. Раньше
                                                // риггинг был только прямой (FK) — этот блок закрывает
                                                // крупнейший пробел, отмеченный в README ("риггинг без
                                                // инверсной кинематики"). Управляет тем же
                                                // `Skeleton::ik_constraints`, что реально читает
                                                // рендер (`world_transform_with_ik` в pony-render) —
                                                // не косметическая панель, двигает персонажа взаправду.
                                                ui.separator();
                                                ui.collapsing("IK (Two Bone)", |ui| {
                                                    ui.small("Раздел 41 ТЗ: цепочка Root->Mid->Tip тянется к точке Target. Пример: Hip->Knee->Hoof.");
                                                    let ik_ids: Vec<String> = character.skeleton.ik_constraints.iter().map(|c| c.id.clone()).collect();
                                                    for ik_id in &ik_ids {
                                                        let Some(idx) = character.skeleton.ik_constraints.iter().position(|c| &c.id == ik_id) else { continue };
                                                        ui.group(|ui| {
                                                            let constraint = &mut character.skeleton.ik_constraints[idx];
                                                            ui.horizontal(|ui| {
                                                                ui.strong(&constraint.id);
                                                                ui.checkbox(&mut constraint.enabled, "включен");
                                                            });
                                                            ui.small(format!("{} -> {} -> {}", constraint.root, constraint.mid, constraint.tip));
                                                            ui.add(egui::DragValue::new(&mut constraint.target.x).prefix("Target X: ").speed(0.5));
                                                            ui.add(egui::DragValue::new(&mut constraint.target.y).prefix("Target Y: ").speed(0.5));
                                                            ui.add(egui::Slider::new(&mut constraint.weight, 0.0..=1.0).text("Weight"));

                                                            let mut has_pole = constraint.pole_target.is_some();
                                                            if ui.checkbox(&mut has_pole, "Pole Target").changed() {
                                                                constraint.pole_target = if has_pole { Some(constraint.target + glam::Vec2::new(0.0, 20.0)) } else { None };
                                                            }
                                                            if let Some(pole) = constraint.pole_target.as_mut() {
                                                                ui.add(egui::DragValue::new(&mut pole.x).prefix("Pole X: ").speed(0.5));
                                                                ui.add(egui::DragValue::new(&mut pole.y).prefix("Pole Y: ").speed(0.5));
                                                            }
                                                            if ui.button("Удалить IK").clicked() {
                                                                ik_to_remove = Some(constraint.id.clone());
                                                            }
                                                        });
                                                    }

                                                    ui.separator();
                                                    ui.label("Новый IK-констрейнт:");
                                                    ui.horizontal(|ui| {
                                                        ui.label("Root:");
                                                        egui::ComboBox::from_id_source("ik_new_root").selected_text(ik_new_root.clone().unwrap_or_else(|| "-".into())).show_ui(ui, |ui| {
                                                            for b in &character.skeleton.bones {
                                                                if ui.selectable_label(ik_new_root.as_deref() == Some(b.id.as_str()), &b.id).clicked() {
                                                                    ik_new_root = Some(b.id.clone());
                                                                }
                                                            }
                                                        });
                                                    });
                                                    ui.horizontal(|ui| {
                                                        ui.label("Mid:");
                                                        egui::ComboBox::from_id_source("ik_new_mid").selected_text(ik_new_mid.clone().unwrap_or_else(|| "-".into())).show_ui(ui, |ui| {
                                                            for b in &character.skeleton.bones {
                                                                if ui.selectable_label(ik_new_mid.as_deref() == Some(b.id.as_str()), &b.id).clicked() {
                                                                    ik_new_mid = Some(b.id.clone());
                                                                }
                                                            }
                                                        });
                                                    });
                                                    ui.horizontal(|ui| {
                                                        ui.label("Tip:");
                                                        egui::ComboBox::from_id_source("ik_new_tip").selected_text(ik_new_tip.clone().unwrap_or_else(|| "-".into())).show_ui(ui, |ui| {
                                                            for b in &character.skeleton.bones {
                                                                if ui.selectable_label(ik_new_tip.as_deref() == Some(b.id.as_str()), &b.id).clicked() {
                                                                    ik_new_tip = Some(b.id.clone());
                                                                }
                                                            }
                                                        });
                                                    });
                                                    // Root/Mid/Tip должны быть тремя разными костями: при
                                                    // совпадении длина звена нулевая, и решатель IK
                                                    // раньше превращал позу цепочки в NaN (части пропадали).
                                                    let distinct = ik_new_root != ik_new_mid && ik_new_mid != ik_new_tip && ik_new_root != ik_new_tip;
                                                    let can_create = ik_new_root.is_some() && ik_new_mid.is_some() && ik_new_tip.is_some() && distinct;
                                                    if ik_new_root.is_some() && ik_new_mid.is_some() && ik_new_tip.is_some() && !distinct {
                                                        ui.colored_label(egui::Color32::from_rgb(230, 150, 90), "Root, Mid и Tip должны быть разными костями");
                                                    }
                                                    if ui.add_enabled(can_create, egui::Button::new("+ Создать IK-констрейнт")).clicked() {
                                                        let (root, mid, tip) = (ik_new_root.take().unwrap(), ik_new_mid.take().unwrap(), ik_new_tip.take().unwrap());
                                                        let tip_world = character.skeleton.world_transform_with_ik(&tip).unwrap_or_default();
                                                        let ik_id = unique_name(&format!("ik_{}", character.skeleton.ik_constraints.len() + 1), |c| character.skeleton.ik_constraints.iter().any(|k| k.id == c));
                                                        let mut constraint = pony_core::IkConstraint::new(ik_id, root, mid, tip);
                                                        // Target по умолчанию — текущая мировая позиция tip
                                                        // (нога не дёргается в момент создания констрейнта).
                                                        constraint.target = tip_world.position;
                                                        character.skeleton.add_ik_constraint(constraint);
                                                        status_message = "IK-констрейнт создан".into();
                                                    }
                                                });
                                                if let Some(id) = ik_to_remove.take() {
                                                    character.skeleton.remove_ik_constraint(&id);
                                                    status_message = format!("IK-констрейнт '{id}' удалён");
                                                }
                                            }
                                            // SVG Inspector (раздел 79 ТЗ) — специальная панель:
                                            // список ВСЕХ SVG-частей персонажа сразу, а не только
                                            // той, что выбрана на Stage/в Timeline. Раньше кнопка
                                            // "Редактировать SVG" была только в Properties и
                                            // появлялась лишь для уже выбранной части — легко
                                            // было не найти её вообще, если не знать заранее, что
                                            // искать. Здесь — явный, самодостаточный список.
                                            RightTab::SvgInspector => {
                                                let mut svg_parts: Vec<(String, String, i32, Option<String>)> = character
                                                    .parts
                                                    .iter()
                                                    .filter_map(|(id, p)| match &p.source {
                                                        pony_core::part::PartSource::Vector { path } => Some((id.clone(), path.clone(), p.layer, p.bone.clone())),
                                                        _ => None,
                                                    })
                                                    .collect();
                                                svg_parts.sort_by(|a, b| a.0.cmp(&b.0));

                                                ui.label(format!("SVG-частей в документе: {}", svg_parts.len()));
                                                ui.small("Раздел 79 ТЗ (\"SVG Inspector\"): структура, атрибуты и прямое редактирование SVG-частей персонажа.");
                                                ui.separator();

                                                if svg_parts.is_empty() {
                                                    ui.label("Ни одна часть персонажа пока не сделана из SVG.");
                                                    ui.small("Нарисуй фигуру любым инструментом (Rect/Oval/.../Pen) и нажми \"Сохранить как слой\" на левой панели — она появится здесь.");
                                                } else {
                                                    egui::ScrollArea::vertical().show(ui, |ui| {
                                                        for (id, path, layer, bone) in &svg_parts {
                                                            ui.group(|ui| {
                                                                ui.horizontal(|ui| {
                                                                    ui.strong(id);
                                                                    if editing_part.as_deref() == Some(id.as_str()) {
                                                                        ui.colored_label(egui::Color32::from_rgb(120, 220, 140), "(редактируется)");
                                                                    }
                                                                });
                                                                ui.small(format!("Файл: {path}"));
                                                                ui.small(format!("Кость: {} · Слой: {layer}", bone.as_deref().unwrap_or("(нет)")));
                                                                ui.horizontal(|ui| {
                                                                    if ui.button("Редактировать SVG").clicked() {
                                                                        status_message = start_editing_svg_part(id, path, &mut vector_doc, &mut editing_part, &mut active_tool, &mut selected_shape_index);
                                                                    }
                                                                    if ui.button("Выбрать на Stage").clicked() {
                                                                        selected_layer = Some(id.clone());
                                                                        right_tab = RightTab::Properties;
                                                                    }
                                                                });
                                                            });
                                                        }
                                                    });
                                                }
                                            }
                                            RightTab::Symbols => {
                                                ui.small("Раздел 28 ТЗ (\"Symbol — reusable object\") + раздел 95 (\"Symbol instance overrides\"): переиспользуемые объекты — правка определения сразу отражается на всех инстансах.");
                                                ui.separator();

                                                ui.label("Convert to Symbol");
                                                ui.horizontal(|ui| {
                                                    ui.text_edit_singleline(&mut symbol_name_buf);
                                                    let can_convert = selected_shape_index.map(|i| !matches!(vector_doc.shapes.get(i), Some(pony_core::VectorShape::Instance { .. }) | None)).unwrap_or(false);
                                                    if ui.add_enabled(can_convert && !symbol_name_buf.trim().is_empty(), egui::Button::new("Создать из выбранной фигуры")).clicked() {
                                                        if let Some(idx) = selected_shape_index {
                                                            if let Some(shape) = vector_doc.shapes.get(idx).cloned() {
                                                                let name = symbol_name_buf.trim().to_string();
                                                                vector_doc.upsert_symbol(name.clone(), vec![shape]);
                                                                // Раздел 28: исходная фигура на холсте заменяется
                                                                // инстансом нового символа (сама геометрия теперь
                                                                // живёт в SymbolDef, не дублируется на холсте) —
                                                                // identity-transform, т.к. фигура уже была в
                                                                // абсолютных координатах документа.
                                                                vector_doc.shapes[idx] = pony_core::VectorShape::Instance { symbol: name.clone(), transform: (1.0, 0.0, 0.0, 1.0, 0.0, 0.0), fill_override: None };
                                                                status_message = format!("Символ '{name}' создан, фигура заменена на инстанс");
                                                                symbol_name_buf.clear();
                                                            }
                                                        }
                                                    }
                                                });
                                                if selected_shape_index.is_none() {
                                                    ui.small("Выбери фигуру инструментом SubSelection на холсте, чтобы превратить её в символ.");
                                                }
                                                ui.separator();

                                                if vector_doc.symbols.is_empty() {
                                                    ui.label("Пока нет ни одного символа в этом документе.");
                                                } else {
                                                    ui.label(format!("Определения символов: {}", vector_doc.symbols.len()));
                                                    let mut place_instance: Option<String> = None;
                                                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                                                        for def in &vector_doc.symbols {
                                                            ui.group(|ui| {
                                                                ui.horizontal(|ui| {
                                                                    ui.strong(&def.name);
                                                                    ui.weak(format!("{} фигур(ы)", def.shapes.len()));
                                                                });
                                                                let instance_count = vector_doc.shapes.iter().filter(|s| matches!(s, pony_core::VectorShape::Instance { symbol, .. } if symbol == &def.name)).count();
                                                                ui.small(format!("Инстансов на холсте: {instance_count}"));
                                                                if ui.button("+ Разместить инстанс в центре").clicked() {
                                                                    place_instance = Some(def.name.clone());
                                                                }
                                                            });
                                                        }
                                                    });
                                                    if let Some(name) = place_instance {
                                                        // Центр текущего viewBox документа — разумная точка
                                                        // по умолчанию, дальше инстанс двигается как обычная
                                                        // фигура (единственный control point — точка
                                                        // переноса, см. VectorShape::control_points).
                                                        let (min_x, min_y, max_x, max_y) = vector_doc.bounds().unwrap_or((0.0, 0.0, 100.0, 100.0));
                                                        let center = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
                                                        if let Some(inst) = vector_doc.new_instance_centered_at(&name, center.0, center.1) {
                                                            vector_doc.add(inst);
                                                            selected_shape_index = Some(vector_doc.shapes.len() - 1);
                                                            active_tool = Tool::SubSelection;
                                                            status_message = format!("Инстанс символа '{name}' размещён");
                                                        }
                                                    }
                                                }
                                                ui.separator();

                                                let selected_is_instance = selected_shape_index.and_then(|i| vector_doc.shapes.get(i)).map(|s| matches!(s, pony_core::VectorShape::Instance { .. })).unwrap_or(false);
                                                if selected_is_instance {
                                                    ui.label("Выбранная фигура — Symbol Instance.");
                                                    if ui.button("Break Apart Symbol (разорвать связь)").clicked() {
                                                        if let Some(idx) = selected_shape_index {
                                                            if vector_doc.break_apart_symbol_instance(idx) {
                                                                status_message = "Инстанс разорван — теперь независимая копия геометрии".into();
                                                            }
                                                        }
                                                    }
                                                    ui.small("Раздел 95 ТЗ: после разрыва фигура(ы) больше не связаны с Symbol Definition — правка определения на них не влияет.");
                                                }
                                            }
                                            RightTab::Groups => {
                                                ui.small("Раздел 27 ТЗ (\"g является контейнером и может содержать другие g на произвольной глубине\"): организационные группы поверх частей персонажа — независимы от того, к какой кости каждая часть привязана.");
                                                ui.separator();

                                                if character.groups.groups.is_empty() {
                                                    ui.label("Пока нет ни одной группы.");
                                                    ui.small("Лассо выдели несколько частей на Stage, затем Edit -> Group (создать постоянную группу).");
                                                } else {
                                                    let mut ungroup_id: Option<String> = None;
                                                    egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                                                        // Сначала группы верхнего уровня, потом их дети — простой
                                                        // способ показать иерархию без полноценного дерева виджетов:
                                                        // отступ по глубине вложенности через `ui.indent`.
                                                        fn draw_group_and_children(
                                                            ui: &mut egui::Ui,
                                                            character: &mut pony_core::Character,
                                                            group_id: &str,
                                                            ungroup_id: &mut Option<String>,
                                                            depth: usize,
                                                        ) {
                                                            let Some(group) = character.groups.find(group_id).cloned() else { return };
                                                            let member_count = character.parts.values().filter(|p| p.group.as_deref() == Some(group_id)).count();
                                                            ui.horizontal(|ui| {
                                                                ui.add_space(depth as f32 * 16.0);
                                                                ui.strong(&group.name);
                                                                ui.weak(format!("({member_count} частей)"));
                                                            });
                                                            ui.horizontal(|ui| {
                                                                ui.add_space(depth as f32 * 16.0);
                                                                let mut hidden = group.hidden;
                                                                if ui.checkbox(&mut hidden, "Скрыта").changed() {
                                                                    if let Some(g) = character.groups.find_mut(group_id) {
                                                                        g.hidden = hidden;
                                                                    }
                                                                }
                                                                if ui.button("Ungroup").clicked() {
                                                                    *ungroup_id = Some(group_id.to_string());
                                                                }
                                                            });
                                                            let children: Vec<String> = character.groups.groups.iter().filter(|g| g.parent.as_deref() == Some(group_id)).map(|g| g.id.clone()).collect();
                                                            for child_id in children {
                                                                draw_group_and_children(ui, character, &child_id, ungroup_id, depth + 1);
                                                            }
                                                        }
                                                        let top_level: Vec<String> = character.groups.groups.iter().filter(|g| g.parent.is_none()).map(|g| g.id.clone()).collect();
                                                        for group_id in top_level {
                                                            ui.group(|ui| {
                                                                draw_group_and_children(ui, &mut character, &group_id, &mut ungroup_id, 0);
                                                            });
                                                        }
                                                    });
                                                    if let Some(id) = ungroup_id {
                                                        character.ungroup(&id);
                                                        status_message = "Группа разгруппирована — части остались на месте".into();
                                                    }
                                                }
                                            }
                                            RightTab::Gradients => {
                                                ui.small("Раздел 60 ТЗ (Gradients): именованные линейные и радиальные градиенты с произвольным числом цветовых остановок. Правка уже существующего градиента здесь сразу отражается на всех фигурах, которые ссылаются на него по имени (см. VectorDoc::upsert_gradient).");
                                                ui.separator();

                                                ui.label(match &gradient_editing_existing {
                                                    Some(name) => format!("Редактирование градиента '{name}':"),
                                                    None => "Новый градиент:".to_string(),
                                                });
                                                ui.horizontal(|ui| {
                                                    ui.label("Имя:");
                                                    ui.text_edit_singleline(&mut gradient_name_buf);
                                                });
                                                ui.horizontal(|ui| {
                                                    ui.selectable_value(&mut gradient_is_radial, false, "Linear");
                                                    ui.selectable_value(&mut gradient_is_radial, true, "Radial");
                                                });
                                                if gradient_is_radial {
                                                    ui.horizontal(|ui| {
                                                        ui.label("Центр:");
                                                        ui.add(egui::DragValue::new(&mut gradient_rad_cx).prefix("X: ").speed(1.0));
                                                        ui.add(egui::DragValue::new(&mut gradient_rad_cy).prefix("Y: ").speed(1.0));
                                                    });
                                                    ui.horizontal(|ui| {
                                                        ui.label("Радиус:");
                                                        ui.add(egui::DragValue::new(&mut gradient_rad_r).speed(1.0).clamp_range(0.01..=100000.0));
                                                    });
                                                } else {
                                                    ui.horizontal(|ui| {
                                                        ui.label("Точка 1:");
                                                        ui.add(egui::DragValue::new(&mut gradient_lin_x1).prefix("X: ").speed(1.0));
                                                        ui.add(egui::DragValue::new(&mut gradient_lin_y1).prefix("Y: ").speed(1.0));
                                                    });
                                                    ui.horizontal(|ui| {
                                                        ui.label("Точка 2:");
                                                        ui.add(egui::DragValue::new(&mut gradient_lin_x2).prefix("X: ").speed(1.0));
                                                        ui.add(egui::DragValue::new(&mut gradient_lin_y2).prefix("Y: ").speed(1.0));
                                                    });
                                                }
                                                ui.separator();
                                                ui.label(format!("Остановки цвета ({}):", gradient_stops.len()));
                                                let stops_len = gradient_stops.len();
                                                let mut remove_stop: Option<usize> = None;
                                                for (i, stop) in gradient_stops.iter_mut().enumerate() {
                                                    ui.horizontal(|ui| {
                                                        ui.add(egui::DragValue::new(&mut stop.offset).speed(0.01).clamp_range(0.0..=1.0).prefix("offset: "));
                                                        let mut c = rgba_to_color32(stop.color);
                                                        if ui.color_edit_button_srgba(&mut c).changed() {
                                                            stop.color = color32_to_rgba(c);
                                                        }
                                                        if ui.add_enabled(stops_len > 1, egui::Button::new("✕")).clicked() {
                                                            remove_stop = Some(i);
                                                        }
                                                    });
                                                }
                                                if let Some(i) = remove_stop {
                                                    gradient_stops.remove(i);
                                                }
                                                if ui.button("+ Добавить остановку").clicked() {
                                                    let offset = gradient_stops.last().map(|s| (s.offset + 1.0) / 2.0).unwrap_or(0.5).clamp(0.0, 1.0);
                                                    gradient_stops.push(pony_core::GradientStop { offset, color: pony_core::RgbaColor::new(128, 128, 128, 255) });
                                                }
                                                ui.separator();
                                                ui.horizontal(|ui| {
                                                    let can_save = !gradient_name_buf.trim().is_empty() && !gradient_stops.is_empty();
                                                    if ui.add_enabled(can_save, egui::Button::new("💾 Сохранить градиент")).clicked() {
                                                        let kind = if gradient_is_radial {
                                                            pony_core::GradientKind::Radial { cx: gradient_rad_cx, cy: gradient_rad_cy, r: gradient_rad_r }
                                                        } else {
                                                            pony_core::GradientKind::Linear { x1: gradient_lin_x1, y1: gradient_lin_y1, x2: gradient_lin_x2, y2: gradient_lin_y2 }
                                                        };
                                                        let name = gradient_name_buf.trim().to_string();
                                                        vector_doc.upsert_gradient(pony_core::GradientDef::new(name.clone(), kind, gradient_stops.clone()));
                                                        // Обновить fill-плейсхолдер у всех фигур, ссылающихся на
                                                        // этот градиент по имени (в т.ч. если правили уже
                                                        // существующий) — та же пост-обработка, что делает
                                                        // from_svg_str после парсинга.
                                                        vector_doc.sync_gradient_fallback_colors();
                                                        status_message = format!("Градиент '{name}' сохранён");
                                                        gradient_editing_existing = Some(name);
                                                    }
                                                    if ui.button("Новый (очистить форму)").clicked() {
                                                        gradient_editing_existing = None;
                                                        gradient_name_buf.clear();
                                                        gradient_is_radial = false;
                                                        gradient_lin_x1 = 0.0;
                                                        gradient_lin_y1 = 0.0;
                                                        gradient_lin_x2 = 100.0;
                                                        gradient_lin_y2 = 0.0;
                                                        gradient_rad_cx = 50.0;
                                                        gradient_rad_cy = 50.0;
                                                        gradient_rad_r = 50.0;
                                                        gradient_stops = vec![
                                                            pony_core::GradientStop { offset: 0.0, color: pony_core::RgbaColor::new(255, 255, 255, 255) },
                                                            pony_core::GradientStop { offset: 1.0, color: pony_core::RgbaColor::new(0, 0, 0, 255) },
                                                        ];
                                                    }
                                                });
                                                ui.separator();

                                                let mut load_into_form: Option<String> = None;
                                                let mut delete_gradient: Option<String> = None;
                                                let mut apply_to_selected: Option<String> = None;
                                                if vector_doc.gradients.is_empty() {
                                                    ui.label("Пока нет ни одного градиента в этом документе.");
                                                } else {
                                                    ui.label(format!("Определения градиентов: {}", vector_doc.gradients.len()));
                                                    egui::ScrollArea::vertical().max_height(260.0).id_source("gradients_list").show(ui, |ui| {
                                                        for def in &vector_doc.gradients {
                                                            ui.group(|ui| {
                                                                ui.horizontal(|ui| {
                                                                    ui.strong(&def.name);
                                                                    ui.weak(match def.kind {
                                                                        pony_core::GradientKind::Linear { .. } => "Linear",
                                                                        pony_core::GradientKind::Radial { .. } => "Radial",
                                                                    });
                                                                    ui.weak(format!("{} стопов", def.stops.len()));
                                                                });
                                                                ui.horizontal(|ui| {
                                                                    if ui.button("Загрузить в форму").clicked() {
                                                                        load_into_form = Some(def.name.clone());
                                                                    }
                                                                    if ui.button("Удалить").clicked() {
                                                                        delete_gradient = Some(def.name.clone());
                                                                    }
                                                                    let can_apply = selected_shape_index
                                                                        .and_then(|i| vector_doc.shapes.get(i))
                                                                        .map(|s| !matches!(s, pony_core::VectorShape::Instance { .. } | pony_core::VectorShape::Line { .. } | pony_core::VectorShape::Polyline { .. }))
                                                                        .unwrap_or(false);
                                                                    if ui.add_enabled(can_apply, egui::Button::new("Применить к выбранной фигуре")).clicked() {
                                                                        apply_to_selected = Some(def.name.clone());
                                                                    }
                                                                });
                                                            });
                                                        }
                                                    });
                                                }
                                                if selected_shape_index.is_none() {
                                                    ui.small("Выбери фигуру инструментом SubSelection на холсте, чтобы применить к ней градиент.");
                                                }

                                                if let Some(name) = load_into_form {
                                                    if let Some(def) = vector_doc.find_gradient(&name) {
                                                        gradient_name_buf = def.name.clone();
                                                        match def.kind {
                                                            pony_core::GradientKind::Linear { x1, y1, x2, y2 } => {
                                                                gradient_is_radial = false;
                                                                gradient_lin_x1 = x1;
                                                                gradient_lin_y1 = y1;
                                                                gradient_lin_x2 = x2;
                                                                gradient_lin_y2 = y2;
                                                            }
                                                            pony_core::GradientKind::Radial { cx, cy, r } => {
                                                                gradient_is_radial = true;
                                                                gradient_rad_cx = cx;
                                                                gradient_rad_cy = cy;
                                                                gradient_rad_r = r;
                                                            }
                                                        }
                                                        gradient_stops = def.stops.clone();
                                                        gradient_editing_existing = Some(name.clone());
                                                        status_message = format!("Градиент '{name}' загружен в форму для редактирования");
                                                    }
                                                }
                                                if let Some(name) = delete_gradient {
                                                    vector_doc.remove_gradient(&name);
                                                    if gradient_editing_existing.as_deref() == Some(name.as_str()) {
                                                        gradient_editing_existing = None;
                                                    }
                                                    status_message = format!("Градиент '{name}' удалён (фигуры, ссылавшиеся на него, откатились на плоский fill)");
                                                }
                                                if let Some(name) = apply_to_selected {
                                                    if let Some(idx) = selected_shape_index {
                                                        if let Some(def) = vector_doc.find_gradient(&name).cloned() {
                                                            let avg = def.average_color();
                                                            if let Some(shape) = vector_doc.shapes.get_mut(idx) {
                                                                shape.set_fill_gradient(Some(name.clone()), avg);
                                                                status_message = format!("Градиент '{name}' применён к выбранной фигуре");
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            RightTab::Boolean => {
                                                ui.small("Раздел 44 ТЗ (Modify -> \"Combine Paths\"): Union/Difference/Intersection/XOR/Divide между двумя фигурами документа. Обе исходные фигуры заменяются результатом (одним или несколькими новыми Polygon) — не восстанавливаются.");
                                                ui.separator();

                                                let candidates: Vec<(usize, String)> = vector_doc
                                                    .shapes
                                                    .iter()
                                                    .enumerate()
                                                    .filter(|(_, s)| pony_core::flatten_shape_to_contour(s).is_some())
                                                    .map(|(i, s)| (i, format!("#{i} {}", shape_kind_label(s))))
                                                    .collect();

                                                if candidates.len() < 2 {
                                                    ui.label("Нужно минимум 2 подходящие фигуры на холсте (Rect/Ellipse/Polygon/замкнутый Path — не Line/Polyline/Instance).");
                                                } else {
                                                    let front_label = boolean_front_idx.and_then(|i| candidates.iter().find(|(ci, _)| *ci == i)).map(|(_, l)| l.clone()).unwrap_or_else(|| "-".to_string());
                                                    ui.horizontal(|ui| {
                                                        ui.label("Верхняя фигура (front):");
                                                        egui::ComboBox::from_id_source("boolean_front").selected_text(front_label).show_ui(ui, |ui| {
                                                            for (i, label) in &candidates {
                                                                ui.selectable_value(&mut boolean_front_idx, Some(*i), label);
                                                            }
                                                        });
                                                    });
                                                    let back_label = boolean_back_idx.and_then(|i| candidates.iter().find(|(ci, _)| *ci == i)).map(|(_, l)| l.clone()).unwrap_or_else(|| "-".to_string());
                                                    ui.horizontal(|ui| {
                                                        ui.label("Нижняя фигура (back):");
                                                        egui::ComboBox::from_id_source("boolean_back").selected_text(back_label).show_ui(ui, |ui| {
                                                            for (i, label) in &candidates {
                                                                ui.selectable_value(&mut boolean_back_idx, Some(*i), label);
                                                            }
                                                        });
                                                    });
                                                    ui.horizontal(|ui| {
                                                        ui.label("Операция:");
                                                        egui::ComboBox::from_id_source("boolean_op").selected_text(boolean_op_label(boolean_op_kind)).show_ui(ui, |ui| {
                                                            for op in [
                                                                pony_core::BooleanOp::Union,
                                                                pony_core::BooleanOp::Difference,
                                                                pony_core::BooleanOp::Intersection,
                                                                pony_core::BooleanOp::Xor,
                                                                pony_core::BooleanOp::Divide,
                                                            ] {
                                                                ui.selectable_value(&mut boolean_op_kind, op, boolean_op_label(op));
                                                            }
                                                        });
                                                    });
                                                    ui.small(match boolean_op_kind {
                                                        pony_core::BooleanOp::Union => "Объединить обе фигуры в одну (или несколько, если не пересекаются).",
                                                        pony_core::BooleanOp::Difference => "Вычесть нижнюю фигуру из верхней.",
                                                        pony_core::BooleanOp::Intersection => "Оставить только область пересечения.",
                                                        pony_core::BooleanOp::Xor => "Оставить только непересекающиеся части — каждая сохраняет цвет своей исходной фигуры.",
                                                        pony_core::BooleanOp::Divide => "Разрезать обе фигуры на все непересекающиеся куски (включая перекрытие) — как Illustrator/Animate Divide.",
                                                    });

                                                    let same_shape = boolean_front_idx.is_some() && boolean_front_idx == boolean_back_idx;
                                                    if same_shape {
                                                        ui.colored_label(egui::Color32::from_rgb(230, 150, 90), "Верхняя и нижняя фигура не могут совпадать.");
                                                    }
                                                    let can_run = boolean_front_idx.is_some() && boolean_back_idx.is_some() && !same_shape;
                                                    if ui.add_enabled(can_run, egui::Button::new("Выполнить")).clicked() {
                                                        if let (Some(fi), Some(bi)) = (boolean_front_idx, boolean_back_idx) {
                                                            if let (Some(front), Some(back)) = (vector_doc.shapes.get(fi).cloned(), vector_doc.shapes.get(bi).cloned()) {
                                                                if let Some(result) = pony_core::boolean_op(&front, &back, boolean_op_kind) {
                                                                    // Удаляем исходные фигуры по убыванию индекса,
                                                                    // чтобы удаление одной не сдвинуло индекс другой
                                                                    // (тот же приём, что и везде при батч-удалении
                                                                    // из Vec по нескольким индексам).
                                                                    let mut to_remove = [fi, bi];
                                                                    to_remove.sort_unstable();
                                                                    vector_doc.shapes.remove(to_remove[1]);
                                                                    vector_doc.shapes.remove(to_remove[0]);
                                                                    let piece_count = result.pieces.len();
                                                                    for piece in &result.pieces {
                                                                        vector_doc.shapes.push(pony_core::piece_to_shape(piece));
                                                                    }
                                                                    boolean_front_idx = None;
                                                                    boolean_back_idx = None;
                                                                    selected_shape_index = None;
                                                                    status_message = if result.dropped_holes > 0 {
                                                                        format!("Boolean-операция: получено {piece_count} кусков(а), но {} внутренних контуров (дырок) отброшено — VectorShape::Polygon не хранит дырки", result.dropped_holes)
                                                                    } else if piece_count == 0 {
                                                                        "Boolean-операция: результат пуст (фигуры не пересекаются либо взаимно уничтожились)".to_string()
                                                                    } else {
                                                                        format!("Boolean-операция выполнена: получено {piece_count} куска(ов)")
                                                                    };
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    });
                                }

                                // --- окно импорта Adobe Animate ---
                                if xfl_window_open {
                                    let mut open = true;
                                    egui::Window::new("Импорт Adobe Animate").open(&mut open).default_width(640.0).default_pos(egui::pos2(220.0, 70.0)).show(ctx, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label("Файл .fla / .xfl / папка XFL:");
                                            let r = ui.add(egui::TextEdit::singleline(&mut xfl_path).desired_width(260.0).hint_text("путь к .fla / .xfl"));
                                            if ui.button("Обзор…").clicked() {
                                                file_dialog.show_for(file_dialog::Purpose::ImportAnimate, (!xfl_path.is_empty()).then(|| std::path::Path::new(xfl_path.as_str())), "");
                                            }
                                            if ui.button("Загрузить").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                                                xfl_load_request = true;
                                            }
                                        });
                                        if !xfl_info.is_empty() {
                                            ui.small(&xfl_info);
                                        }
                                        let Some(doc) = &xfl_doc else {
                                            ui.weak("Выбери файл кнопкой «Обзор…» (или впиши путь и нажми «Загрузить»).");
                                            return;
                                        };
                                        ui.separator();
                                        ui.horizontal_top(|ui| {
                                            // Левая колонка: символы библиотеки.
                                            ui.vertical(|ui| {
                                                ui.set_width(230.0);
                                                ui.add(egui::TextEdit::singleline(&mut xfl_filter).hint_text("поиск символа…"));
                                                egui::ScrollArea::vertical().id_source("xfl_symbols").max_height(380.0).show(ui, |ui| {
                                                    if ui.selectable_label(xfl_symbol.is_none(), format!("(главная шкала, {} кадр.)", doc.main.frame_count())).clicked() {
                                                        xfl_symbol = None;
                                                        xfl_frame = 0;
                                                    }
                                                    let filter = xfl_filter.to_lowercase();
                                                    // Контейнеры "~..." (персонаж, голова) — первыми.
                                                    let mut names = doc.symbol_names();
                                                    names.sort_by_key(|n| (!n.starts_with('~'), n.to_lowercase()));
                                                    for name in names.into_iter().filter(|n| filter.is_empty() || n.to_lowercase().contains(&filter)) {
                                                        let frames = doc.symbols[name].frame_count();
                                                        if ui.selectable_label(xfl_symbol.as_deref() == Some(name), format!("{name}  ·{frames}")).clicked() {
                                                            xfl_symbol = Some(name.to_string());
                                                            xfl_frame = 0;
                                                        }
                                                    }
                                                });
                                            });
                                            ui.separator();
                                            // Правая колонка: кадр, превью, параметры.
                                            ui.vertical(|ui| {
                                                let Some(tl) = doc.timeline(xfl_symbol.as_deref()) else { return };
                                                let count = tl.frame_count();
                                                xfl_frame = xfl_frame.min(count - 1);
                                                ui.horizontal(|ui| {
                                                    ui.label("Кадр:");
                                                    ui.add(egui::Slider::new(&mut xfl_frame, 0..=count - 1));
                                                    if ui.small_button("◀").clicked() {
                                                        xfl_frame = xfl_frame.saturating_sub(1);
                                                    }
                                                    if ui.small_button("▶").clicked() {
                                                        xfl_frame = (xfl_frame + 1).min(count - 1);
                                                    }
                                                });
                                                let labels = tl.frame_labels();
                                                if !labels.is_empty() {
                                                    ui.horizontal_wrapped(|ui| {
                                                        ui.weak("Метки:");
                                                        for (idx, label) in &labels {
                                                            if ui.small_button(format!("{label} ({idx})")).clicked() {
                                                                xfl_frame = *idx;
                                                            }
                                                        }
                                                    });
                                                }
                                                // Превью перерисовывается только при смене символа/кадра.
                                                let key = (xfl_symbol.clone(), xfl_frame);
                                                if xfl_preview_key.as_ref() != Some(&key) {
                                                    xfl_preview_key = Some(key);
                                                    xfl_preview = doc
                                                        .render_svg(xfl_symbol.as_deref(), xfl_frame)
                                                        .and_then(|r| pony_render::rasterize_svg_rgba(&r.svg, 360))
                                                        .map(|(w, h, rgba)| ctx.load_texture("xfl-preview", egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba), egui::TextureOptions::LINEAR));
                                                }
                                                let (rect, _) = ui.allocate_exact_size(egui::vec2(370.0, 300.0), egui::Sense::hover());
                                                ui.painter().rect_filled(rect, 4.0, egui::Color32::from_gray(235));
                                                match &xfl_preview {
                                                    Some(tex) => {
                                                        let size = tex.size_vec2();
                                                        let k = (rect.width() / size.x).min(rect.height() / size.y).min(1.0);
                                                        let img = egui::Rect::from_center_size(rect.center(), size * k);
                                                        ui.painter().image(tex.id(), img, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                                                    }
                                                    None => {
                                                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "(пустой кадр)", egui::FontId::proportional(13.0), egui::Color32::from_gray(90));
                                                    }
                                                }
                                                ui.horizontal(|ui| {
                                                    ui.label("Масштаб:");
                                                    ui.add(egui::DragValue::new(&mut xfl_opts.scale).speed(0.01).clamp_range(0.01..=10.0)).on_hover_text("Мировых единиц на пиксель Animate");
                                                    ui.checkbox(&mut xfl_opts.expand_containers, "Раскрыть «~»-контейнеры")
                                                        .on_hover_text("Вложенные символы с именем на «~» (например ~RD_Head) раскладываются на отдельные части: глаза, рот, брови…");
                                                });
                                                ui.small("Каждый слой станет частью со своей костью в точке трансформации Animate — её можно двигать, вращать и анимировать.");
                                                if ui.button("Импортировать как нового персонажа").clicked() {
                                                    actions.push(Action::ImportXfl);
                                                }
                                            });
                                        });
                                    });
                                    xfl_window_open = open;
                                }

                                if let Some(result) = file_dialog.ui(ctx) {
                                    dialog_result = Some(result);
                                }

                                // Timeline — TopBottomPanel::bottom, но добавлена ЗДЕСЬ,
                                // после боковых панелей, а не раньше них. Раньше стояла
                                // перед SidePanel::left/right — из-за чего при своей
                                // раскладке получала полную ширину окна (боковые панели
                                // ещё не "откусили" себе место), и часть её собственных
                                // виджетов (слайдер зума) оказывалась под координатами,
                                // где позже поверх рисовалась левая панель инструментов, и
                                // пряталась под ней — не баг слайдера, баг порядка панелей.
                                // Тот же класс проблемы уже был с левой/нижней панелью
                                // раньше в этом проекте — правило "top -> bottom -> side ->
                                // central" здесь нарушалось конкретно для этой панели.
                                if show_timeline {
                                    egui::TopBottomPanel::bottom("timeline").min_height(160.0).resizable(true).show(ctx, |ui| {
                                        // --- переключатель персонажей сцены + режим таймлайна ---
                                        let mut switch_to: Option<usize> = None; // индекс в other_characters
                                        ui.horizontal(|ui| {
                                            ui.label("Timeline:");
                                            ui.selectable_value(&mut timeline_mode_all, false, "Персонаж");
                                            ui.selectable_value(&mut timeline_mode_all, true, "Все");
                                            ui.separator();
                                            if ui.button("+ Новый персонаж").clicked() {
                                                // Текущий активный уходит в список остальных, на
                                                // его место встаёт свежий пустой персонаж — та же
                                                // логика "своп", что и при обычном переключении,
                                                // просто с новым слотом вместо существующего.
                                                new_character_count += 1;
                                                let new_name = format!("Pony {new_character_count}");
                                                other_characters.push(CharacterSlot {
                                                    character: std::mem::replace(&mut character, empty_character()),
                                                    player: std::mem::replace(&mut player, AnimationPlayer::new()),
                                                    name: active_character_name.clone(),
                                                });
                                                active_character_name = new_name;
                                                selected_layer = None;
                                                selected_bone = None;
                                                history.reset(&character, &vector_doc);
                                                fit_request = Some(FitView::Character);
                                                status_message = format!("Создан персонаж '{active_character_name}'");
                                            }
                                            if ui.add_enabled(!other_characters.is_empty(), egui::Button::new("Удалить активного")).clicked() {
                                                if let Some(next) = other_characters.pop() {
                                                    character = next.character;
                                                    player = next.player;
                                                    let deleted_name = std::mem::replace(&mut active_character_name, next.name);
                                                    selected_layer = None;
                                                    selected_bone = None;
                                                    history.reset(&character, &vector_doc);
                                                    status_message = format!("Персонаж '{deleted_name}' удалён, активен '{active_character_name}'");
                                                }
                                            }
                                        });

                                        if timeline_mode_all {
                                            ui.separator();
                                            ui.small("Клик по строке — сделать персонажа активным и войти в его таймлайн (Character).");
                                            egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                                                let playing_mark = if playing { "▶" } else { "⏸" };
                                                ui.label(format!("{playing_mark} {active_character_name} (активен) — {} частей · {} костей", character.parts.len(), character.skeleton.bones.len()));
                                                character_timeline_strip(ui, &character, &player, true);
                                                ui.add_space(4.0);

                                                for (i, slot) in other_characters.iter().enumerate() {
                                                    let playing_mark = if slot.player.current_name().is_some() { "▶" } else { "⏸" };
                                                    ui.label(format!("{playing_mark} {} — {} частей · {} костей", slot.name, slot.character.parts.len(), slot.character.skeleton.bones.len()));
                                                    if character_timeline_strip(ui, &slot.character, &slot.player, false).clicked() {
                                                        switch_to = Some(i);
                                                    }
                                                    ui.add_space(4.0);
                                                }
                                            });
                                        } else {
                                            ui.label(format!("Timeline — {active_character_name}"));
                                            timeline_widget(
                                                ui,
                                                &mut character,
                                                &mut player,
                                                &mut playing,
                                                &mut hidden_layers,
                                                &mut locked_layers,
                                                &mut selected_layer,
                                                &mut timeline_zoom,
                                                &mut selected_keyframe,
                                                &mut timeline_channel,
                                                &selected_bone,
                                            );
                                        }

                                        // Своп персонажа-по-клику из режима "Все" — тот же приём,
                                        // что и в "+ Новый"/"Удалить": текущий активный уходит в
                                        // список, выбранный оттуда встаёт на его место.
                                        if let Some(i) = switch_to {
                                            let picked = other_characters.remove(i);
                                            other_characters.push(CharacterSlot {
                                                character: std::mem::replace(&mut character, picked.character),
                                                player: std::mem::replace(&mut player, picked.player),
                                                name: std::mem::replace(&mut active_character_name, picked.name),
                                            });
                                            selected_layer = None;
                                            selected_bone = None;
                                            timeline_mode_all = false; // drill-down: сразу в таймлайн выбранного
                                            history.reset(&character, &vector_doc);
                                            status_message = format!("Активен '{active_character_name}'");
                                        }
                                    });
                                }

                                egui::CentralPanel::default().show(ctx, |ui| {
                                    let avail = ui.available_rect_before_wrap();
                                    let ruler = if show_rulers { 16.0 } else { 0.0 };
                                    let canvas_rect = egui::Rect::from_min_max(egui::pos2(avail.left() + ruler, avail.top() + ruler), avail.max);
                                    let response = ui.interact(canvas_rect, ui.id().with("stage_canvas"), egui::Sense::click_and_drag());
                                    // ВАЖНО: painter_at(avail), не голый ui.painter() — иначе при
                                    // увеличении/панорамировании Stage сцена и оверлеи (линии
                                    // скелета, превью фигур) рисуются БЕЗ обрезки по границам
                                    // CentralPanel и протекают поверх меню и палитры инструментов.
                                    let painter = ui.painter_at(avail);
                                    let painter = &painter;

                                    painter.rect_filled(avail, 0.0, egui::Color32::from_gray(38));

                                    // --- навигация по Stage ---
                                    // Пан: инструмент Hand ИЛИ средняя кнопка мыши в любом
                                    // инструменте (раньше, чтобы сдвинуть вид, приходилось
                                    // каждый раз переключаться на Hand и обратно).
                                    if (active_tool == Tool::Hand && response.dragged_by(egui::PointerButton::Primary)) || response.dragged_by(egui::PointerButton::Middle) {
                                        stage_pan += response.drag_delta();
                                    }
                                    // Зум колесом (и Ctrl+колесо / жестом) — К КУРСОРУ, а не к
                                    // центру: точка под мышью остаётся на месте. Раньше зум шёл к
                                    // центру холста, и мелкую деталь приходилось ловить паном.
                                    let zoom_around = |stage_zoom: &mut f32, stage_pan: &mut egui::Vec2, factor: f32, anchor: egui::Pos2| {
                                        let new_zoom = (*stage_zoom * factor).clamp(MIN_STAGE_ZOOM, MAX_STAGE_ZOOM);
                                        let k = new_zoom / *stage_zoom;
                                        let from_center = anchor - canvas_rect.center();
                                        *stage_pan = from_center - (from_center - *stage_pan) * k;
                                        *stage_zoom = new_zoom;
                                    };
                                    if let Some(hover) = response.hover_pos() {
                                        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
                                        let factor = pinch * (scroll * 0.0015).exp();
                                        if (factor - 1.0).abs() > 1e-4 {
                                            zoom_around(&mut stage_zoom, &mut stage_pan, factor, hover);
                                        }
                                    }
                                    if active_tool == Tool::Zoom {
                                        if let Some(pos) = response.interact_pointer_pos() {
                                            if response.clicked() {
                                                zoom_around(&mut stage_zoom, &mut stage_pan, 1.25, pos);
                                            } else if response.secondary_clicked() {
                                                zoom_around(&mut stage_zoom, &mut stage_pan, 1.0 / 1.25, pos);
                                            }
                                        }
                                    }
                                    if let Some(fit) = fit_request.take() {
                                        match fit {
                                            FitView::Document => {
                                                stage_zoom = (canvas_rect.width() / SCENE_WIDTH as f32).min(canvas_rect.height() / SCENE_HEIGHT as f32) * 0.95;
                                                stage_pan = egui::Vec2::ZERO;
                                            }
                                            FitView::Character => {
                                                if let Some((lo, hi)) = character_bounds(&character, &hidden_layers) {
                                                    let size = (hi - lo).max(glam::Vec2::splat(1.0)) * camera.zoom.max(0.0001);
                                                    stage_zoom = ((canvas_rect.width() / size.x).min(canvas_rect.height() / size.y) * 0.6).clamp(MIN_STAGE_ZOOM, MAX_STAGE_ZOOM);
                                                    // Центр габарита -> центр холста (та же формула, что
                                                    // world_to_screen ниже, только решённая относительно пана).
                                                    let c = (lo + hi) / 2.0 - camera.position;
                                                    let (sr, cr) = camera.rotation.sin_cos();
                                                    let v = glam::Vec2::new(c.x * cr + c.y * sr, -c.x * sr + c.y * cr) * camera.zoom * stage_zoom;
                                                    stage_pan = egui::vec2(-v.x, v.y);
                                                } else {
                                                    // Персонаж пуст — показываем весь документ.
                                                    stage_zoom = (canvas_rect.width() / SCENE_WIDTH as f32).min(canvas_rect.height() / SCENE_HEIGHT as f32) * 0.95;
                                                    stage_pan = egui::Vec2::ZERO;
                                                }
                                            }
                                        }
                                    }
                                    stage_zoom = stage_zoom.clamp(MIN_STAGE_ZOOM, MAX_STAGE_ZOOM);

                                    let stage_w = SCENE_WIDTH as f32 * stage_zoom;
                                    let stage_h = SCENE_HEIGHT as f32 * stage_zoom;
                                    let center = canvas_rect.center() + stage_pan;
                                    let stage_rect = egui::Rect::from_center_size(center, egui::vec2(stage_w, stage_h));

                                    // --- рендер сцены в разрешении экрана ---
                                    // Кадр покрывает весь видимый холст, а камера для него —
                                    // камера документа, домноженная на зум Stage и сдвинутая
                                    // на пан. Геометрически это ровно то же отображение мир->экран,
                                    // что и раньше (см. screen_to_world/world_to_screen ниже), но
                                    // растеризованное в пикселях экрана: персонаж чёткий на любом
                                    // зуме, а не растянутая картинка 960x720.
                                    let ppp = ctx.pixels_per_point();
                                    let render_w = ((canvas_rect.width() * ppp).round() as u32).clamp(1, 4096);
                                    let render_h = ((canvas_rect.height() * ppp).round() as u32).clamp(1, 4096);
                                    let image_rect = egui::Rect::from_min_size(canvas_rect.min, egui::vec2(render_w as f32, render_h as f32) / ppp);
                                    let mut view_camera = camera;
                                    {
                                        let s = camera.zoom.max(0.0001) * stage_zoom;
                                        let d_screen = center - image_rect.center();
                                        let d = glam::Vec2::new(d_screen.x, -d_screen.y) / s;
                                        let (sr, cr) = camera.rotation.sin_cos();
                                        view_camera.position = camera.position - glam::Vec2::new(d.x * cr - d.y * sr, d.x * sr + d.y * cr);
                                        view_camera.zoom = s * ppp;
                                    }
                                    let up_to_date = last_render_key.as_ref().is_some_and(|k| {
                                        k.size == (render_w, render_h) && k.camera == view_camera && k.lighting == lighting && k.hidden == hidden_layers && k.character == character
                                    });
                                    let animated_fx = particles_enabled || camera.shake_intensity > 0.0 || blink_timer > 0.0;
                                    if !up_to_date || animated_fx || force_render {
                                        force_render = false;
                                        let mut visible_character = character.clone();
                                        // Часть скрыта, если скрыта ОНА САМА или группа, в
                                        // которую она входит (по всей цепочке родителей, см.
                                        // `Character::is_part_hidden_by_group`).
                                        visible_character.parts.retain(|id, _| !hidden_layers.contains(id) && !character.is_part_hidden_by_group(id));
                                        if blink_timer > 0.0 {
                                            // 0 -> 1 (закрыт) -> 0 за время моргания
                                            let phase = 1.0 - blink_timer / BLINK_SECONDS;
                                            let closed = (phase * std::f32::consts::PI).sin();
                                            let current = visible_character.default_morph.get("Blink");
                                            visible_character.default_morph.set("Blink", current.max(closed));
                                        }
                                        let particles_arg = if particles_enabled { Some(&particle_emitter) } else { None };
                                        let rendered = pony_renderer.render_character(&gpu_ctx, &visible_character, render_w, render_h, &view_camera, elapsed_time, &lighting, particles_arg);
                                        let color_image = egui::ColorImage::from_rgba_unmultiplied([rendered.width as usize, rendered.height as usize], &rendered.rgba);
                                        match &mut scene_texture {
                                            Some(handle) => handle.set(color_image, egui::TextureOptions::LINEAR),
                                            None => scene_texture = Some(ctx.load_texture("pony-scene", color_image, egui::TextureOptions::LINEAR)),
                                        }
                                        last_frame_output = Some(rendered);
                                        last_render_key = Some(RenderKey { character: character.clone(), hidden: hidden_layers.clone(), camera: view_camera, lighting: lighting.clone(), size: (render_w, render_h) });
                                    }
                                    scene_image_rect = image_rect;

                                    // Сцена рисуется ПЕРВОЙ — до всех блоков инструментов:
                                    // живое превью фигуры (Rect/Oval/Line/Pencil) рисуется прямо
                                    // внутри этих блоков и должно ложиться ПОВЕРХ сцены.
                                    if let Some(tex) = &scene_texture {
                                        painter.image(tex.id(), image_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                                    }
                                    // Всё вне экспортной области документа притемнено — видно,
                                    // что попадёт в GIF/спрайт-лист, а что нет.
                                    let outside = egui::Color32::from_rgba_unmultiplied(30, 30, 34, 200);
                                    let c = canvas_rect;
                                    let st = stage_rect.intersect(c);
                                    if st.is_positive() {
                                        painter.rect_filled(egui::Rect::from_min_max(c.min, egui::pos2(c.right(), st.top())), 0.0, outside);
                                        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(c.left(), st.bottom()), c.max), 0.0, outside);
                                        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(c.left(), st.top()), egui::pos2(st.left(), st.bottom())), 0.0, outside);
                                        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(st.right(), st.top()), egui::pos2(c.right(), st.bottom())), 0.0, outside);
                                    } else {
                                        painter.rect_filled(c, 0.0, outside);
                                    }

                                    // Перевод экранных координат в мировые координаты сцены —
                                    // обратное преобразование той же камеры (position/rotation/zoom/
                                    // shake), что и в Renderer::render_character (см. compute_projection
                                    // в pony-render), только в обратную сторону. Не инвертируем сам
                                    // 2.5D-поворот (facing_yaw) — сравниваем клик с уже повёрнутыми
                                    // координатами частей (см. part_at), так проще и корректно.
                                    let inv_zoom = 1.0 / camera.zoom.max(0.0001);
                                    let shake = camera.shake_offset(elapsed_time);
                                    let (sin_r, cos_r) = camera.rotation.sin_cos();
                                    let screen_to_world = |screen_pos: egui::Pos2| -> glam::Vec2 {
                                        let frame_px_x = (screen_pos.x - stage_rect.left()) / stage_zoom;
                                        let frame_px_y = (screen_pos.y - stage_rect.top()) / stage_zoom;
                                        let after_cam_x = frame_px_x - SCENE_WIDTH as f32 / 2.0;
                                        let after_cam_y = SCENE_HEIGHT as f32 / 2.0 - frame_px_y;
                                        let scaled = glam::Vec2::new(after_cam_x, after_cam_y) * inv_zoom;
                                        let rotated = glam::Vec2::new(scaled.x * cos_r - scaled.y * sin_r, scaled.x * sin_r + scaled.y * cos_r);
                                        rotated + glam::Vec2::new(camera.position.x + shake.x, camera.position.y + shake.y)
                                    };
                                    // Обратное преобразование screen_to_world — нужно, чтобы рисовать
                                    // превью фигур и уже нарисованные VectorDoc-шейпы на экране в тех
                                    // же координатах, что видит пользователь (с учётом камеры/зума/пана).
                                    let world_to_screen = |world_pos: glam::Vec2| -> egui::Pos2 {
                                        let relative = world_pos - glam::Vec2::new(camera.position.x + shake.x, camera.position.y + shake.y);
                                        let unrotated =
                                            glam::Vec2::new(relative.x * cos_r + relative.y * sin_r, -relative.x * sin_r + relative.y * cos_r);
                                        let after_cam = unrotated * camera.zoom.max(0.0001);
                                        let frame_px_x = after_cam.x + SCENE_WIDTH as f32 / 2.0;
                                        let frame_px_y = SCENE_HEIGHT as f32 / 2.0 - after_cam.y;
                                        egui::pos2(stage_rect.left() + frame_px_x * stage_zoom, stage_rect.top() + frame_px_y * stage_zoom)
                                    };
                                    // Точка, где РЕАЛЬНО нажали кнопку мыши. Для начала drag'а
                                    // это важно: egui признаёт жест драгом только после порога
                                    // сдвига, и `interact_pointer_pos()` в этот момент уже уехал —
                                    // прямоугольники начинались не там, где нажал, а лассо и
                                    // захват узлов промахивались.
                                    let press_origin = ui.input(|i| i.pointer.press_origin());

                                    if active_tool == Tool::Selection {
                                        // Клик — выбрать часть под курсором (или снять выбор).
                                        if response.clicked() {
                                            if let Some(screen_pos) = response.interact_pointer_pos() {
                                                selected_layer = part_at(&character, &hidden_layers, &locked_layers, screen_to_world(screen_pos));
                                            }
                                        }

                                        // Перетаскивание — двигать САМУ ЧАСТЬ (её смещение
                                        // относительно кости), а не кость (это делает Xform).
                                        // Раньше тащилась ПРЕДЫДУЩАЯ выбранная часть, даже если
                                        // нажал на другую, и её центр прыгал под курсор. Теперь
                                        // на старте драга выбирается часть под точкой нажатия, а
                                        // смещение захвата сохраняется — часть едет за мышью
                                        // ровно с того места, за которое взяли.
                                        if response.drag_started_by(egui::PointerButton::Primary) {
                                            grab_offset = None;
                                            if let Some(origin) = press_origin {
                                                let press_world = screen_to_world(origin);
                                                let hit = part_at(&character, &hidden_layers, &locked_layers, press_world);
                                                if hit.is_some() {
                                                    selected_layer = hit;
                                                }
                                                if let Some(part) = selected_layer.as_ref().filter(|id| !locked_layers.contains(*id)).and_then(|id| character.parts.get(id)) {
                                                    if let Some(world) = part.bone.as_ref().and_then(|b| character.skeleton.world_transform_with_ik(b)) {
                                                        grab_offset = Some(pony_render::part_world_position(part, &world) - press_world);
                                                    }
                                                }
                                            }
                                        }
                                        if response.dragged_by(egui::PointerButton::Primary) {
                                            if let (Some(id), Some(offset), Some(pos)) = (selected_layer.clone(), grab_offset, response.interact_pointer_pos()) {
                                                let target = screen_to_world(pos) + offset;
                                                let bone_id = character.parts.get(&id).and_then(|p| p.bone.clone());
                                                if let Some(bone_id) = bone_id {
                                                    let bone_world = character.skeleton.world_transform_with_ik(&bone_id).unwrap_or_default();
                                                    let new_pivot = pony_render::pivot_for_world_position(target, &bone_world);
                                                    if let Some(p) = character.parts.get_mut(&id) {
                                                        p.pivot = new_pivot;
                                                    }
                                                }
                                            }
                                        }
                                        if response.drag_stopped_by(egui::PointerButton::Primary) {
                                            grab_offset = None;
                                        }
                                    }

                                    if active_tool == Tool::FreeTransform {
                                        // Что двигаем/вращаем — решается в момент нажатия:
                                        // точка кости (если скелет показан) -> эта кость;
                                        // часть -> её кость, с Alt — кость-родитель (у
                                        // импортированных из Animate паппетов это контейнер:
                                        // Alt+клик по глазу берёт всю голову); пустое место —
                                        // кость выбранного слоя. Цель — `selected_bone`.
                                        if response.drag_started_by(egui::PointerButton::Primary) || response.clicked() {
                                            if let Some(origin) = press_origin.or(response.interact_pointer_pos()) {
                                                let alt = ui.input(|i| i.modifiers.alt);
                                                let bone_hit = if show_skeleton {
                                                    character
                                                        .skeleton
                                                        .bones
                                                        .iter()
                                                        .filter_map(|b| character.skeleton.world_transform_with_ik(&b.id).map(|w| (b.id.clone(), world_to_screen(w.position).distance(origin))))
                                                        .filter(|(_, d)| *d < 8.0)
                                                        .min_by(|a, b| a.1.total_cmp(&b.1))
                                                        .map(|(id, _)| id)
                                                } else {
                                                    None
                                                };
                                                if let Some(b) = bone_hit {
                                                    selected_bone = Some(b);
                                                } else {
                                                    if let Some(hit) = part_at(&character, &hidden_layers, &locked_layers, screen_to_world(origin)) {
                                                        selected_layer = Some(hit);
                                                    }
                                                    let mut bone = selected_layer.as_ref().and_then(|id| character.parts.get(id)).and_then(|p| p.bone.clone());
                                                    // Уже выбран предок этой кости (например, вся голова
                                                    // через Alt) — остаёмся на нём: тащить голову можно
                                                    // за любой её элемент, как группу в Animate.
                                                    let mut ancestor = bone.as_ref().and_then(|b| character.skeleton.find(b)).and_then(|b| b.parent.clone());
                                                    let mut guard = 0;
                                                    while let Some(a) = ancestor.clone() {
                                                        if selected_bone.as_deref() == Some(a.as_str()) && character.skeleton.find(&a).is_some_and(|b| b.parent.is_some()) {
                                                            bone = Some(a);
                                                            break;
                                                        }
                                                        ancestor = character.skeleton.find(&a).and_then(|b| b.parent.clone());
                                                        guard += 1;
                                                        if guard > 256 {
                                                            break;
                                                        }
                                                    }
                                                    if alt {
                                                        if let Some(parent) = bone.as_ref().and_then(|b| character.skeleton.find(b)).and_then(|b| b.parent.clone()) {
                                                            if character.skeleton.find(&parent).is_some_and(|p| p.parent.is_some()) {
                                                                bone = Some(parent);
                                                            }
                                                        }
                                                    }
                                                    if bone.is_some() {
                                                        selected_bone = bone;
                                                    }
                                                }
                                                if let Some(b) = &selected_bone {
                                                    status_message = format!("Xform: кость '{b}' (тащить — двигать, Shift — вращать, Alt+клик — кость-родитель)");
                                                }
                                            }
                                        }
                                        let layer_locked = selected_layer.as_ref().is_some_and(|id| locked_layers.contains(id));
                                        if response.dragged_by(egui::PointerButton::Primary) && ui.input(|i| i.modifiers.shift) && !layer_locked {
                                            // Shift — поворот вокруг точки кости (в Animate — угол
                                            // Free Transform). Угол между прошлой и текущей позицией
                                            // курсора относительно кости, в мировых координатах.
                                            if let (Some(bone_id), Some(pos)) = (selected_bone.clone(), response.interact_pointer_pos()) {
                                                if let Some(center) = character.skeleton.world_transform_with_ik(&bone_id).map(|w| w.position) {
                                                    let prev = screen_to_world(pos - response.drag_delta()) - center;
                                                    let cur = screen_to_world(pos) - center;
                                                    if prev.length() > 1e-3 && cur.length() > 1e-3 {
                                                        let mut da = cur.y.atan2(cur.x) - prev.y.atan2(prev.x);
                                                        if da > std::f32::consts::PI {
                                                            da -= std::f32::consts::TAU;
                                                        } else if da < -std::f32::consts::PI {
                                                            da += std::f32::consts::TAU;
                                                        }
                                                        if let Some(bone) = character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                                            bone.local_transform.rotation += da;
                                                        }
                                                    }
                                                }
                                            }
                                        } else if response.dragged_by(egui::PointerButton::Primary) && !layer_locked {
                                            if let Some(bone_id) = selected_bone.clone().filter(|b| character.skeleton.find(b).is_some()) {
                                                let delta_screen = response.drag_delta();
                                                let delta_frame = glam::Vec2::new(delta_screen.x, -delta_screen.y) / stage_zoom;
                                                let delta_after_cam = delta_frame * inv_zoom;
                                                let delta_world = glam::Vec2::new(
                                                    delta_after_cam.x * cos_r - delta_after_cam.y * sin_r,
                                                    delta_after_cam.x * sin_r + delta_after_cam.y * cos_r,
                                                );
                                                // Локальная позиция кости — в пространстве РОДИТЕЛЯ:
                                                // сдвиг в мире переводим через мировую трансформацию
                                                // родителя (раньше мировой сдвиг прибавлялся напрямую, и
                                                // кость повёрнутого/отмасштабированного родителя уезжала
                                                // не туда, куда тянули).
                                                let parent_world = character.skeleton.find(&bone_id).and_then(|b| b.parent.clone()).and_then(|p| character.skeleton.world_transform_with_ik(&p));
                                                let bone_world = character.skeleton.world_transform_with_ik(&bone_id);
                                                if let Some(bone) = character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                                    match (parent_world, bone_world) {
                                                        (Some(parent_world), Some(bone_world)) => {
                                                            bone.local_transform.position = pony_render::pivot_for_world_position(bone_world.position + delta_world, &parent_world);
                                                        }
                                                        _ => bone.local_transform.position += delta_world,
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if active_tool == Tool::Eyedropper && response.clicked() {
                                        if let (Some(screen_pos), Some(frame)) = (response.interact_pointer_pos(), &last_frame_output) {
                                            // Пиксель кадра под курсором: кадр покрывает
                                            // `scene_image_rect` в разрешении экрана.
                                            let fx = ((screen_pos.x - scene_image_rect.left()) * ppp).floor() as i32;
                                            let fy = ((screen_pos.y - scene_image_rect.top()) * ppp).floor() as i32;
                                            if fx >= 0 && fy >= 0 && (fx as u32) < frame.width && (fy as u32) < frame.height {
                                                let idx = ((fy as u32 * frame.width + fx as u32) * 4) as usize;
                                                fill_color = egui::Color32::from_rgba_unmultiplied(frame.rgba[idx], frame.rgba[idx + 1], frame.rgba[idx + 2], frame.rgba[idx + 3]);
                                                status_message = format!("Пипетка: Fill = #{:02x}{:02x}{:02x}", frame.rgba[idx], frame.rgba[idx + 1], frame.rgba[idx + 2]);
                                            }
                                        }
                                    }

                                    // --- добавить кость кликом по Stage ---
                                    if placing_bone && response.clicked() {
                                        if let Some(screen_pos) = response.interact_pointer_pos() {
                                            let target = screen_to_world(screen_pos);
                                            let parent_id = selected_bone.clone().unwrap_or_else(|| "Root".to_string());
                                            if let Some(parent_world) = character.skeleton.world_transform_with_ik(&parent_id) {
                                                bone_count += 1;
                                                let new_id = unique_name(&format!("Bone_{bone_count}"), |c| character.skeleton.find(c).is_some());
                                                // Та же математика, что и у частей (compose() в Skeleton
                                                // устроен идентично part_world_position) — клик даёт мировую
                                                // точку, а хранить нужно смещение относительно родителя.
                                                let local_pos = pony_render::pivot_for_world_position(target, &parent_world);
                                                character.skeleton.add_bone(pony_core::skeleton::Bone {
                                                    id: new_id.clone(),
                                                    parent: Some(parent_id),
                                                    local_transform: pony_core::skeleton::Transform2D { position: local_pos, ..Default::default() },
                                                    length: 8.0,
                                                });
                                                selected_bone = Some(new_id.clone());
                                                bone_rename_buf = new_id.clone();
                                                status_message = format!("Кость '{new_id}' добавлена");
                                            } else {
                                                status_message = "Родительская кость не найдена".into();
                                            }
                                        }
                                        placing_bone = false;
                                    }

                                    // --- PaintBucket/InkBottle: перекрасить уже нарисованную (ещё
                                    // не сохранённую как часть) фигуру на холсте ---
                                    if matches!(active_tool, Tool::PaintBucket | Tool::InkBottle) && response.clicked() {
                                        if let Some(screen_pos) = response.interact_pointer_pos() {
                                            let w = screen_to_world(screen_pos);
                                            if let Some(idx) = vector_doc.shape_at(w.x, -w.y) {
                                                if let Some(shape) = vector_doc.shapes.get_mut(idx) {
                                                    if active_tool == Tool::PaintBucket {
                                                        shape.set_fill(color32_to_rgba(fill_color));
                                                    } else {
                                                        shape.set_stroke(color32_to_rgba(stroke_color));
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // --- Pen: клик добавляет точку пути; клик рядом с первой точкой
                                    // (и минимум 3 точки) замыкает как заливаемый Polygon; в отличие
                                    // от Pencil/Brush — это дискретные клики, не перетаскивание ---
                                    if active_tool == Tool::Pen && response.clicked() {
                                        if let Some(screen_pos) = response.interact_pointer_pos() {
                                            let w = screen_to_world(screen_pos);
                                            let svg_pt = (w.x, -w.y);
                                            let close_enough_to_start = pen_points.first().map(|(sx, sy)| {
                                                let dx = sx - svg_pt.0;
                                                let dy = sy - svg_pt.1;
                                                (dx * dx + dy * dy).sqrt() < 10.0 / (stage_zoom * camera.zoom.max(0.0001))
                                            });
                                            if close_enough_to_start == Some(true) && pen_points.len() >= 3 {
                                                vector_doc.add(pony_core::VectorShape::Polygon {
                                                    points: std::mem::take(&mut pen_points),
                                                    fill: color32_to_rgba(fill_color),
                                                    stroke: color32_to_rgba(stroke_color),
                                                    stroke_width: 1.5,
                                                    fill_gradient: None,
                                                });
                                                status_message = "Путь Pen замкнут в многоугольник".into();
                                            } else {
                                                pen_points.push(svg_pt);
                                            }
                                        }
                                    }
                                    // Живое превью пути Pen — отрезки между уже поставленными точками
                                    // плюс маркеры-узлы, чтобы видеть, где кликать для замыкания.
                                    if pen_points.len() >= 1 {
                                        let screen_pts: Vec<egui::Pos2> = pen_points.iter().map(|(x, y)| world_to_screen(glam::Vec2::new(*x, -*y))).collect();
                                        if screen_pts.len() >= 2 {
                                            painter.add(egui::Shape::line(screen_pts.clone(), egui::Stroke::new(1.5_f32, stroke_color)));
                                        }
                                        for p in &screen_pts {
                                            painter.circle_filled(*p, 3.0, egui::Color32::from_rgb(120, 200, 255));
                                        }
                                    }

                                    // --- SubSelection: клик выбирает уже нарисованную (не
                                    // сохранённую) фигуру на холсте, дальше её "узлы"
                                    // (control points) можно тащить, меняя саму форму.
                                    if active_tool == Tool::SubSelection {
                                        if response.drag_started_by(egui::PointerButton::Primary) {
                                            // Сначала проверяем, не начали ли тащить УЖЕ
                                            // выбранный узел (приоритет над выбором новой
                                            // фигуры — иначе перетаскивание узла около края
                                            // другой фигуры перевыбирало бы фигуру вместо
                                            // редактирования точки).
                                            dragging_point_index = None;
                                            if let (Some(shape_idx), Some(pos)) = (selected_shape_index, press_origin) {
                                                if let Some(shape) = vector_doc.shapes.get(shape_idx) {
                                                    let handle_radius_world = 8.0 / (stage_zoom * camera.zoom.max(0.0001));
                                                    for (i, (px, py)) in shape.control_points().iter().enumerate() {
                                                        let handle_world = screen_to_world(pos);
                                                        let dx = px - handle_world.x;
                                                        let dy = -py - handle_world.y;
                                                        if (dx * dx + dy * dy).sqrt() < handle_radius_world {
                                                            dragging_point_index = Some(i);
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        if response.dragged_by(egui::PointerButton::Primary) {
                                            if let (Some(shape_idx), Some(point_idx), Some(pos)) = (selected_shape_index, dragging_point_index, response.interact_pointer_pos()) {
                                                let w = screen_to_world(pos);
                                                if let Some(shape) = vector_doc.shapes.get_mut(shape_idx) {
                                                    shape.set_control_point(point_idx, (w.x, -w.y));
                                                }
                                            }
                                        }
                                        if response.drag_stopped_by(egui::PointerButton::Primary) {
                                            dragging_point_index = None;
                                        }
                                        if response.clicked() && dragging_point_index.is_none() {
                                            if let Some(pos) = response.interact_pointer_pos() {
                                                let w = screen_to_world(pos);
                                                selected_shape_index = vector_doc.shape_at(w.x, -w.y);
                                            }
                                        }
                                        // Узлы выбранной фигуры — рисуем поверх её собственного
                                        // превью, чтобы было видно, за что можно тащить.
                                        if let Some(shape_idx) = selected_shape_index {
                                            if let Some(shape) = vector_doc.shapes.get(shape_idx) {
                                                for (px, py) in shape.control_points() {
                                                    let p = world_to_screen(glam::Vec2::new(px, -py));
                                                    painter.rect_filled(egui::Rect::from_center_size(p, egui::vec2(7.0, 7.0)), 1.0, egui::Color32::WHITE);
                                                    painter.rect_stroke(egui::Rect::from_center_size(p, egui::vec2(7.0, 7.0)), 1.0, egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(60, 140, 220)));
                                                }
                                            }
                                        }
                                    }

                                    // --- Lasso: рамкой выделить сразу несколько частей
                                    // персонажа (по их мировой позиции — та же формула,
                                    // что и у остальных hit-тестов в этом файле) ---
                                    if active_tool == Tool::Lasso {
                                        if response.drag_started_by(egui::PointerButton::Primary) {
                                            lasso_start = press_origin.map(screen_to_world);
                                        }
                                        if response.dragged_by(egui::PointerButton::Primary) {
                                            if let (Some(start), Some(cur_screen)) = (lasso_start, response.interact_pointer_pos()) {
                                                let start_screen = world_to_screen(start);
                                                painter.rect_stroke(
                                                    egui::Rect::from_two_pos(start_screen, cur_screen),
                                                    0.0,
                                                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(120, 230, 140)),
                                                );
                                            }
                                        }
                                        if response.drag_stopped_by(egui::PointerButton::Primary) {
                                            if let (Some(start), Some(end_screen)) = (lasso_start.take(), response.interact_pointer_pos()) {
                                                let end = screen_to_world(end_screen);
                                                let (min_x, max_x) = (start.x.min(end.x), start.x.max(end.x));
                                                let (min_y, max_y) = (start.y.min(end.y), start.y.max(end.y));
                                                multi_selected.clear();
                                                for (id, part) in &character.parts {
                                                    let Some(bone_id) = &part.bone else { continue };
                                                    let Some(world) = character.skeleton.world_transform_with_ik(bone_id) else { continue };
                                                    let pos = pony_render::part_world_position(part, &world);
                                                    if pos.x >= min_x && pos.x <= max_x && pos.y >= min_y && pos.y <= max_y {
                                                        multi_selected.insert(id.clone());
                                                    }
                                                }
                                                status_message = format!("Лассо: выделено частей {}", multi_selected.len());
                                            }
                                        }
                                    }
                                    // Обводка группового выделения — зелёная, чтобы не путать
                                    // с обычным (синим) выделением одной части.
                                    for id in &multi_selected {
                                        if let Some(part) = character.parts.get(id) {
                                            if let Some(world) = part.bone.as_ref().and_then(|b| character.skeleton.world_transform_with_ik(b)) {
                                                let pos = pony_render::part_world_position(part, &world);
                                                let size = pony_render::part_render_size(part);
                                                let half_w = size.x * world.scale.x.abs() / 2.0;
                                                let half_h = size.y * world.scale.y.abs() / 2.0;
                                                let p0 = world_to_screen(glam::Vec2::new(pos.x - half_w, pos.y - half_h));
                                                let p1 = world_to_screen(glam::Vec2::new(pos.x + half_w, pos.y + half_h));
                                                painter.rect_stroke(egui::Rect::from_two_pos(p0, p1), 0.0, egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(120, 230, 140)));
                                            }
                                        }
                                    }

                                    // --- рисование фигур (раздел 16 ТЗ: SVG теперь не только импорт) ---
                                    let is_shape_tool = matches!(active_tool, Tool::Rectangle | Tool::Oval | Tool::Line | Tool::PolyStar);
                                    if is_shape_tool {
                                        if response.drag_started_by(egui::PointerButton::Primary) {
                                            draw_start = press_origin.map(screen_to_world);
                                        }
                                        if response.dragged_by(egui::PointerButton::Primary) {
                                            // Живое превью — рисуем прямо в экранных координатах поверх
                                            // сцены, ничего пока не коммитим в vector_doc.
                                            if let (Some(start_world), Some(cur_screen)) = (draw_start, response.interact_pointer_pos()) {
                                                let start_screen = world_to_screen(start_world);
                                                let preview_stroke = egui::Stroke::new(1.5_f32, stroke_color);
                                                match active_tool {
                                                    Tool::Rectangle => {
                                                        painter.rect_stroke(egui::Rect::from_two_pos(start_screen, cur_screen), 0.0, preview_stroke);
                                                    }
                                                    Tool::Oval => {
                                                        // Эллипс, а не круг по большей стороне — превью
                                                        // должно совпадать с тем, что будет создано.
                                                        let center = start_screen.lerp(cur_screen, 0.5);
                                                        let (rx, ry) = ((cur_screen.x - start_screen.x).abs() / 2.0, (cur_screen.y - start_screen.y).abs() / 2.0);
                                                        let pts: Vec<egui::Pos2> = (0..48)
                                                            .map(|i| {
                                                                let a = i as f32 / 48.0 * std::f32::consts::TAU;
                                                                center + egui::vec2(a.cos() * rx, a.sin() * ry)
                                                            })
                                                            .collect();
                                                        painter.add(egui::Shape::closed_line(pts, preview_stroke));
                                                    }
                                                    Tool::Line => {
                                                        painter.line_segment([start_screen, cur_screen], preview_stroke);
                                                    }
                                                    Tool::PolyStar => {
                                                        let r = (cur_screen - start_screen).length();
                                                        let pts: Vec<egui::Pos2> = (0..poly_sides)
                                                            .map(|i| {
                                                                let a = (i as f32) / (poly_sides as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                                                                start_screen + egui::vec2(a.cos() * r, a.sin() * r)
                                                            })
                                                            .collect();
                                                        if pts.len() >= 3 {
                                                            painter.add(egui::Shape::closed_line(pts, preview_stroke));
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                        if response.drag_stopped_by(egui::PointerButton::Primary) {
                                            if let (Some(start_world), Some(end_screen)) = (draw_start.take(), response.interact_pointer_pos()) {
                                                let end_world = screen_to_world(end_screen);
                                                let fill = color32_to_rgba(fill_color);
                                                let stroke = color32_to_rgba(stroke_color);
                                                let shape = match active_tool {
                                                    Tool::Rectangle => Some(pony_core::VectorShape::Rect {
                                                        x: start_world.x.min(end_world.x),
                                                        // SVG Y растёт вниз, мировая — вверх: верхний край
                                                        // прямоугольника (наименьший SVG y) — это БОЛЬШИЙ
                                                        // мировой y. Скобки здесь существенны: без них
                                                        // `-a.max(b)` — это `-(a.max(b))` только если `-`
                                                        // применяется к результату .max(), но при разных
                                                        // знаках операндов (мировой y может быть
                                                        // отрицательным) порядок вычисления даёт другой
                                                        // результат — отдельно взятая ошибка, которая была
                                                        // здесь и делала нарисованные прямоугольники
                                                        // невидимыми (уезжали в произвольную позицию).
                                                        y: -(start_world.y.max(end_world.y)),
                                                        w: (end_world.x - start_world.x).abs(),
                                                        h: (end_world.y - start_world.y).abs(),
                                                        fill,
                                                        stroke,
                                                        stroke_width: 1.5,
                                                        fill_gradient: None,
                                                    }),
                                                    Tool::Oval => {
                                                        let rx = (end_world.x - start_world.x).abs() / 2.0;
                                                        let ry = (end_world.y - start_world.y).abs() / 2.0;
                                                        Some(pony_core::VectorShape::Ellipse {
                                                            cx: (start_world.x + end_world.x) / 2.0,
                                                            cy: -(start_world.y + end_world.y) / 2.0,
                                                            rx: rx.max(1.0),
                                                            ry: ry.max(1.0),
                                                            fill,
                                                            stroke,
                                                            stroke_width: 1.5,
                                                            fill_gradient: None,
                                                        })
                                                    }
                                                    Tool::Line => Some(pony_core::VectorShape::Line {
                                                        x1: start_world.x,
                                                        y1: -start_world.y,
                                                        x2: end_world.x,
                                                        y2: -end_world.y,
                                                        stroke,
                                                        stroke_width: 2.0,
                                                    }),
                                                    Tool::PolyStar => {
                                                        let r = (end_world - start_world).length().max(1.0);
                                                        // Считаем в мировых координатах (Y вверх), потом
                                                        // переводим в SVG (Y вниз) при сборе точек — та же
                                                        // логика инверсии, что и у остальных фигур в этом файле.
                                                        let points: Vec<(f32, f32)> = (0..poly_sides)
                                                            .map(|i| {
                                                                let a = (i as f32) / (poly_sides as f32) * std::f32::consts::TAU + std::f32::consts::FRAC_PI_2;
                                                                let wx = start_world.x + a.cos() * r;
                                                                let wy = start_world.y + a.sin() * r;
                                                                (wx, -wy)
                                                            })
                                                            .collect();
                                                        Some(pony_core::VectorShape::Polygon { points, fill, stroke, stroke_width: 1.5, fill_gradient: None })
                                                    }
                                                    _ => None,
                                                };
                                                if let Some(shape) = shape {
                                                    vector_doc.add(shape);
                                                }
                                            }
                                        }
                                    }

                                    if matches!(active_tool, Tool::Pencil | Tool::Brush) {
                                        if response.drag_started_by(egui::PointerButton::Primary) {
                                            pencil_points.clear();
                                            if let Some(origin) = press_origin {
                                                let w = screen_to_world(origin);
                                                pencil_points.push((w.x, -w.y));
                                            }
                                        }
                                        if response.dragged_by(egui::PointerButton::Primary) {
                                            if let Some(pos) = response.interact_pointer_pos() {
                                                let w = screen_to_world(pos);
                                                pencil_points.push((w.x, -w.y));
                                            }
                                        }
                                        if response.drag_stopped_by(egui::PointerButton::Primary) && pencil_points.len() < 2 {
                                            pencil_points.clear();
                                        }
                                        if response.drag_stopped_by(egui::PointerButton::Primary) && pencil_points.len() >= 2 {
                                            vector_doc.add(pony_core::VectorShape::Polyline {
                                                points: std::mem::take(&mut pencil_points),
                                                stroke: color32_to_rgba(stroke_color),
                                                stroke_width: if active_tool == Tool::Brush { 4.0 } else { 1.5 },
                                            });
                                        }
                                        // Живое превью мазка, пока рисуется.
                                        if pencil_points.len() >= 2 {
                                            let screen_pts: Vec<egui::Pos2> =
                                                pencil_points.iter().map(|(x, y)| world_to_screen(glam::Vec2::new(*x, -*y))).collect();
                                            painter.add(egui::Shape::line(screen_pts, egui::Stroke::new(2.0_f32, stroke_color)));
                                        }
                                    }

                                    // Уже нарисованные фигуры — поверх сцены, в тех же экранных
                                    // координатах, что и всё остальное на Stage. Symbol Instance
                                    // (раздел 28 ТЗ) не рисуется напрямую — резолвится в
                                    // конкретные фигуры символа (с уже применённым transform и
                                    // fill_override этого инстанса, см. `resolve_symbol_instance`)
                                    // и рисуется ТОЙ ЖЕ функцией превью, что и обычные фигуры —
                                    // инстанс на Stage должен выглядеть неотличимо от инлайновой
                                    // копии той же геометрии.
                                    for shape in &vector_doc.shapes {
                                        match shape {
                                            pony_core::VectorShape::Instance { symbol, transform, fill_override } => {
                                                for resolved in pony_core::resolve_symbol_instance(&vector_doc, symbol, *transform, *fill_override, 8) {
                                                    draw_vector_shape_preview(painter, &resolved, &vector_doc, &world_to_screen);
                                                }
                                            }
                                            other => draw_vector_shape_preview(painter, other, &vector_doc, &world_to_screen),
                                        }
                                    }

                                    // Визуализация скелета — без неё костями просто нечем было бы
                                    // управлять: непонятно, где они, что чей родитель, что выбрано.
                                    if show_skeleton {
                                        for bone in &character.skeleton.bones {
                                            let Some(world) = character.skeleton.world_transform_with_ik(&bone.id) else { continue };
                                            let p = world_to_screen(glam::Vec2::new(world.position.x, world.position.y));
                                            let is_sel = selected_bone.as_deref() == Some(bone.id.as_str());
                                            let color = if is_sel { egui::Color32::from_rgb(255, 210, 60) } else { egui::Color32::from_rgb(120, 190, 255) };
                                            if let Some(parent_id) = &bone.parent {
                                                if let Some(parent_world) = character.skeleton.world_transform_with_ik(parent_id) {
                                                    let pp = world_to_screen(glam::Vec2::new(parent_world.position.x, parent_world.position.y));
                                                    painter.line_segment([pp, p], egui::Stroke::new(1.5_f32, color.gamma_multiply(0.7)));
                                                }
                                            }
                                            painter.circle_filled(p, if is_sel { 5.0 } else { 3.5 }, color);
                                            if is_sel {
                                                painter.circle_stroke(p, 8.0, egui::Stroke::new(1.5_f32, color));
                                            }
                                        }
                                    }
                                    if placing_bone {
                                        if let Some(hover) = response.hover_pos() {
                                            painter.circle_stroke(hover, 6.0, egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(255, 210, 60)));
                                        }
                                    }

                                    // Обводка выбранной части — иначе непонятно, что именно
                                    // сейчас двигаешь и попал ли клик туда, куда целился.
                                    if let Some(sel_id) = &selected_layer {
                                        if let Some(part) = character.parts.get(sel_id) {
                                            if let Some(world) = part.bone.as_ref().and_then(|b| character.skeleton.world_transform_with_ik(b)) {
                                                let part_pos = pony_render::part_world_position(part, &world);
                                                let depth_z = pony_render::part_depth(&character, part);
                                                let (yawed_x, foreshorten) = pony_core::apply_yaw_2_5d(part_pos.x, depth_z, character.facing_yaw);
                                                let size = pony_render::part_render_size(part);
                                                let half_w = size.x * world.scale.x.abs() * foreshorten / 2.0;
                                                let half_h = size.y * world.scale.y.abs() / 2.0;
                                                let p0 = world_to_screen(glam::Vec2::new(yawed_x - half_w, part_pos.y - half_h));
                                                let p1 = world_to_screen(glam::Vec2::new(yawed_x + half_w, part_pos.y + half_h));
                                                let rect = egui::Rect::from_two_pos(p0, p1);
                                                painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(90, 170, 255)));
                                                // Точка привязки к кости — видно, вокруг чего
                                                // часть будет вращаться при повороте кости.
                                                let bone_screen = world_to_screen(glam::Vec2::new(world.position.x, world.position.y));
                                                painter.circle_stroke(bone_screen, 4.0, egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(255, 190, 90)));
                                                painter.line_segment([bone_screen, rect.center()], egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 190, 90)));
                                            }
                                        }
                                    }

                                    painter.rect_stroke(stage_rect, 0.0, egui::Stroke::new(1.5_f32, egui::Color32::from_gray(15)));

                                    painter.rect_filled(egui::Rect::from_min_max(avail.min, egui::pos2(avail.right(), avail.top() + ruler)), 0.0, egui::Color32::from_gray(50));
                                    painter.rect_filled(egui::Rect::from_min_max(avail.min, egui::pos2(avail.left() + ruler, avail.bottom())), 0.0, egui::Color32::from_gray(50));
                                    let mut rx = stage_rect.left();
                                    while rx < canvas_rect.right() {
                                        if rx >= canvas_rect.left() {
                                            painter.line_segment([egui::pos2(rx, avail.top()), egui::pos2(rx, avail.top() + ruler)], egui::Stroke::new(1.0_f32, egui::Color32::from_gray(110)));
                                        }
                                        rx += 50.0 * stage_zoom;
                                    }
                                    let mut ry = stage_rect.top();
                                    while ry < canvas_rect.bottom() {
                                        if ry >= canvas_rect.top() {
                                            painter.line_segment([egui::pos2(avail.left(), ry), egui::pos2(avail.left() + ruler, ry)], egui::Stroke::new(1.0_f32, egui::Color32::from_gray(110)));
                                        }
                                        ry += 50.0 * stage_zoom;
                                    }

                                    mouse_stage_pos = response.hover_pos().map(|p| egui::pos2((p.x - stage_rect.left()) / stage_zoom, (p.y - stage_rect.top()) / stage_zoom));
                                });
                            });

                            // --- загрузка документа Adobe Animate (окно импорта) ---
                            if std::mem::take(&mut xfl_load_request) {
                                let path = xfl_path.trim().to_string();
                                let t = Instant::now();
                                match pony_core::xfl::XflDocument::open(&path) {
                                    Ok(doc) => {
                                        xfl_info = format!(
                                            "{}: {} символов, сцена {}×{}, {} fps — загружено за {:.1} с{}",
                                            doc.name,
                                            doc.symbols.len(),
                                            doc.width,
                                            doc.height,
                                            doc.frame_rate,
                                            t.elapsed().as_secs_f32(),
                                            if doc.warnings.is_empty() { String::new() } else { format!(" (пропущено битых символов: {})", doc.warnings.len()) }
                                        );
                                        xfl_symbol = default_xfl_symbol(&doc);
                                        xfl_frame = 0;
                                        xfl_preview_key = None;
                                        status_message = format!("Adobe Animate: открыт {path}");
                                        xfl_doc = Some(doc);
                                    }
                                    Err(err) => {
                                        xfl_info = format!("Не удалось открыть {path}: {err}");
                                        status_message = xfl_info.clone();
                                    }
                                }
                                xfl_window_open = true;
                                needs_redraw = true;
                            }

                            // --- результат проводника ---
                            if let Some(result) = dialog_result.take() {
                                use file_dialog::Purpose;
                                let path_str = result.path.to_string_lossy().into_owned();
                                let ext = result.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                                match result.purpose {
                                    Purpose::OpenDocument if ext == "fla" || ext == "xfl" || result.path.is_dir() => {
                                        xfl_path = path_str;
                                        xfl_load_request = true;
                                    }
                                    Purpose::ImportAnimate => {
                                        xfl_path = path_str;
                                        xfl_load_request = true;
                                    }
                                    Purpose::OpenDocument => {
                                        pending_open = path_str;
                                        actions.insert(0, Action::OpenPath);
                                    }
                                    Purpose::SaveDocument => {
                                        asset_path = path_str;
                                        actions.insert(0, Action::Save);
                                    }
                                    Purpose::ImportLayer => {
                                        let source = match ext.as_str() {
                                            "png" => Some(PartSource::Png { path: path_str.clone() }),
                                            "svg" => Some(PartSource::Vector { path: path_str.clone() }),
                                            "psd" => Some(PartSource::Psd { path: path_str.clone(), layer: None }),
                                            "kra" => Some(PartSource::Kra { path: path_str.clone(), layer_file: None }),
                                            _ => None,
                                        };
                                        match source {
                                            Some(source) => {
                                                import_count += 1;
                                                let stem = result.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| format!("imported_{import_count}"));
                                                let part_id = unique_name(&stem, |c| character.parts.contains_key(c));
                                                let max_layer = character.parts.values().map(|p| p.layer).max().unwrap_or(0);
                                                character.add_part(Part::new(&part_id, PartKind::Custom, source).with_bone(default_bone(&character)).with_layer(max_layer + 1));
                                                selected_layer = Some(part_id.clone());
                                                fit_request = Some(FitView::Character);
                                                status_message = format!("Импортировано '{path_str}' как слой '{part_id}'");
                                            }
                                            None => status_message = format!("Неподдерживаемый формат: {path_str} (нужны .png, .svg, .psd, .kra)"),
                                        }
                                    }
                                    Purpose::ExportGif | Purpose::ExportSpritesheet => {
                                        // Экспортируется то, что видно: скрытые в редакторе слои
                                        // и группы раньше всё равно попадали в GIF/спрайт-лист.
                                        let mut visible = character.clone();
                                        visible.parts.retain(|id, _| !hidden_layers.contains(id) && !character.is_part_hidden_by_group(id));
                                        status_message = if result.purpose == Purpose::ExportGif {
                                            export_animation_gif(&mut pony_renderer, &gpu_ctx, &visible, player.current_name(), &camera, &lighting, &path_str)
                                        } else {
                                            export_animation_spritesheet(&mut pony_renderer, &gpu_ctx, &visible, player.current_name(), &camera, &lighting, &path_str)
                                        };
                                    }
                                }
                                needs_redraw = true;
                            }

                            // --- действия из меню/тулбара/горячих клавиш ---
                            for action in actions.drain(..) {
                                match action {
                                    Action::Open => {
                                        let start = (!asset_path.is_empty()).then(|| std::path::PathBuf::from(&asset_path));
                                        file_dialog.show_for(file_dialog::Purpose::OpenDocument, start.as_deref(), "");
                                    }
                                    Action::SaveAs => {
                                        let start = (!asset_path.is_empty()).then(|| std::path::PathBuf::from(&asset_path));
                                        file_dialog.show_for(file_dialog::Purpose::SaveDocument, start.as_deref(), &format!("{}.asset", file_stem_for(&active_character_name)));
                                    }
                                    Action::Save if asset_path.is_empty() => {
                                        file_dialog.show_for(file_dialog::Purpose::SaveDocument, None, &format!("{}.asset", file_stem_for(&active_character_name)));
                                    }
                                    Action::ExportGif | Action::ExportSpritesheet => {
                                        let (purpose, ext) = if action == Action::ExportGif { (file_dialog::Purpose::ExportGif, "gif") } else { (file_dialog::Purpose::ExportSpritesheet, "png") };
                                        let start = (!asset_path.is_empty()).then(|| std::path::PathBuf::from(&asset_path));
                                        let anim = player.current_name().unwrap_or("animation").to_string();
                                        file_dialog.show_for(purpose, start.as_deref(), &format!("{}_{}.{ext}", file_stem_for(&active_character_name), file_stem_for(&anim)));
                                    }
                                    Action::ImportXfl => {
                                        let Some(doc) = &xfl_doc else { continue };
                                        let out_dir = xfl_import_dir(&doc.name, xfl_symbol.as_deref(), xfl_frame);
                                        match pony_core::xfl::import_to_character(doc, xfl_symbol.as_deref(), xfl_frame, &xfl_opts, &out_dir) {
                                            Ok((imported, report)) => {
                                                // Новый персонаж сцены — текущий уходит в список
                                                // остальных (как "+ Новый персонаж" на таймлайне).
                                                new_character_count += 1;
                                                let name = format!("{} {new_character_count}", imported.name);
                                                other_characters.push(CharacterSlot {
                                                    character: std::mem::replace(&mut character, imported),
                                                    player: std::mem::replace(&mut player, AnimationPlayer::new()),
                                                    name: std::mem::replace(&mut active_character_name, name),
                                                });
                                                // Перезаписанные SVG (повторный импорт того же кадра)
                                                // не должны браться из кэша текстур.
                                                for part in character.parts.values() {
                                                    if let PartSource::Vector { path } = &part.source {
                                                        pony_renderer.invalidate_texture(path);
                                                    }
                                                }
                                                playing = false;
                                                selected_layer = None;
                                                selected_bone = None;
                                                selected_keyframe = None;
                                                multi_selected.clear();
                                                hidden_layers.clear();
                                                locked_layers.clear();
                                                show_skeleton = false;
                                                history.reset(&character, &vector_doc);
                                                fit_request = Some(FitView::Character);
                                                xfl_window_open = false;
                                                status_message = format!(
                                                    "Импортировано: {} частей -> {}{}",
                                                    report.parts,
                                                    report.out_dir.display(),
                                                    if report.unsupported > 0 { format!(" (пропущено текста/картинок: {})", report.unsupported) } else { String::new() }
                                                );
                                            }
                                            Err(err) => status_message = format!("Импорт не удался: {err}"),
                                        }
                                    }
                                    Action::New | Action::DemoPony => {
                                        character = if action == Action::DemoPony { build_walking_pony() } else { empty_character() };
                                        asset_path.clear();
                                        pick_default_animation(&character, &mut player);
                                        player.apply(&mut character);
                                        playing = false;
                                        vector_doc.clear();
                                        editing_part = None;
                                        selected_layer = None;
                                        selected_keyframe = None;
                                        selected_shape_index = None;
                                        multi_selected.clear();
                                        hidden_layers.clear();
                                        locked_layers.clear();
                                        fit_request = Some(FitView::Character);
                                        status_message = if action == Action::DemoPony { "Демо-пони (Ctrl+Z — вернуть предыдущего)".into() } else { "Новый пустой персонаж (Ctrl+Z — вернуть предыдущего)".into() };
                                    }
                                    Action::OpenPath => match Character::load_from_file(&pending_open) {
                                        Ok(loaded) => {
                                            // Путь документа меняется только после успешной
                                            // загрузки: иначе Ctrl+S после ошибки перезаписал бы
                                            // нечитаемый файл текущим персонажем.
                                            asset_path = std::mem::take(&mut pending_open);
                                            character = loaded;
                                            // Раньше после Open плеер оставался пустым: таймлайн
                                            // показывал пустую сетку, и ключи было некуда ставить.
                                            pick_default_animation(&character, &mut player);
                                            player.apply(&mut character);
                                            playing = false;
                                            selected_layer = None;
                                            selected_keyframe = None;
                                            multi_selected.clear();
                                            hidden_layers.clear();
                                            locked_layers.clear();
                                            history.reset(&character, &vector_doc);
                                            saved_revision = history.revision;
                                            fit_request = Some(FitView::Character);
                                            status_message = format!("Загружено из {asset_path}");
                                        }
                                        Err(err) => status_message = format!("Ошибка загрузки {pending_open}: {err}"),
                                    },
                                    Action::Save => {
                                        status_message = match character.save_to_file(&asset_path) {
                                            Ok(()) => {
                                                // Сохраняем то, что видно сейчас: незафиксированная
                                                // правка (если есть) тоже должна считаться сохранённой.
                                                history.commit_if_changed(&character, &vector_doc, &player);
                                                saved_revision = history.revision;
                                                format!("Сохранено в {asset_path}")
                                            }
                                            Err(err) => format!("Ошибка сохранения {asset_path}: {err}"),
                                        };
                                    }
                                    Action::Undo | Action::Redo => {
                                        let restored = if action == Action::Undo { history.undo(&character, &vector_doc) } else { history.redo(&character, &vector_doc) };
                                        match restored {
                                            Some(state) => {
                                                character = state.character;
                                                vector_doc = state.vector_doc;
                                                // Ссылки на то, чего после отмены может уже не быть.
                                                if selected_layer.as_ref().is_some_and(|id| !character.parts.contains_key(id)) {
                                                    selected_layer = None;
                                                }
                                                if selected_bone.as_ref().is_some_and(|id| character.skeleton.find(id).is_none()) {
                                                    selected_bone = None;
                                                }
                                                if selected_shape_index.is_some_and(|i| i >= vector_doc.shapes.len()) {
                                                    selected_shape_index = None;
                                                }
                                                if !player.is_valid(&character) {
                                                    pick_default_animation(&character, &mut player);
                                                }
                                                selected_keyframe = None;
                                                status_message = if action == Action::Undo { "Отменено".into() } else { "Возвращено".into() };
                                            }
                                            None => status_message = if action == Action::Undo { "Нечего отменять".into() } else { "Нечего возвращать".into() },
                                        }
                                    }
                                    Action::TogglePlay => {
                                        if !playing && player.is_finished(&character) && player.is_valid(&character) {
                                            // Доигравшая незацикленная анимация — Play начинает сначала.
                                            player.seek(&character, 0.0);
                                        }
                                        playing = !playing && player.is_valid(&character);
                                        if !player.is_valid(&character) {
                                            status_message = "Нет анимации — создай её кнопкой «+» на таймлайне".into();
                                        }
                                    }
                                    Action::Rewind | Action::GoToEnd | Action::PrevFrame | Action::NextFrame => {
                                        playing = false;
                                        let duration = player.current_name().and_then(|n| character.animations.get(n)).map(|a| a.duration).unwrap_or(0.0);
                                        let frame = (player.time() * FPS).round();
                                        let t = match action {
                                            Action::Rewind => 0.0,
                                            Action::GoToEnd => duration,
                                            Action::PrevFrame => (frame - 1.0) / FPS,
                                            _ => (frame + 1.0) / FPS,
                                        };
                                        player.seek(&character, t);
                                        player.apply(&mut character);
                                    }
                                    Action::InsertKeyframe => {
                                        // В Xform ключ ставится той кости, которую двигаешь
                                        // (например, кости-контейнеру головы), иначе — кости слоя.
                                        let bone = if active_tool == Tool::FreeTransform && selected_bone.as_ref().is_some_and(|b| character.skeleton.find(b).is_some()) {
                                            selected_bone.clone()
                                        } else {
                                            selected_layer.as_ref().and_then(|id| character.parts.get(id)).and_then(|p| p.bone.clone())
                                        };
                                        match (selected_layer.clone(), bone) {
                                            (Some(layer_id), Some(bone_id)) => {
                                                let value = character.skeleton.find(&bone_id).map(|b| bone_channel_value(b, timeline_channel)).unwrap_or(0.0);
                                                let anim_name = ensure_animation(&mut character, &mut player);
                                                // Время — по сетке кадров: ключ "между кадрами"
                                                // потом нельзя было ни выделить, ни удалить.
                                                let frame = (player.time() * FPS).round();
                                                if let Some(anim) = character.animations.get_mut(&anim_name) {
                                                    insert_keyframe(anim, &bone_id, timeline_channel, frame / FPS, value);
                                                    selected_keyframe = Some((layer_id, frame as i64));
                                                    status_message = format!("Ключ на кадре {frame} для кости '{bone_id}' ({})", channel_label(timeline_channel));
                                                }
                                            }
                                            (Some(_), None) => status_message = "У выбранного слоя нет кости — ключ ставить не к чему".into(),
                                            (None, _) => status_message = "Сначала выбери слой (на Stage или в Timeline)".into(),
                                        }
                                    }
                                }
                                needs_redraw = true;
                            }

                            // --- история правок: фиксируем, когда жест мыши закончен ---
                            if !egui_ctx.input(|i| i.pointer.any_down()) && history.commit_if_changed(&character, &vector_doc, &player) {
                                needs_redraw = true;
                            }
                            let dirty = history.revision != saved_revision;
                            let doc_name = if asset_path.is_empty() { "Без имени".to_string() } else { asset_path.clone() };
                            let title = format!("pony-engine — {active_character_name} — {doc_name}{}", if dirty { " *" } else { "" });
                            if title != window_title {
                                window.set_title(&title);
                                window_title = title;
                            }

                            egui_state.handle_platform_output(&window, full_output.platform_output);
                            let clipped_primitives = egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);

                            for (id, image_delta) in &full_output.textures_delta.set {
                                egui_renderer.update_texture(&gpu_ctx.device, &gpu_ctx.queue, *id, image_delta);
                            }

                            let surface_texture = match surface.get_current_texture() {
                                Ok(t) => t,
                                Err(_) => {
                                    surface.configure(&gpu_ctx.device, &surface_config);
                                    return;
                                }
                            };
                            let view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
                            let mut encoder = gpu_ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("gui-frame") });

                            let screen_descriptor = egui_wgpu::ScreenDescriptor {
                                size_in_pixels: [surface_config.width, surface_config.height],
                                pixels_per_point: full_output.pixels_per_point,
                            };
                            egui_renderer.update_buffers(&gpu_ctx.device, &gpu_ctx.queue, &mut encoder, &clipped_primitives, &screen_descriptor);

                            {
                                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                    label: Some("gui-pass"),
                                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                        view: &view,
                                        resolve_target: None,
                                        ops: wgpu::Operations {
                                            load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.1, g: 0.1, b: 0.12, a: 1.0 }),
                                            store: wgpu::StoreOp::Store,
                                        },
                                    })],
                                    depth_stencil_attachment: None,
                                    timestamp_writes: None,
                                    occlusion_query_set: None,
                                });
                                egui_renderer.render(&mut rpass, &clipped_primitives, &screen_descriptor);
                            }

                            for id in &full_output.textures_delta.free {
                                egui_renderer.free_texture(id);
                            }

                            gpu_ctx.queue.submit(Some(encoder.finish()));
                            surface_texture.present();

                            // Перерисовываемся снова, только если есть зачем: идёт
                            // анимация/частицы/тряска, egui просит (ховер, анимация
                            // виджетов), документ ещё не догнал последний рендер или
                            // только что было действие. Иначе цикл спит до следующего
                            // события — раньше окно молотило CPU/GPU на 100% всегда.
                            let egui_repaint = full_output.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay);
                            let render_stale = !last_render_key.as_ref().is_some_and(|k| k.character == character && k.hidden == hidden_layers && k.lighting == lighting);
                            let busy = playing || particles_enabled || camera.shake_intensity > 0.0 || blink_timer > 0.0 || render_stale || needs_redraw || autoexit_at.is_some();
                            needs_redraw = false;
                            match egui_repaint {
                                _ if busy => window.request_redraw(),
                                Some(delay) if delay.is_zero() => window.request_redraw(),
                                Some(delay) if delay < Duration::from_secs(10) => elwt.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(Instant::now() + delay)),
                                _ => elwt.set_control_flow(winit::event_loop::ControlFlow::Wait),
                            }
                        }
                        _ => {}
                    }
                }
                // WaitUntil истёк (egui попросил перерисоваться с задержкой —
                // например, мигание курсора в текстовом поле).
                Event::NewEvents(winit::event::StartCause::ResumeTimeReached { .. }) => window.request_redraw(),
                _ => {}
            }
        })
        .expect("event loop error");
}

#[cfg(test)]
mod timeline_tests {
    use super::*;
    use pony_core::animation::Animation;

    fn test_anim() -> Animation {
        Animation { name: "Test".into(), duration: 1.0, looping: false, tracks: Vec::new() }
    }

    #[test]
    fn insert_then_remove_keyframe_round_trips_the_value() {
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.5, 12.5);
        assert_eq!(anim.tracks.len(), 1, "должна создаться дорожка");
        let removed = remove_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.5);
        assert_eq!(removed, Some(12.5));
        assert!(anim.tracks[0].keyframes.is_empty(), "ключ должен реально исчезнуть из дорожки");
    }

    #[test]
    fn remove_keyframe_at_wrong_time_returns_none_and_does_not_touch_others() {
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.5, 12.5);
        let removed = remove_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.9); // другое время
        assert_eq!(removed, None);
        assert_eq!(anim.tracks[0].keyframes.len(), 1, "существующий ключ не должен был пострадать");
    }

    #[test]
    fn dragging_a_keyframe_to_a_new_time_is_remove_then_insert() {
        // Симулируем то, что делает drag-обработчик в timeline_widget: убрать
        // старый ключ, поставить новый с тем же значением на новом времени.
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.2, 7.0);
        let value = remove_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.2).expect("ключ должен был найтись");
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.8, value);

        assert_eq!(anim.tracks[0].keyframes.len(), 1, "должен остаться ровно один ключ, не два");
        let kf = &anim.tracks[0].keyframes[0];
        assert!((kf.time - 0.8).abs() < 1e-4, "ключ должен переехать на новое время");
        match kf.value {
            pony_core::animation::AnimValue::Float(v) => assert!((v - 7.0).abs() < 1e-4, "значение должно сохраниться при перетаскивании"),
            _ => panic!("ожидали AnimValue::Float"),
        }
    }

    #[test]
    fn insert_keyframe_extends_duration_when_keyframe_is_past_the_end() {
        let mut anim = test_anim();
        assert_eq!(anim.duration, 1.0);
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 2.5, 0.0);
        assert!((anim.duration - 2.5).abs() < 1e-4, "длительность анимации должна вырасти под новый ключ");
    }

    #[test]
    fn remove_keyframe_finds_key_placed_between_frames() {
        // Ключ не ровно на границе кадра (0.43с ~ кадр 10.3) рисуется в
        // ячейке кадра 10 — и удаляться должен по клику на эту ячейку.
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.43, 3.0);
        assert_eq!(remove_keyframe(&mut anim, "Head", BoneChannel::PositionY, 10.0 / FPS), Some(3.0));
    }

    #[test]
    fn has_keyframe_at_only_sees_the_same_bone_and_channel() {
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.5, 1.0);
        assert!(has_keyframe_at(&anim, "Head", BoneChannel::PositionY, 0.5));
        assert!(!has_keyframe_at(&anim, "Head", BoneChannel::Rotation, 0.5));
        assert!(!has_keyframe_at(&anim, "Body", BoneChannel::PositionY, 0.5));
        assert!(!has_keyframe_at(&anim, "Head", BoneChannel::PositionY, 0.75));
    }

    #[test]
    fn ensure_animation_creates_one_when_player_has_nothing() {
        let mut character = Character::new("Empty");
        let mut player = AnimationPlayer::new();
        let name = ensure_animation(&mut character, &mut player);
        assert!(character.animations.contains_key(&name));
        assert_eq!(player.current_name(), Some(name.as_str()));
        // Повторный вызов не плодит новые анимации.
        assert_eq!(ensure_animation(&mut character, &mut player), name);
        assert_eq!(character.animations.len(), 1);
    }

    #[test]
    fn history_records_edits_but_not_animation_pose() {
        let mut character = build_walking_pony();
        let doc = pony_core::VectorDoc::new();
        let mut player = AnimationPlayer::new();
        player.play("Walk");
        let mut history = History::new(&character, &doc);

        // Воспроизведение двигает голову — это не правка.
        player.advance(&character, 0.3);
        player.apply(&mut character);
        assert!(!history.commit_if_changed(&character, &doc, &player));
        assert!(history.undo.is_empty());

        // Правка пользователя — шаг истории.
        character.parts.get_mut("horn").unwrap().pivot.x += 5.0;
        assert!(history.commit_if_changed(&character, &doc, &player));
        assert_eq!(history.undo.len(), 1);

        // Undo возвращает старое значение, Redo — новое.
        let before = history.undo(&character, &doc).unwrap();
        let moved_x = character.parts["horn"].pivot.x;
        assert!((before.character.parts["horn"].pivot.x - (moved_x - 5.0)).abs() < 1e-4);
        let after = history.redo(&before.character, &before.vector_doc).unwrap();
        assert!((after.character.parts["horn"].pivot.x - moved_x).abs() < 1e-4);
    }

    #[test]
    fn history_records_vector_doc_drawing() {
        let character = build_walking_pony();
        let mut doc = pony_core::VectorDoc::new();
        let player = AnimationPlayer::new();
        let mut history = History::new(&character, &doc);
        doc.add(pony_core::VectorShape::Line { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0, stroke: pony_core::RgbaColor::new(0, 0, 0, 255), stroke_width: 1.0 });
        assert!(history.commit_if_changed(&character, &doc, &player));
        let prev = history.undo(&character, &doc).unwrap();
        assert!(prev.vector_doc.shapes.is_empty(), "Undo должен убрать нарисованную фигуру");
    }

    #[test]
    fn part_at_picks_topmost_visible_part() {
        let character = build_walking_pony();
        let none = HashSet::new();
        let world = character.skeleton.world_transform_with_ik("Head").unwrap();
        let head_pos = pony_render::part_world_position(&character.parts["head"], &world);
        let hit = part_at(&character, &none, &none, head_pos).expect("в центре головы должна найтись часть");
        assert!(character.parts[&hit].layer >= character.parts["head"].layer);
        // Скрытая часть не выбирается.
        let mut hidden = HashSet::new();
        hidden.insert(hit.clone());
        assert_ne!(part_at(&character, &hidden, &none, head_pos).as_deref(), Some(hit.as_str()));
    }

    #[test]
    fn unique_name_skips_taken_names() {
        let taken = ["drawn_1", "drawn_1_2"];
        assert_eq!(unique_name("drawn_1", |c| taken.contains(&c)), "drawn_1_3");
        assert_eq!(unique_name("drawn_5", |c| taken.contains(&c)), "drawn_5");
    }

    #[test]
    fn insert_keyframe_at_existing_time_replaces_not_duplicates() {
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.3, 1.0);
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.3, 99.0);
        assert_eq!(anim.tracks[0].keyframes.len(), 1, "не должно быть двух ключей на одном времени");
        match anim.tracks[0].keyframes[0].value {
            pony_core::animation::AnimValue::Float(v) => assert!((v - 99.0).abs() < 1e-4),
            _ => panic!(),
        }
    }
}
