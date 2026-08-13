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

const SCENE_WIDTH: u32 = 480;
const SCENE_HEIGHT: u32 = 360;
const FPS: f32 = 24.0;
const ASSET_PATH: &str = "gui_character.asset";

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
        .add_part(Part::new("leg_fl", PartKind::LegFL, PartSource::Png { path: "assets/pony/leg.png".into() }).with_bone("LowerLegFL").with_layer(0))
        .add_part(Part::new("leg_fr", PartKind::LegFR, PartSource::Png { path: "assets/pony/leg.png".into() }).with_bone("LowerLegFR").with_layer(0))
        .add_part(Part::new("wing_l", PartKind::Wing, PartSource::Vector { path: "assets/pony_svg/wing.svg".into() }).with_bone("Body").with_layer(0))
        .add_part(Part::new("tail", PartKind::Tail, PartSource::Vector { path: "assets/pony_svg/tail.svg".into() }).with_bone("Body").with_layer(0))
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
        }
    }
}

/// Timeline как в Animate: слои встроены прямо в панель (левая колонка —
/// имя + видимость + блокировка), справа — сетка КАДРОВ (не времени),
/// ключевые кадры — кружки в ячейках, плейхед. Один слой может не иметь
/// собственной дорожки анимации (нормально, как в Animate — статичные слои).
#[allow(clippy::too_many_arguments)]
fn timeline_widget(
    ui: &mut egui::Ui,
    character: &mut Character,
    player: &mut AnimationPlayer,
    playing: &mut bool,
    hidden: &mut HashSet<String>,
    locked: &mut HashSet<String>,
    selected: &mut Option<String>,
) {
    let anim_data: Option<(f32, Vec<Track>)> =
        player.current_name().and_then(|n| character.animations.get(n)).map(|a| (a.duration, a.tracks.clone()));
    let (duration, tracks) = anim_data.unwrap_or((1.0, Vec::new()));
    let duration = duration.max(0.01);
    let total_frames = (duration * FPS).round().max(1.0) as i64;

    let mut layer_ids: Vec<String> = character.parts.keys().cloned().collect();
    layer_ids.sort_by_key(|id| (character.parts[id].layer, id.clone()));

    // Дорожка привязывается к слою, если её AnimTarget::Bone указывает на
    // кость этого слоя. Дорожки без такой привязки (Morph/EyeParam/Camera)
    // собираются в один дополнительный ряд "Прочее" внизу — в реальном
    // Animate у морфинга/камеры тоже нет отдельного "слоя" на канвасе.
    let mut layer_frames: Vec<(String, Vec<i64>)> = layer_ids.iter().map(|id| (id.clone(), Vec::new())).collect();
    let mut misc_frames: Vec<i64> = Vec::new();
    for track in &tracks {
        let bone_id = match &track.target {
            AnimTarget::Bone { id, .. } => Some(id.as_str()),
            _ => None,
        };
        let matched_layer = bone_id.and_then(|bid| layer_ids.iter().find(|lid| character.parts[*lid].bone.as_deref() == Some(bid)));
        let frames: Vec<i64> = track.keyframes.iter().map(|kf| (kf.time * FPS).round() as i64).collect();
        match matched_layer {
            Some(lid) => {
                if let Some(entry) = layer_frames.iter_mut().find(|(id, _)| id == lid) {
                    entry.1.extend(frames);
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

    let desired_size = egui::vec2(ui.available_width(), total_height);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_gray(20));

    let grid_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + label_col_width, rect.top()), rect.max);
    let ruler_rect = egui::Rect::from_min_max(grid_rect.min, egui::pos2(grid_rect.right(), grid_rect.top() + ruler_height));
    painter.rect_filled(ruler_rect, 0.0, egui::Color32::from_gray(32));

    let frame_w = (grid_rect.width() / total_frames as f32).max(2.0);
    let tick_every = if total_frames <= 30 { 1 } else { (total_frames / 20).max(1) };
    for f in (0..=total_frames).step_by(tick_every as usize) {
        let x = grid_rect.left() + f as f32 * frame_w;
        painter.line_segment([egui::pos2(x, ruler_rect.top()), egui::pos2(x, ruler_rect.bottom())], egui::Stroke::new(1.0, egui::Color32::from_gray(70)));
        painter.text(egui::pos2(x + 2.0, ruler_rect.top() + 1.0), egui::Align2::LEFT_TOP, format!("{f}"), egui::FontId::proportional(9.0), egui::Color32::from_gray(160));
    }

    let mut row_labels: Vec<(String, Option<Vec<i64>>)> = layer_ids
        .iter()
        .zip(layer_frames.into_iter().map(|(_, frames)| frames))
        .map(|(id, frames)| (id.clone(), Some(frames)))
        .collect();
    if has_misc {
        row_labels.push(("(Прочее: камера/морфы)".to_string(), None));
    }

    for (i, (label, frames_opt)) in row_labels.iter().enumerate() {
        let row_top = grid_rect.top() + ruler_height + row_height * i as f32;
        let label_row_rect = egui::Rect::from_min_max(egui::pos2(rect.left(), row_top), egui::pos2(grid_rect.left(), row_top + row_height));
        let grid_row_rect = egui::Rect::from_min_max(egui::pos2(grid_rect.left(), row_top), egui::pos2(grid_rect.right(), row_top + row_height));
        let bg = if i % 2 == 0 { egui::Color32::from_gray(26) } else { egui::Color32::from_gray(22) };
        painter.rect_filled(label_row_rect, 0.0, bg);
        painter.rect_filled(grid_row_rect, 0.0, bg);

        let is_layer_row = frames_opt.is_some();
        if is_layer_row {
            let is_selected = selected.as_deref() == Some(label.as_str());
            if is_selected {
                painter.rect_filled(label_row_rect, 0.0, egui::Color32::from_rgb(45, 65, 90));
            }
            let cb_rect = egui::Rect::from_min_size(egui::pos2(rect.left() + 4.0, row_top + row_height / 2.0 - 6.0), egui::vec2(12.0, 12.0));
            let visible = !hidden.contains(label);
            painter.rect_stroke(cb_rect, 2.0, egui::Stroke::new(1.0, egui::Color32::from_gray(150)));
            if visible {
                painter.rect_filled(cb_rect.shrink(2.5), 1.0, egui::Color32::from_rgb(120, 170, 230));
            }
            let lock_rect = egui::Rect::from_min_size(egui::pos2(rect.left() + 20.0, row_top + row_height / 2.0 - 6.0), egui::vec2(12.0, 12.0));
            let is_locked = locked.contains(label);
            painter.rect_stroke(lock_rect, 2.0, egui::Stroke::new(1.0, egui::Color32::from_gray(150)));
            if is_locked {
                painter.rect_filled(lock_rect.shrink(2.5), 1.0, egui::Color32::from_rgb(230, 170, 90));
            }
            painter.text(
                egui::pos2(rect.left() + 38.0, row_top + row_height / 2.0),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(210),
            );
        } else {
            painter.text(
                egui::pos2(rect.left() + 6.0, row_top + row_height / 2.0),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(150),
            );
        }

        let frames: &[i64] = frames_opt.as_deref().unwrap_or(&misc_frames);
        for &f in frames {
            let x = grid_rect.left() + f as f32 * frame_w + frame_w / 2.0;
            let cy = row_top + row_height / 2.0;
            painter.circle_filled(egui::pos2(x, cy), 4.0, egui::Color32::from_rgb(230, 190, 90));
            painter.circle_stroke(egui::pos2(x, cy), 4.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 90, 30)));
        }
    }

    let current_frame = (player.time() * FPS).round().clamp(0.0, total_frames as f32);
    let playhead_x = grid_rect.left() + current_frame * frame_w;
    painter.line_segment([egui::pos2(playhead_x, rect.top()), egui::pos2(playhead_x, rect.bottom())], egui::Stroke::new(2.0, egui::Color32::from_rgb(230, 80, 80)));

    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            if pos.x < grid_rect.left() {
                let row_i = ((pos.y - (grid_rect.top() + ruler_height)) / row_height).floor() as i64;
                if row_i >= 0 && (row_i as usize) < row_labels.len() {
                    let (label, frames_opt) = &row_labels[row_i as usize];
                    if frames_opt.is_some() {
                        let local_x = pos.x - rect.left();
                        if (4.0..16.0).contains(&local_x) {
                            if hidden.contains(label) {
                                hidden.remove(label);
                            } else {
                                hidden.insert(label.clone());
                            }
                        } else if (20.0..32.0).contains(&local_x) {
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
    }
    if response.dragged() || (response.clicked() && response.interact_pointer_pos().map(|p| p.x >= grid_rect.left()).unwrap_or(false)) {
        if let Some(pos) = response.interact_pointer_pos() {
            if pos.x >= grid_rect.left() {
                let frac = ((pos.x - grid_rect.left()) / grid_rect.width()).clamp(0.0, 1.0);
                let target_t = frac * duration;
                *playing = false;
                let current = player.time();
                player.advance(character, target_t - current);
                player.apply(character);
            }
        }
    }
}

fn color32_to_rgb(c: egui::Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

fn str_to_owned(s: &str) -> String {
    s.to_string()
}

/// Глубина истории отмены. Снимок — это полный клон `Character`, поэтому
/// стек ограничен: 50 шагов на персонажа из десятка частей — единицы
/// мегабайт, а не «пока не кончится память».
const MAX_UNDO_DEPTH: usize = 50;

/// Положить текущее состояние персонажа в стек отмены. Вызывается ПЕРЕД
/// мутацией. Redo при этом очищается — это стандартное поведение: после
/// нового действия «вперёд» идти уже некуда.
fn push_undo(undo: &mut Vec<pony_core::Character>, redo: &mut Vec<pony_core::Character>, character: &pony_core::Character) {
    undo.push(character.clone());
    if undo.len() > MAX_UNDO_DEPTH {
        undo.remove(0);
    }
    redo.clear();
}

fn selected_bone_id(character: &pony_core::Character, selected: &Option<String>) -> Option<String> {
    selected.as_ref().and_then(|id| character.parts.get(id)).and_then(|p| p.bone.clone())
}

/// Вставить (или заменить) ключевой кадр позиции по Y для указанной кости
/// в момент `time`. Если дорожки на эту кость ещё нет — создаём её.
/// Ключ ровно на том же времени заменяется, а не дублируется — иначе
/// сэмплирование зависело бы от порядка вставки.
fn insert_keyframe(anim: &mut pony_core::animation::Animation, bone_id: &str, time: f32, value: f32) {
    let target_matches = |t: &AnimTarget| matches!(t, AnimTarget::Bone { id, channel } if id == bone_id && *channel == BoneChannel::PositionY);

    let track = match anim.tracks.iter_mut().find(|t| target_matches(&t.target)) {
        Some(t) => t,
        None => {
            anim.tracks.push(Track {
                target: AnimTarget::Bone { id: bone_id.to_string(), channel: BoneChannel::PositionY },
                keyframes: Vec::new(),
            });
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

fn color32_to_rgba(c: egui::Color32) -> pony_core::RgbaColor {
    pony_core::RgbaColor::new(c.r(), c.g(), c.b(), c.a())
}

fn rgba_to_color32(c: pony_core::RgbaColor) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// Отрисовать одну сохранённую в `VectorDoc` фигуру поверх Stage — экранные
/// координаты вычисляет `world_to_screen`. Хранение фигур — в SVG-системе
/// координат (Y вниз), а весь остальной Stage — в мировой (Y вверх), поэтому
/// здесь Y инвертируется обратно перед вызовом `world_to_screen`.
fn draw_vector_shape_preview(painter: &egui::Painter, shape: &pony_core::VectorShape, world_to_screen: &impl Fn(glam::Vec2) -> egui::Pos2) {
    use pony_core::VectorShape;
    match shape {
        VectorShape::Rect { x, y, w, h, fill, stroke, stroke_width } => {
            let p0 = world_to_screen(glam::Vec2::new(*x, -*y));
            let p1 = world_to_screen(glam::Vec2::new(x + w, -(y + h)));
            let rect = egui::Rect::from_two_pos(p0, p1);
            painter.rect(rect, 0.0, rgba_to_color32(*fill), egui::Stroke::new(*stroke_width, rgba_to_color32(*stroke)));
        }
        VectorShape::Ellipse { cx, cy, rx, ry: _ry, fill, stroke, stroke_width } => {
            // Превью рисует круг (egui::Painter не даёт эллипс одной командой) —
            // упрощение только для оверлея на Stage; сохранённый .svg честно
            // хранит rx/ry раздельно и после сохранения через resvg
            // рендерится настоящим эллипсом, без искажения.
            let center = world_to_screen(glam::Vec2::new(*cx, -*cy));
            let edge = world_to_screen(glam::Vec2::new(cx + rx, -*cy));
            let r = (edge.x - center.x).abs();
            painter.circle(center, r, rgba_to_color32(*fill), egui::Stroke::new(*stroke_width, rgba_to_color32(*stroke)));
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
        VectorShape::Polygon { points, fill, stroke, stroke_width } => {
            // Честная оговорка: egui::Shape::convex_polygon корректно
            // рисует только ВЫПУКЛЫЕ многоугольники — для PolyStar (всегда
            // правильный n-угольник) это всегда так, но если Pen нарисует
            // невыпуклую фигуру, ПРЕВЬЮ на Stage может исказиться. Сам
            // сохранённый .svg при этом рисуется правильно в любом случае —
            // это ограничение только egui-оверлея, не движка.
            let screen_pts: Vec<egui::Pos2> = points.iter().map(|(x, y)| world_to_screen(glam::Vec2::new(*x, -*y))).collect();
            painter.add(egui::Shape::convex_polygon(screen_pts, rgba_to_color32(*fill), egui::Stroke::new(*stroke_width, rgba_to_color32(*stroke))));
        }
    }
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
) -> String {
    let frames = match render_animation_frames(renderer, ctx, character, anim_name, camera, lighting) {
        Ok(f) => f,
        Err(msg) => return format!("Экспорт GIF не выполнен: {msg}"),
    };
    let delay_cs = (100.0 / EXPORT_FPS).round() as u16;
    match export_gif("gui_export.gif", &frames, delay_cs) {
        Ok(()) => format!("GIF сохранён: gui_export.gif ({} кадров)", frames.len()),
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
) -> String {
    let frames = match render_animation_frames(renderer, ctx, character, anim_name, camera, lighting) {
        Ok(f) => f,
        Err(msg) => return format!("Экспорт спрайт-листа не выполнен: {msg}"),
    };
    match export_spritesheet("gui_export_sheet.png", &frames, 6) {
        Ok(layout) => format!("Спрайт-лист сохранён: gui_export_sheet.png ({}x{} кадров)", layout.columns, layout.rows),
        Err(err) => format!("Ошибка экспорта спрайт-листа: {err}"),
    }
}

fn main() {
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
    let texture_budget = pony_system::WorkloadPolicy::from_profile(&pony_system::SystemProfile::detect_without_gpus()).memory_budget_bytes;
    let mut pony_renderer = Renderer::new_with_budget(&gpu_ctx, texture_budget);

    let egui_ctx = egui::Context::default();
    let mut egui_state = egui_winit::State::new(egui_ctx.clone(), egui::ViewportId::ROOT, &window, None, None);
    let mut egui_renderer = egui_wgpu::Renderer::new(&gpu_ctx.device, surface_format, None, 1);

    let mut character = build_walking_pony();
    let mut player = AnimationPlayer::new();
    player.play("Walk");
    let mut camera = pony_core::Camera::default();
    let script_engine = pony_script::ScriptEngine::new();
    let mut script_text = String::from("// Пример — жми \"Выполнить\"\npony.Smile(0.8);\npony.Blink();\ncamera.Zoom(1.2);");
    let mut script_log = String::new();
    let mut playing = true;
    let mut scene_texture: Option<egui::TextureHandle> = None;
    let mut last_frame = Instant::now();
    let mut elapsed_time: f32 = 0.0;
    let mut hidden_layers: HashSet<String> = HashSet::new();
    let mut locked_layers: HashSet<String> = HashSet::new();
    let mut selected_layer: Option<String> = None;
    let mut active_tool = Tool::Selection;
    let mut right_tab = RightTab::Properties;
    let mut stage_zoom: f32 = 1.0;
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

    // --- 2.5D-поворот (раздел 8 ТЗ) в градусах для UI, character.facing_yaw в радианах ---
    let mut facing_yaw_deg: f32 = 0.0;

    // --- рисование/редактирование SVG прямо на Stage (раздел 16 ТЗ) ---
    let mut vector_doc = pony_core::VectorDoc::new();
    // Начало текущего перетаскивания (мировые координаты) для Rect/Oval/Line —
    // фигура коммитится в vector_doc только когда drag реально закончился.
    let mut draw_start: Option<glam::Vec2> = None;
    // Точки текущего свободного мазка (Pencil/Brush) — копятся все кадры
    // одного drag'а, коммитятся одной Polyline в конце.
    let mut pencil_points: Vec<(f32, f32)> = Vec::new();
    let mut svg_save_count: u32 = 0;
    // Импорт ассетов из GUI (см. меню File > Import asset as layer).
    let mut import_path = String::from("assets/pony/body.png");
    let mut import_count: u32 = 0;
    // История отмены (меню Edit). Снимки полного Character — см. push_undo.
    let mut undo_stack: Vec<pony_core::Character> = Vec::new();
    let mut redo_stack: Vec<pony_core::Character> = Vec::new();
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

    // --- SubSelection: редактирование точек уже нарисованной (ещё не
    // сохранённой как часть) фигуры на холсте ---
    let mut selected_shape_index: Option<usize> = None;
    let mut dragging_point_index: Option<usize> = None;

    // --- Lasso: рамкой выделить несколько частей персонажа сразу, для
    // групповых операций (скрыть/показать/удалить группой) ---
    let mut lasso_start: Option<glam::Vec2> = None;
    let mut multi_selected: std::collections::HashSet<String> = std::collections::HashSet::new();

    // --- производительность: не гонять дорогой GPU-рендер персонажа
    // (render+readback на CPU+перезалив текстуры) на каждый кадр во время
    // перетаскивания фигуры рисования (Rect/Oval/Line/Pencil/Brush) — в
    // этот момент персонаж вообще не меняется (см. код Stage ниже: эти
    // инструменты пишут только в vector_doc/pencil_points), а мышь на
    // Xvfb/софтверном рендере шлёт события на каждый пиксель движения,
    // и полный ре-рендер персонажа на каждое из них — вот источник лага
    // при рисовании. Осознанно НЕ делаем общий dirty-флаг на все места
    // мутации персонажа (Transform-слайдеры, drag кости, скрипт и т.д.) —
    // легко забыть один случай и получить куда худший баг, "картинка не
    // обновилась". Здесь риска нет: вне рисования рендерим, как и раньше,
    // каждый кадр, без исключений.
    // Значение — про ПРОШЛЫЙ кадр (обновляется в конце текущего, читается
    // в начале следующего): при click-то-draw без задержки на первый кадр
    // после начала drag'а мы ещё отрендерим персонажа (не страшно — редко),
    // а все последующие кадры одного drag'а — уже пропустят рендер.
    let mut was_dragging_shape_last_frame = false;
    let mut last_frame_output: Option<pony_render::FrameOutput> = None;

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
                    if response.consumed {
                        window.request_redraw();
                        return;
                    }
                    match event {
                        WindowEvent::CloseRequested => elwt.exit(),
                        WindowEvent::Resized(new_size) => {
                            surface_config.width = new_size.width.max(1);
                            surface_config.height = new_size.height.max(1);
                            surface.configure(&gpu_ctx.device, &surface_config);
                        }
                        WindowEvent::RedrawRequested => {
                            let dt = last_frame.elapsed().as_secs_f32();
                            last_frame = Instant::now();
                            elapsed_time += dt;
                            if playing {
                                player.advance(&character, dt);
                                player.apply(&mut character);
                            }
                            // Тряска затухает со временем — сама Camera не хранит
                            // часы и не решает это за нас (см. camera.rs), поэтому
                            // затухание — здесь, в игровом цикле GUI.
                            camera.shake_intensity = (camera.shake_intensity - dt * 0.6).max(0.0);

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

                            character.facing_yaw = facing_yaw_deg.to_radians();

                            // Пропускаем дорогой рендер персонажа именно во время
                            // перетаскивания фигуры рисования (см. пояснение у объявления
                            // `was_dragging_shape_last_frame` выше) — во всех остальных
                            // случаях рендерим каждый кадр, как и раньше, без исключений.
                            if !was_dragging_shape_last_frame {
                                let mut visible_character = character.clone();
                                visible_character.parts.retain(|id, _| !hidden_layers.contains(id));
                                let particles_arg = if particles_enabled { Some(&particle_emitter) } else { None };
                                let rendered = pony_renderer.render_character(
                                    &gpu_ctx,
                                    &visible_character,
                                    SCENE_WIDTH,
                                    SCENE_HEIGHT,
                                    &camera,
                                    elapsed_time,
                                    &lighting,
                                    particles_arg,
                                );
                                let color_image =
                                    egui::ColorImage::from_rgba_unmultiplied([rendered.width as usize, rendered.height as usize], &rendered.rgba);
                                match &mut scene_texture {
                                    Some(handle) => handle.set(color_image, egui::TextureOptions::NEAREST),
                                    None => scene_texture = Some(egui_ctx.load_texture("pony-scene", color_image, egui::TextureOptions::NEAREST)),
                                }
                                last_frame_output = Some(rendered);
                            }

                            let raw_input = egui_state.take_egui_input(&window);
                            let full_output = egui_ctx.run(raw_input, |ctx| {
                                egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
                                    egui::menu::bar(ui, |ui| {
                                        ui.menu_button("File", |ui| {
                                            if ui.button("New").clicked() {
                                                character = build_walking_pony();
                                                player = AnimationPlayer::new();
                                                player.play("Walk");
                                                selected_layer = None;
                                                status_message = "Новый персонаж".into();
                                                ui.close_menu();
                                            }
                                            if ui.button(format!("Save ({ASSET_PATH})")).clicked() {
                                                status_message = match character.save_to_file(ASSET_PATH) {
                                                    Ok(()) => format!("Сохранено в {ASSET_PATH}"),
                                                    Err(err) => format!("Ошибка сохранения: {err}"),
                                                };
                                                ui.close_menu();
                                            }
                                            if ui.button(format!("Open ({ASSET_PATH})")).clicked() {
                                                match Character::load_from_file(ASSET_PATH) {
                                                    Ok(loaded) => {
                                                        character = loaded;
                                                        player = AnimationPlayer::new();
                                                        selected_layer = None;
                                                        status_message = format!("Загружено из {ASSET_PATH}");
                                                    }
                                                    Err(err) => status_message = format!("Ошибка загрузки: {err}"),
                                                }
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            ui.separator();
                                            // Импорт ассетов: движок умеет PNG/SVG/PSD/KRA, но
                                            // до этого добраться до них из GUI было нельзя
                                            // вообще — форматы работали только если прописать
                                            // PartSource руками в коде. Диалога выбора файла
                                            // нет (`rfd` не собирается в этой песочнице, см.
                                            // README), поэтому импорт идёт по фиксированному
                                            // пути из поля ниже — не идеально, но реально
                                            // работает и честно об этом говорит.
                                            ui.menu_button("Import asset as layer", |ui| {
                                                ui.label("Путь к файлу (относительно рабочей папки):");
                                                ui.add(egui::TextEdit::singleline(&mut import_path).desired_width(260.0));
                                                let ext = std::path::Path::new(&import_path)
                                                    .extension()
                                                    .and_then(|e| e.to_str())
                                                    .unwrap_or("")
                                                    .to_lowercase();
                                                let source = match ext.as_str() {
                                                    "png" => Some(pony_core::part::PartSource::Png { path: import_path.clone() }),
                                                    "svg" => Some(pony_core::part::PartSource::Vector { path: import_path.clone() }),
                                                    "psd" => Some(pony_core::part::PartSource::Psd { path: import_path.clone(), layer: None }),
                                                    "kra" => Some(pony_core::part::PartSource::Kra { path: import_path.clone(), layer_file: None }),
                                                    _ => None,
                                                };
                                                match &source {
                                                    Some(_) => {
                                                        ui.label(format!("Формат: {} — поддерживается", ext.to_uppercase()));
                                                    }
                                                    None => {
                                                        ui.label("Поддерживаются: .png, .svg, .psd, .kra");
                                                    }
                                                }
                                                if ui.add_enabled(source.is_some(), egui::Button::new("Импортировать")).clicked() {
                                                    if let Some(source) = source {
                                                        if !std::path::Path::new(&import_path).exists() {
                                                            status_message = format!("Файл не найден: {import_path}");
                                                        } else {
                                                            import_count += 1;
                                                            let part_id = format!("imported_{import_count}");
                                                            character.add_part(
                                                                pony_core::part::Part::new(&part_id, pony_core::part::PartKind::Custom, source)
                                                                    .with_bone("Body")
                                                                    .with_layer(20 + import_count as i32),
                                                            );
                                                            status_message = format!("Импортировано '{import_path}' как слой '{part_id}'");
                                                        }
                                                    }
                                                    ui.close_menu();
                                                }
                                            });
                                            ui.separator();
                                            if ui.button("Export GIF (текущая анимация)").clicked() {
                                                status_message =
                                                    export_animation_gif(&mut pony_renderer, &gpu_ctx, &character, player.current_name(), &camera, &lighting);
                                                ui.close_menu();
                                            }
                                            if ui.button("Export Spritesheet (текущая анимация)").clicked() {
                                                status_message = export_animation_spritesheet(
                                                    &mut pony_renderer,
                                                    &gpu_ctx,
                                                    &character,
                                                    player.current_name(),
                                                    &camera,
                                                    &lighting,
                                                );
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("Exit").clicked() {
                                                elwt.exit();
                                            }
                                        });
                                        ui.menu_button("Edit", |ui| {
                                            let can_undo = !undo_stack.is_empty();
                                            let can_redo = !redo_stack.is_empty();
                                            if ui.add_enabled(can_undo, egui::Button::new(format!("Undo ({})", undo_stack.len()))).clicked() {
                                                if let Some(prev) = undo_stack.pop() {
                                                    redo_stack.push(character.clone());
                                                    character = prev;
                                                    selected_layer = None;
                                                    status_message = "Отменено".into();
                                                }
                                                ui.close_menu();
                                            }
                                            if ui.add_enabled(can_redo, egui::Button::new(format!("Redo ({})", redo_stack.len()))).clicked() {
                                                if let Some(next) = redo_stack.pop() {
                                                    undo_stack.push(character.clone());
                                                    character = next;
                                                    selected_layer = None;
                                                    status_message = "Возвращено".into();
                                                }
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            let has_sel = selected_layer.is_some();
                                            if ui.add_enabled(has_sel, egui::Button::new("Delete layer")).clicked() {
                                                if let Some(id) = selected_layer.clone() {
                                                    push_undo(&mut undo_stack, &mut redo_stack, &character);
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
                                                        push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                        let new_id = format!("{id}_copy");
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
                                                    push_undo(&mut undo_stack, &mut redo_stack, &character);
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
                                            }
                                        });

                                        ui.menu_button("View", |ui| {
                                            if ui.button("Zoom In").clicked() {
                                                stage_zoom = (stage_zoom * 1.25).min(8.0);
                                                ui.close_menu();
                                            }
                                            if ui.button("Zoom Out").clicked() {
                                                stage_zoom = (stage_zoom / 1.25).max(0.1);
                                                ui.close_menu();
                                            }
                                            if ui.button("Zoom 100%").clicked() {
                                                stage_zoom = 1.0;
                                                ui.close_menu();
                                            }
                                            if ui.button("Fit / центрировать Stage").clicked() {
                                                stage_zoom = 1.0;
                                                stage_pan = egui::Vec2::ZERO;
                                                status_message = "Вид сброшен".into();
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            ui.checkbox(&mut show_rulers, "Линейки");
                                            ui.checkbox(&mut show_timeline, "Timeline");
                                            ui.checkbox(&mut show_right_panel, "Правые панели");
                                            ui.checkbox(&mut show_skeleton, "Скелет");
                                        });

                                        ui.menu_button("Insert", |ui| {
                                            let anim_name = player.current_name().map(str_to_owned);
                                            let can_key = anim_name.is_some() && selected_layer.is_some();
                                            if ui
                                                .add_enabled(can_key, egui::Button::new("Keyframe (позиция выбранного слоя)"))
                                                .on_disabled_hover_text("Нужны активная анимация и выбранный слой")
                                                .clicked()
                                            {
                                                if let (Some(anim_name), Some(layer_id)) = (anim_name, selected_layer.clone()) {
                                                    let bone = character.parts.get(&layer_id).and_then(|p| p.bone.clone());
                                                    if let Some(bone_id) = bone {
                                                        let value = character
                                                            .skeleton
                                                            .find(&bone_id)
                                                            .map(|b| b.local_transform.position.y)
                                                            .unwrap_or(0.0);
                                                        let t = player.time();
                                                        push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                        if let Some(anim) = character.animations.get_mut(&anim_name) {
                                                            insert_keyframe(anim, &bone_id, t, value);
                                                            status_message = format!("Ключ на t={t:.2}с для кости '{bone_id}'");
                                                        }
                                                    } else {
                                                        status_message = "У выбранного слоя нет кости — ключ ставить не к чему".into();
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("Пустой слой (заглушка-цвет)").clicked() {
                                                push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                blank_layer_count += 1;
                                                let id = format!("layer_{blank_layer_count}");
                                                let max_layer = character.parts.values().map(|p| p.layer).max().unwrap_or(0);
                                                character.add_part(
                                                    pony_core::part::Part::new(
                                                        &id,
                                                        pony_core::part::PartKind::Custom,
                                                        pony_core::part::PartSource::Png { path: String::new() },
                                                    )
                                                    .with_bone("Body")
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
                                                    push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                    if let Some(b) = character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                                        b.local_transform.scale.x = -b.local_transform.scale.x;
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            if ui.add_enabled(has_sel, egui::Button::new("Отразить по вертикали")).clicked() {
                                                if let Some(bone_id) = selected_bone_id(&character, &selected_layer) {
                                                    push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                    if let Some(b) = character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                                        b.local_transform.scale.y = -b.local_transform.scale.y;
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            if ui.add_enabled(has_sel, egui::Button::new("Сбросить трансформацию")).clicked() {
                                                if let Some(bone_id) = selected_bone_id(&character, &selected_layer) {
                                                    push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                    if let Some(b) = character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                                        b.local_transform = pony_core::skeleton::Transform2D::default();
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.add_enabled(has_sel, egui::Button::new("Поднять слой выше")).clicked() {
                                                if let Some(id) = selected_layer.clone() {
                                                    push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                    if let Some(p) = character.parts.get_mut(&id) {
                                                        p.layer += 1;
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            if ui.add_enabled(has_sel, egui::Button::new("Опустить слой ниже")).clicked() {
                                                if let Some(id) = selected_layer.clone() {
                                                    push_undo(&mut undo_stack, &mut redo_stack, &character);
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
                                                        push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                        pony_script::apply_commands(&mut character, &mut camera, &mut player, &commands);
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
                                            if ui.button(if playing { "Stop" } else { "Play" }).clicked() {
                                                playing = !playing;
                                                ui.close_menu();
                                            }
                                            if ui.button("Rewind (в начало)").clicked() {
                                                let cur = player.time();
                                                player.advance(&character, -cur);
                                                player.apply(&mut character);
                                                ui.close_menu();
                                            }
                                            if ui.button("К концу анимации").clicked() {
                                                playing = false;
                                                if let Some(dur) = player.current_name().and_then(|n| character.animations.get(n)).map(|a| a.duration) {
                                                    let cur = player.time();
                                                    player.advance(&character, dur - cur);
                                                    player.apply(&mut character);
                                                }
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            ui.weak("Анимации персонажа:");
                                            let mut names: Vec<String> = character.animations.keys().cloned().collect();
                                            names.sort();
                                            for name in names {
                                                let is_current = player.current_name() == Some(name.as_str());
                                                if ui.selectable_label(is_current, &name).clicked() {
                                                    player.play(&name);
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
                                            ui.label(format!("Undo/Redo: {} / {}", undo_stack.len(), redo_stack.len()));
                                        });

                                        ui.menu_button("Window", |ui| {
                                            ui.checkbox(&mut show_timeline, "Timeline");
                                            ui.checkbox(&mut show_right_panel, "Panels");
                                            ui.checkbox(&mut show_rulers, "Линейки");
                                        });
                                        ui.menu_button("Help", |ui| {
                                            ui.weak("pony-engine — см. README.md в репозитории");
                                        });
                                    });
                                });

                                egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
                                    ui.horizontal(|ui| {
                                        if ui.button("New").clicked() {
                                            character = build_walking_pony();
                                            player = AnimationPlayer::new();
                                            player.play("Walk");
                                        }
                                        if ui.button("Open").clicked() {
                                            if let Ok(loaded) = Character::load_from_file(ASSET_PATH) {
                                                character = loaded;
                                                player = AnimationPlayer::new();
                                            }
                                        }
                                        if ui.button("Save").clicked() {
                                            let _ = character.save_to_file(ASSET_PATH);
                                        }
                                        ui.add_enabled(false, egui::Button::new("Undo"));
                                        ui.add_enabled(false, egui::Button::new("Redo"));
                                        ui.add_enabled(false, egui::Button::new("Publish"));
                                        ui.separator();
                                        ui.label("Zoom:");
                                        ui.add(egui::Slider::new(&mut stage_zoom, 0.25..=4.0).show_value(true));
                                        ui.separator();
                                        egui::ComboBox::from_label("Workspace").selected_text("Animator").show_ui(ui, |ui| {
                                            let _ = ui.selectable_label(true, "Animator");
                                        });
                                    });
                                });

                                egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(format!("Инструмент: {}", active_tool.label()));
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
                                        if ui.button("⏮").clicked() {
                                            let cur = player.time();
                                            player.advance(&character, -cur);
                                            player.apply(&mut character);
                                        }
                                        if ui.button(if playing { "⏸" } else { "▶" }).clicked() {
                                            playing = !playing;
                                        }
                                        if ui.button("⏭").clicked() {
                                            playing = false;
                                            if let Some(anim) = player.current_name().and_then(|n| character.animations.get(n)) {
                                                let dur = anim.duration;
                                                let cur = player.time();
                                                player.advance(&character, dur - cur);
                                                player.apply(&mut character);
                                            }
                                        }
                                        ui.separator();
                                        let current_frame = (player.time() * FPS).round() as i64;
                                        ui.label(format!("Кадр: {current_frame}"));
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

                                if show_timeline {
                                    egui::TopBottomPanel::bottom("timeline").min_height(160.0).resizable(true).show(ctx, |ui| {
                                        ui.label("Timeline");
                                        timeline_widget(ui, &mut character, &mut player, &mut playing, &mut hidden_layers, &mut locked_layers, &mut selected_layer);
                                    });
                                }

                                egui::SidePanel::left("tools").resizable(false).min_width(64.0).show(ctx, |ui| {
                                    ui.vertical_centered_justified(|ui| {
                                        for tool in [Tool::Selection, Tool::SubSelection, Tool::FreeTransform, Tool::Lasso] {
                                            if ui.selectable_label(active_tool == tool, tool.label()).clicked() {
                                                active_tool = tool;
                                            }
                                        }
                                        ui.separator();
                                        for tool in [Tool::Pen, Tool::Pencil, Tool::Brush, Tool::Line, Tool::Rectangle, Tool::Oval, Tool::PolyStar] {
                                            if ui.selectable_label(active_tool == tool, tool.label()).clicked() {
                                                active_tool = tool;
                                            }
                                        }
                                        ui.separator();
                                        for tool in [Tool::PaintBucket, Tool::InkBottle, Tool::Eyedropper] {
                                            if ui.selectable_label(active_tool == tool, tool.label()).clicked() {
                                                active_tool = tool;
                                            }
                                        }
                                        ui.separator();
                                        for tool in [Tool::Hand, Tool::Zoom] {
                                            if ui.selectable_label(active_tool == tool, tool.label()).clicked() {
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
                                        if ui.button("Сохранить как слой").clicked() && !vector_doc.is_empty() {
                                            svg_save_count += 1;
                                            let path = format!("drawn_{svg_save_count}.svg");
                                            match std::fs::write(&path, vector_doc.to_svg_string()) {
                                                Ok(()) => {
                                                    push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                    let part_id = format!("drawn_{svg_save_count}");

                                                    // Часть должна встать ТУДА, где нарисована, и
                                                    // ТОГО размера, какого нарисована — иначе
                                                    // «нарисовал в углу» превращалось бы в
                                                    // «появилось в центре персонажа непонятного
                                                    // размера», и рисование было бы бесполезно.
                                                    // Габарит рисунка в SVG-координатах (Y вниз),
                                                    // мир — Y вверх, отсюда минус.
                                                    let (min_x, min_y, max_x, max_y) = vector_doc.bounds().unwrap_or((0.0, 0.0, 1.0, 1.0));
                                                    let draw_center = glam::Vec2::new((min_x + max_x) / 2.0, -(min_y + max_y) / 2.0);
                                                    let draw_size = glam::Vec2::new((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));

                                                    // Привязываем к выбранной кости, если слой
                                                    // выбран, иначе к корню тела — и пересчитываем
                                                    // смещение так, чтобы часть осталась на месте.
                                                    let bone_id = selected_layer
                                                        .as_ref()
                                                        .and_then(|id| character.parts.get(id))
                                                        .and_then(|p| p.bone.clone())
                                                        .unwrap_or_else(|| "Body".to_string());
                                                    let bone_world = character.skeleton.world_transform(&bone_id).unwrap_or_default();
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
                                        if ui.button("Очистить холст").clicked() {
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
                                                                push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                                let old_world = part
                                                                    .bone
                                                                    .as_ref()
                                                                    .and_then(|b| character.skeleton.world_transform(b))
                                                                    .unwrap_or_default();
                                                                let keep_at = pony_render::part_world_position(&part, &old_world);
                                                                let new_world = character.skeleton.world_transform(&target_bone).unwrap_or_default();
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
                                                ui.add(egui::Slider::new(&mut facing_yaw_deg, -89.0..=89.0).suffix("°"));
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
                                                ui.label(format!("Инструмент: {}", active_tool.label()));
                                                if let Some(id) = &selected_layer {
                                                    ui.label(format!("Выбран слой: {id}"));
                                                    if let Some(part) = character.parts.get(id) {
                                                        if let Some(bone_id) = &part.bone {
                                                            if let Some(world) = character.skeleton.world_transform(bone_id) {
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
                                                            push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                            let parent_id = character.skeleton.find(&bid).and_then(|b| b.parent.clone());
                                                            // Сохраняем мировые позиции частей, которые
                                                            // висели на удаляемых костях, ДО удаления —
                                                            // иначе их нечем будет пересчитать после.
                                                            let removed = character.skeleton.remove_subtree(&bid);
                                                            if let Some(parent_id) = parent_id {
                                                                for part in character.parts.values_mut() {
                                                                    let Some(part_bone) = &part.bone else { continue };
                                                                    if removed.contains(part_bone) {
                                                                        // Кость уже удалена — её world_transform
                                                                        // больше не посчитать, поэтому переносим
                                                                        // часть на родителя с тем же смещением
                                                                        // (примерно на месте, не идеально точно,
                                                                        // но не телепортирует непредсказуемо).
                                                                        part.bone = Some(parent_id.clone());
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
                                                                push_undo(&mut undo_stack, &mut redo_stack, &character);
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
                                                                push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                                if character.skeleton.reparent(&bid, &np) {
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
                                            }
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
                                    // CentralPanel и протекают поверх меню сверху и палитры
                                    // инструментов слева. Само это не влияло на данные (кости и
                                    // фигуры считались верно), только на то, что видно на экране —
                                    // но видно было неверно, и это заметил бы любой, кто увеличил Stage.
                                    let painter = ui.painter_at(avail);
                                    let painter = &painter;

                                    painter.rect_filled(avail, 0.0, egui::Color32::from_gray(38));

                                    if active_tool == Tool::Hand && response.dragged() {
                                        stage_pan += response.drag_delta();
                                    }
                                    if response.hovered() {
                                        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                                        if scroll != 0.0 {
                                            stage_zoom = (stage_zoom * (1.0 + scroll * 0.001)).clamp(0.1, 8.0);
                                        }
                                    }
                                    if active_tool == Tool::Zoom && response.clicked() {
                                        stage_zoom = (stage_zoom * 1.25).clamp(0.1, 8.0);
                                    }

                                    let stage_w = SCENE_WIDTH as f32 * stage_zoom;
                                    let stage_h = SCENE_HEIGHT as f32 * stage_zoom;
                                    let center = canvas_rect.center() + stage_pan;
                                    let stage_rect = egui::Rect::from_center_size(center, egui::vec2(stage_w, stage_h));

                                    // Сцена рисуется ПЕРВОЙ — до всех блоков инструментов,
                                    // потому что живое превью фигуры (Rect/Oval/Line/Pencil)
                                    // рисуется прямо внутри этих блоков и должно ложиться
                                    // ПОВЕРХ сцены. Раньше картинка сцены рисовалась после
                                    // них и полностью перекрывала превью — из-за чего
                                    // казалось, что фигура появляется только после отпускания
                                    // кнопки мыши (на самом деле она рисовалась всё время,
                                    // просто была не видна под сценой).
                                    if let Some(tex) = &scene_texture {
                                        painter.image(tex.id(), stage_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                                    }

                                    // Перевод экранных координат в мировые координаты сцены —
                                    // обратное преобразование той же камеры (position/rotation/zoom/
                                    // shake), что и в Renderer::render_character (см. compute_projection
                                    // в pony-render), только в обратную сторону. Не инвертируем сам
                                    // 2.5D-поворот (facing_yaw) — сравниваем клик с уже повёрнутыми
                                    // координатами частей (см. ниже), так проще и корректно.
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

                                    if active_tool == Tool::Selection {
                                        // Клик — выбрать часть под курсором.
                                        if response.clicked() {
                                            if let Some(screen_pos) = response.interact_pointer_pos() {
                                                let click_world = screen_to_world(screen_pos);
                                                let mut best: Option<(String, i32)> = None;
                                                for (id, part) in &character.parts {
                                                    if locked_layers.contains(id) || hidden_layers.contains(id) {
                                                        continue;
                                                    }
                                                    let Some(bone_id) = &part.bone else { continue };
                                                    let Some(world) = character.skeleton.world_transform(bone_id) else { continue };
                                                    // Та же формула, что и у рендера — см.
                                                    // part_world_position в pony-render.
                                                    let part_pos = pony_render::part_world_position(part, &world);
                                                    let depth_z = -(part.layer as f32) * pony_render::DEPTH_PER_LAYER;
                                                    let (yawed_x, foreshorten) = pony_core::apply_yaw_2_5d(part_pos.x, depth_z, character.facing_yaw);
                                                    let size = pony_render::part_render_size(part);
                                                    let half_w = (size.x * world.scale.x.abs() * foreshorten / 2.0).max(2.0);
                                                    let half_h = (size.y * world.scale.y.abs() / 2.0).max(2.0);
                                                    if (yawed_x - click_world.x).abs() <= half_w && (part_pos.y - click_world.y).abs() <= half_h {
                                                        let better = best.as_ref().map(|(_, l)| part.layer > *l).unwrap_or(true);
                                                        if better {
                                                            best = Some((id.clone(), part.layer));
                                                        }
                                                    }
                                                }
                                                selected_layer = best.map(|(id, _)| id);

                                            }
                                        }

                                        // Перетаскивание — двигать САМУ ЧАСТЬ (её смещение
                                        // относительно кости), а не кость: инструмент Xform
                                        // двигает кость и утаскивает всё, что к ней прикреплено,
                                        // а здесь нужно подвинуть один конкретный элемент.
                                        if response.drag_started() {
                                            if selected_layer.is_some() {
                                                push_undo(&mut undo_stack, &mut redo_stack, &character);
                                            }
                                        }
                                        if response.dragged() {
                                            if let (Some(id), Some(pos)) = (selected_layer.clone(), response.interact_pointer_pos()) {
                                                if !locked_layers.contains(&id) {
                                                    let target = screen_to_world(pos);
                                                    let bone_id = character.parts.get(&id).and_then(|p| p.bone.clone());
                                                    if let Some(bone_id) = bone_id {
                                                        let bone_world = character.skeleton.world_transform(&bone_id).unwrap_or_default();
                                                        let new_pivot = pony_render::pivot_for_world_position(target, &bone_world);
                                                        if let Some(p) = character.parts.get_mut(&id) {
                                                            p.pivot = new_pivot;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if active_tool == Tool::FreeTransform && response.dragged() {
                                        if let Some(id) = &selected_layer {
                                            if let Some(bone_id) = character.parts.get(id).and_then(|p| p.bone.clone()) {
                                                let delta_screen = response.drag_delta();
                                                let delta_frame = glam::Vec2::new(delta_screen.x, -delta_screen.y) / stage_zoom;
                                                let delta_after_cam = delta_frame * inv_zoom;
                                                let delta_world = glam::Vec2::new(
                                                    delta_after_cam.x * cos_r - delta_after_cam.y * sin_r,
                                                    delta_after_cam.x * sin_r + delta_after_cam.y * cos_r,
                                                );
                                                if let Some(bone) = character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                                    bone.local_transform.position.x += delta_world.x;
                                                    bone.local_transform.position.y += delta_world.y;
                                                }
                                            }
                                        }
                                    }

                                    if active_tool == Tool::Eyedropper && response.clicked() {
                                        if let (Some(screen_pos), Some(frame)) = (response.interact_pointer_pos(), &last_frame_output) {
                                            let fx = ((screen_pos.x - stage_rect.left()) / stage_zoom).round() as i32;
                                            let fy = ((screen_pos.y - stage_rect.top()) / stage_zoom).round() as i32;
                                            if fx >= 0 && fy >= 0 && (fx as u32) < frame.width && (fy as u32) < frame.height {
                                                let idx = ((fy as u32 * frame.width + fx as u32) * 4) as usize;
                                                fill_color = egui::Color32::from_rgba_unmultiplied(frame.rgba[idx], frame.rgba[idx + 1], frame.rgba[idx + 2], frame.rgba[idx + 3]);
                                            }
                                        }
                                    }

                                    // --- добавить кость кликом по Stage ---
                                    if placing_bone && response.clicked() {
                                        if let Some(screen_pos) = response.interact_pointer_pos() {
                                            let target = screen_to_world(screen_pos);
                                            let parent_id = selected_bone.clone().unwrap_or_else(|| "Root".to_string());
                                            if let Some(parent_world) = character.skeleton.world_transform(&parent_id) {
                                                push_undo(&mut undo_stack, &mut redo_stack, &character);
                                                bone_count += 1;
                                                let new_id = format!("Bone_{bone_count}");
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
                                                (dx * dx + dy * dy).sqrt() < 10.0 / stage_zoom
                                            });
                                            if close_enough_to_start == Some(true) && pen_points.len() >= 3 {
                                                vector_doc.add(pony_core::VectorShape::Polygon {
                                                    points: std::mem::take(&mut pen_points),
                                                    fill: color32_to_rgba(fill_color),
                                                    stroke: color32_to_rgba(stroke_color),
                                                    stroke_width: 1.5,
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
                                            painter.add(egui::Shape::line(screen_pts.clone(), egui::Stroke::new(1.5, stroke_color)));
                                        }
                                        for p in &screen_pts {
                                            painter.circle_filled(*p, 3.0, egui::Color32::from_rgb(120, 200, 255));
                                        }
                                    }

                                    // --- SubSelection: клик выбирает уже нарисованную (не
                                    // сохранённую) фигуру на холсте, дальше её "узлы"
                                    // (control points) можно тащить, меняя саму форму.
                                    if active_tool == Tool::SubSelection {
                                        if response.drag_started() {
                                            // Сначала проверяем, не начали ли тащить УЖЕ
                                            // выбранный узел (приоритет над выбором новой
                                            // фигуры — иначе перетаскивание узла около края
                                            // другой фигуры перевыбирало бы фигуру вместо
                                            // редактирования точки).
                                            dragging_point_index = None;
                                            if let (Some(shape_idx), Some(pos)) = (selected_shape_index, response.interact_pointer_pos()) {
                                                if let Some(shape) = vector_doc.shapes.get(shape_idx) {
                                                    let handle_radius_world = 8.0 / stage_zoom;
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
                                        if response.dragged() {
                                            if let (Some(shape_idx), Some(point_idx), Some(pos)) = (selected_shape_index, dragging_point_index, response.interact_pointer_pos()) {
                                                let w = screen_to_world(pos);
                                                if let Some(shape) = vector_doc.shapes.get_mut(shape_idx) {
                                                    shape.set_control_point(point_idx, (w.x, -w.y));
                                                }
                                            }
                                        }
                                        if response.drag_stopped() {
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
                                                    painter.rect_stroke(egui::Rect::from_center_size(p, egui::vec2(7.0, 7.0)), 1.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 140, 220)));
                                                }
                                            }
                                        }
                                    }

                                    // --- Lasso: рамкой выделить сразу несколько частей
                                    // персонажа (по их мировой позиции — та же формула,
                                    // что и у остальных hit-тестов в этом файле) ---
                                    if active_tool == Tool::Lasso {
                                        if response.drag_started() {
                                            lasso_start = response.interact_pointer_pos().map(screen_to_world);
                                        }
                                        if response.dragged() {
                                            if let (Some(start), Some(cur_screen)) = (lasso_start, response.interact_pointer_pos()) {
                                                let start_screen = world_to_screen(start);
                                                painter.rect_stroke(
                                                    egui::Rect::from_two_pos(start_screen, cur_screen),
                                                    0.0,
                                                    egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 230, 140)),
                                                );
                                            }
                                        }
                                        if response.drag_stopped() {
                                            if let (Some(start), Some(end_screen)) = (lasso_start.take(), response.interact_pointer_pos()) {
                                                let end = screen_to_world(end_screen);
                                                let (min_x, max_x) = (start.x.min(end.x), start.x.max(end.x));
                                                let (min_y, max_y) = (start.y.min(end.y), start.y.max(end.y));
                                                multi_selected.clear();
                                                for (id, part) in &character.parts {
                                                    let Some(bone_id) = &part.bone else { continue };
                                                    let Some(world) = character.skeleton.world_transform(bone_id) else { continue };
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
                                            if let Some(world) = part.bone.as_ref().and_then(|b| character.skeleton.world_transform(b)) {
                                                let pos = pony_render::part_world_position(part, &world);
                                                let size = pony_render::part_render_size(part);
                                                let half_w = size.x * world.scale.x.abs() / 2.0;
                                                let half_h = size.y * world.scale.y.abs() / 2.0;
                                                let p0 = world_to_screen(glam::Vec2::new(pos.x - half_w, pos.y - half_h));
                                                let p1 = world_to_screen(glam::Vec2::new(pos.x + half_w, pos.y + half_h));
                                                painter.rect_stroke(egui::Rect::from_two_pos(p0, p1), 0.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 230, 140)));
                                            }
                                        }
                                    }

                                    // --- рисование фигур (раздел 16 ТЗ: SVG теперь не только импорт) ---
                                    let is_shape_tool = matches!(active_tool, Tool::Rectangle | Tool::Oval | Tool::Line | Tool::PolyStar);
                                    if is_shape_tool {
                                        if response.drag_started() {
                                            draw_start = response.interact_pointer_pos().map(screen_to_world);
                                        }
                                        if response.dragged() {
                                            // Живое превью — рисуем прямо в экранных координатах поверх
                                            // сцены, ничего пока не коммитим в vector_doc.
                                            if let (Some(start_world), Some(cur_screen)) = (draw_start, response.interact_pointer_pos()) {
                                                let start_screen = world_to_screen(start_world);
                                                let preview_stroke = egui::Stroke::new(1.5, stroke_color);
                                                match active_tool {
                                                    Tool::Rectangle => {
                                                        painter.rect_stroke(egui::Rect::from_two_pos(start_screen, cur_screen), 0.0, preview_stroke);
                                                    }
                                                    Tool::Oval => {
                                                        let center = start_screen.lerp(cur_screen, 0.5);
                                                        let r = ((cur_screen.x - start_screen.x).abs() / 2.0).max((cur_screen.y - start_screen.y).abs() / 2.0);
                                                        painter.circle_stroke(center, r, preview_stroke);
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
                                        if response.drag_stopped() {
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
                                                        Some(pony_core::VectorShape::Polygon { points, fill, stroke, stroke_width: 1.5 })
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
                                        if response.dragged() {
                                            if let Some(pos) = response.interact_pointer_pos() {
                                                let w = screen_to_world(pos);
                                                pencil_points.push((w.x, -w.y));
                                            }
                                        }
                                        if response.drag_stopped() && pencil_points.len() >= 2 {
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
                                            painter.add(egui::Shape::line(screen_pts, egui::Stroke::new(2.0, stroke_color)));
                                        }
                                    }

                                    // Для СЛЕДУЮЩЕГО кадра: был ли этот кадр перетаскиванием
                                    // фигуры рисования — от этого зависит, пропустит ли
                                    // следующий кадр дорогой рендер персонажа (см. объявление
                                    // `was_dragging_shape_last_frame` в начале функции).
                                    was_dragging_shape_last_frame =
                                        response.dragged() && matches!(active_tool, Tool::Rectangle | Tool::Oval | Tool::Line | Tool::Pencil | Tool::Brush);

                                    // Уже нарисованные фигуры — поверх сцены, в тех же экранных
                                    // координатах, что и всё остальное на Stage.
                                    for shape in &vector_doc.shapes {
                                        draw_vector_shape_preview(painter, shape, &world_to_screen);
                                    }

                                    // Визуализация скелета — без неё костями просто нечем было бы
                                    // управлять: непонятно, где они, что чей родитель, что выбрано.
                                    if show_skeleton {
                                        for bone in &character.skeleton.bones {
                                            let Some(world) = character.skeleton.world_transform(&bone.id) else { continue };
                                            let p = world_to_screen(glam::Vec2::new(world.position.x, world.position.y));
                                            let is_sel = selected_bone.as_deref() == Some(bone.id.as_str());
                                            let color = if is_sel { egui::Color32::from_rgb(255, 210, 60) } else { egui::Color32::from_rgb(120, 190, 255) };
                                            if let Some(parent_id) = &bone.parent {
                                                if let Some(parent_world) = character.skeleton.world_transform(parent_id) {
                                                    let pp = world_to_screen(glam::Vec2::new(parent_world.position.x, parent_world.position.y));
                                                    painter.line_segment([pp, p], egui::Stroke::new(1.5, color.gamma_multiply(0.7)));
                                                }
                                            }
                                            painter.circle_filled(p, if is_sel { 5.0 } else { 3.5 }, color);
                                            if is_sel {
                                                painter.circle_stroke(p, 8.0, egui::Stroke::new(1.5, color));
                                            }
                                        }
                                    }
                                    if placing_bone {
                                        if let Some(hover) = response.hover_pos() {
                                            painter.circle_stroke(hover, 6.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 210, 60)));
                                        }
                                    }

                                    // Обводка выбранной части — иначе непонятно, что именно
                                    // сейчас двигаешь и попал ли клик туда, куда целился.
                                    if let Some(sel_id) = &selected_layer {
                                        if let Some(part) = character.parts.get(sel_id) {
                                            if let Some(world) = part.bone.as_ref().and_then(|b| character.skeleton.world_transform(b)) {
                                                let part_pos = pony_render::part_world_position(part, &world);
                                                let depth_z = -(part.layer as f32) * pony_render::DEPTH_PER_LAYER;
                                                let (yawed_x, foreshorten) = pony_core::apply_yaw_2_5d(part_pos.x, depth_z, character.facing_yaw);
                                                let size = pony_render::part_render_size(part);
                                                let half_w = size.x * world.scale.x.abs() * foreshorten / 2.0;
                                                let half_h = size.y * world.scale.y.abs() / 2.0;
                                                let p0 = world_to_screen(glam::Vec2::new(yawed_x - half_w, part_pos.y - half_h));
                                                let p1 = world_to_screen(glam::Vec2::new(yawed_x + half_w, part_pos.y + half_h));
                                                let rect = egui::Rect::from_two_pos(p0, p1);
                                                painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(90, 170, 255)));
                                                // Точка привязки к кости — видно, вокруг чего
                                                // часть будет вращаться при повороте кости.
                                                let bone_screen = world_to_screen(glam::Vec2::new(world.position.x, world.position.y));
                                                painter.circle_stroke(bone_screen, 4.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 190, 90)));
                                                painter.line_segment([bone_screen, rect.center()], egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 190, 90)));
                                            }
                                        }
                                    }

                                    painter.rect_stroke(stage_rect, 0.0, egui::Stroke::new(1.5, egui::Color32::from_gray(15)));

                                    painter.rect_filled(egui::Rect::from_min_max(avail.min, egui::pos2(avail.right(), avail.top() + ruler)), 0.0, egui::Color32::from_gray(50));
                                    painter.rect_filled(egui::Rect::from_min_max(avail.min, egui::pos2(avail.left() + ruler, avail.bottom())), 0.0, egui::Color32::from_gray(50));
                                    let mut rx = stage_rect.left();
                                    while rx < canvas_rect.right() {
                                        if rx >= canvas_rect.left() {
                                            painter.line_segment([egui::pos2(rx, avail.top()), egui::pos2(rx, avail.top() + ruler)], egui::Stroke::new(1.0, egui::Color32::from_gray(110)));
                                        }
                                        rx += 50.0 * stage_zoom;
                                    }
                                    let mut ry = stage_rect.top();
                                    while ry < canvas_rect.bottom() {
                                        if ry >= canvas_rect.top() {
                                            painter.line_segment([egui::pos2(avail.left(), ry), egui::pos2(avail.left() + ruler, ry)], egui::Stroke::new(1.0, egui::Color32::from_gray(110)));
                                        }
                                        ry += 50.0 * stage_zoom;
                                    }

                                    mouse_stage_pos = response.hover_pos().map(|p| egui::pos2((p.x - stage_rect.left()) / stage_zoom, (p.y - stage_rect.top()) / stage_zoom));
                                });
                            });

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
                            window.request_redraw();
                        }
                        _ => {}
                    }
                }
                Event::AboutToWait => window.request_redraw(),
                _ => {}
            }
        })
        .expect("event loop error");
}
