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
use pony_render::{GpuContext, Renderer};
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
    let mut pony_renderer = Renderer::new(&gpu_ctx);

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

                            let mut visible_character = character.clone();
                            visible_character.parts.retain(|id, _| !hidden_layers.contains(id));
                            let frame = pony_renderer.render_character(&gpu_ctx, &visible_character, SCENE_WIDTH, SCENE_HEIGHT, &camera, elapsed_time);
                            let color_image = egui::ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &frame.rgba);
                            match &mut scene_texture {
                                Some(handle) => handle.set(color_image, egui::TextureOptions::NEAREST),
                                None => scene_texture = Some(egui_ctx.load_texture("pony-scene", color_image, egui::TextureOptions::NEAREST)),
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
                                            if ui.button("Exit").clicked() {
                                                elwt.exit();
                                            }
                                        });
                                        for name in ["Edit", "View", "Insert", "Modify", "Text", "Commands"] {
                                            ui.menu_button(name, |ui| {
                                                ui.weak("(не реализовано в этой версии)");
                                            });
                                        }
                                        ui.menu_button("Control", |ui| {
                                            if ui.button(if playing { "Stop" } else { "Play" }).clicked() {
                                                playing = !playing;
                                                ui.close_menu();
                                            }
                                            if ui.button("Rewind").clicked() {
                                                let cur = player.time();
                                                player.advance(&character, -cur);
                                                player.apply(&mut character);
                                                ui.close_menu();
                                            }
                                        });
                                        ui.menu_button("Debug", |ui| {
                                            ui.weak("(не реализовано в этой версии)");
                                        });
                                        ui.menu_button("Window", |ui| {
                                            ui.checkbox(&mut show_timeline, "Timeline");
                                            ui.checkbox(&mut show_right_panel, "Panels");
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
                                            ] {
                                                if ui.selectable_label(right_tab == tab, tab.label()).clicked() {
                                                    right_tab = tab;
                                                }
                                            }
                                        });
                                        ui.separator();
                                        match right_tab {
                                            RightTab::Properties => match &selected_layer {
                                                Some(id) => {
                                                    if let Some(part) = character.parts.get(id).cloned() {
                                                        ui.label(format!("ID: {}", part.id));
                                                        ui.label(format!("Вид: {:?}", part.kind));
                                                        ui.label(format!("Кость: {}", part.bone.as_deref().unwrap_or("(нет)")));
                                                        let mut layer = part.layer;
                                                        if ui.add(egui::DragValue::new(&mut layer).prefix("Порядок отрисовки: ")).changed() {
                                                            if let Some(p) = character.parts.get_mut(id) {
                                                                p.layer = layer;
                                                            }
                                                        }
                                                    }
                                                }
                                                None => {
                                                    ui.label("(слой не выбран — кликни по имени в Timeline)");
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
                                            RightTab::Transform => match &selected_layer {
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
                                            },
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
                                        }
                                    });
                                }

                                egui::CentralPanel::default().show(ctx, |ui| {
                                    let avail = ui.available_rect_before_wrap();
                                    let ruler = 16.0;
                                    let canvas_rect = egui::Rect::from_min_max(egui::pos2(avail.left() + ruler, avail.top() + ruler), avail.max);
                                    let response = ui.interact(canvas_rect, ui.id().with("stage_canvas"), egui::Sense::click_and_drag());
                                    let painter = ui.painter();

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

                                    let stage_w = SCENE_WIDTH as f32 * stage_zoom;
                                    let stage_h = SCENE_HEIGHT as f32 * stage_zoom;
                                    let center = canvas_rect.center() + stage_pan;
                                    let stage_rect = egui::Rect::from_center_size(center, egui::vec2(stage_w, stage_h));

                                    if let Some(tex) = &scene_texture {
                                        painter.image(tex.id(), stage_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
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
