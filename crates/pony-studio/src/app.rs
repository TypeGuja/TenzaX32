//! Состояние редактора и все команды. Веб-интерфейс (ui/*.js) только
//! показывает состояние и присылает команды: модель — персонажи, скелет,
//! анимации, холст рисования, история — живёт здесь, в Rust, и меняется
//! только через `App::handle`.
//!
//! Ответ на каждую команду — изменившиеся разделы состояния (`ui`, `doc`,
//! `anim`, `play`, `frame`, `vec`, `env`, `xfl`, `xfl_preview`): раздел
//! сериализуется и отправляется, только если он отличается от того, что
//! интерфейс уже получил.
//!
//! Логика перенесена из egui-интерфейса `pony-gui` (те же правила
//! выделения, Free Transform, ключей таймлайна, сохранения рисунка и т.д.).

use std::collections::{BTreeSet, HashMap, HashSet};

use glam::Vec2;
use pony_core::animation::{Animation, AnimTarget, AnimValue, BoneChannel, Interpolation, Keyframe, Track};
use pony_core::part::{Part, PartKind, PartSource};
use pony_core::{AnimationPlayer, Character, RgbaColor, VectorDoc, VectorShape};
use serde_json::{json, Value};

use crate::anim::{self, FPS};
use crate::scene;

/// Фоновая задача: результат — текст для строки состояния.
pub type Job = Box<dyn FnOnce() -> Result<String, String> + Send>;

pub const SCENE_WIDTH: u32 = 960;
pub const SCENE_HEIGHT: u32 = 720;
const BLINK_SECONDS: f32 = 0.25;
const MAX_UNDO_DEPTH: usize = 100;

// ------------------------------------------------------------ история

#[derive(Clone, PartialEq)]
struct DocState {
    character: Character,
    vector_doc: VectorDoc,
}

/// История правок. Фиксируется автоматически после каждой команды, если
/// кнопка мыши уже отпущена: один жест (перетаскивание, правка поля) — один
/// шаг отмены. Поза, которую плеер пишет в кости, правкой не считается.
struct History {
    undo: Vec<DocState>,
    redo: Vec<DocState>,
    committed: DocState,
    revision: u64,
}

impl History {
    fn new(character: &Character, vector_doc: &VectorDoc) -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), committed: DocState { character: character.clone(), vector_doc: vector_doc.clone() }, revision: 0 }
    }

    fn commit_if_changed(&mut self, character: &Character, vector_doc: &VectorDoc, player: &AnimationPlayer) -> bool {
        if self.committed.character == *character && self.committed.vector_doc == *vector_doc {
            return false;
        }
        if self.committed.vector_doc == *vector_doc {
            let mut posed = self.committed.character.clone();
            player.apply(&mut posed);
            if posed == *character {
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

    fn undo(&mut self, character: &Character, vector_doc: &VectorDoc) -> Option<DocState> {
        let prev = self.undo.pop()?;
        self.redo.push(DocState { character: character.clone(), vector_doc: vector_doc.clone() });
        self.committed = prev.clone();
        self.revision += 1;
        Some(prev)
    }

    fn redo(&mut self, character: &Character, vector_doc: &VectorDoc) -> Option<DocState> {
        let next = self.redo.pop()?;
        self.undo.push(DocState { character: character.clone(), vector_doc: vector_doc.clone() });
        self.committed = next.clone();
        self.revision += 1;
        Some(next)
    }

    fn reset(&mut self, character: &Character, vector_doc: &VectorDoc) {
        self.undo.clear();
        self.redo.clear();
        self.committed = DocState { character: character.clone(), vector_doc: vector_doc.clone() };
        self.revision += 1;
    }
}

// ------------------------------------------------------------ инструменты

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
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

const TOOLS: [(Tool, &str); 16] = [
    (Tool::Selection, "sel"),
    (Tool::SubSelection, "subsel"),
    (Tool::FreeTransform, "xform"),
    (Tool::Lasso, "lasso"),
    (Tool::Pen, "pen"),
    (Tool::Pencil, "pencil"),
    (Tool::Brush, "brush"),
    (Tool::Line, "line"),
    (Tool::Rectangle, "rect"),
    (Tool::Oval, "oval"),
    (Tool::PolyStar, "poly"),
    (Tool::PaintBucket, "bucket"),
    (Tool::InkBottle, "ink"),
    (Tool::Eyedropper, "drop"),
    (Tool::Hand, "hand"),
    (Tool::Zoom, "zoom"),
];

impl Tool {
    fn key(self) -> &'static str {
        TOOLS.iter().find(|(t, _)| *t == self).map(|(_, k)| *k).unwrap_or("sel")
    }
    fn from_key(k: &str) -> Option<Tool> {
        TOOLS.iter().find(|(_, key)| *key == k).map(|(t, _)| *t)
    }
}

// ------------------------------------------------------------ персонажи

fn empty_character() -> Character {
    let mut character = Character::new("Новый персонаж");
    character.skeleton.add_bone(pony_core::skeleton::Bone { id: "Root".into(), parent: None, local_transform: pony_core::skeleton::Transform2D::default(), length: 0.0 });
    character
}

/// Кость для новых частей: "Body", если есть, иначе корневая.
fn default_bone(character: &Character) -> String {
    if character.skeleton.find("Body").is_some() {
        return "Body".into();
    }
    character.skeleton.bones.iter().find(|b| b.parent.is_none()).map(|b| b.id.clone()).unwrap_or_else(|| "Root".into())
}

/// Демо-персонаж (File > Демо-пони): части из assets/ и анимация "Walk".
pub fn build_walking_pony() -> Character {
    let mut character = Character::new("GuiPony");
    character.skeleton = pony_core::skeleton::default_pony_skeleton();
    let png = |p: &str| PartSource::Png { path: p.into() };
    let svg = |p: &str| PartSource::Vector { path: p.into() };
    character
        .add_part(Part::new("body", PartKind::Body, png("assets/pony/body.png")).with_bone("Body").with_layer(0))
        .add_part(Part::new("head", PartKind::Head, png("assets/pony/head.png")).with_bone("Head").with_layer(1))
        .add_part(Part::new("horn", PartKind::Horn, png("assets/pony/horn.png")).with_bone("Horn").with_layer(2))
        .add_part(Part::new("ear_l", PartKind::Ear, png("assets/pony/ear.png")).with_bone("EarL").with_layer(2))
        .add_part(Part::new("ear_r", PartKind::Ear, png("assets/pony/ear.png")).with_bone("EarR").with_layer(2))
        .add_part(Part::new("eye_l", PartKind::Eyes, png("assets/pony/eye.png")).with_bone("Head").with_layer(2))
        .add_part(Part::new("leg_fl", PartKind::LegFL, png("assets/pony/leg.png")).with_bone("LowerLegFL").with_layer(-1))
        .add_part(Part::new("leg_fr", PartKind::LegFR, png("assets/pony/leg.png")).with_bone("LowerLegFR").with_layer(-1))
        .add_part(Part::new("wing_l", PartKind::Wing, svg("assets/pony_svg/wing.svg")).with_bone("Body").with_layer(1))
        .add_part(Part::new("tail", PartKind::Tail, svg("assets/pony_svg/tail.svg")).with_bone("Body").with_layer(-1))
        .add_part(Part::new("mouth", PartKind::Mouth, svg("assets/pony_svg/mouth.svg")).with_bone("Head").with_layer(2));
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

struct Slot {
    character: Character,
    player: AnimationPlayer,
    name: String,
}

/// Центр и половины габарита части в мире — как её видно на Stage.
fn part_screen_box(character: &Character, part: &Part) -> Option<(Vec2, Vec2)> {
    let world = character.skeleton.world_transform_with_ik(part.bone.as_ref()?)?;
    let part_pos = pony_render::part_world_position(part, &world);
    let depth_z = pony_render::part_depth(character, part);
    let (yawed_x, foreshorten) = pony_core::apply_yaw_2_5d(part_pos.x, depth_z, character.facing_yaw);
    let size = pony_render::part_render_size(part);
    let morph = pony_render::part_morph_scale(character, part);
    let half = Vec2::new(size.x * world.scale.x.abs() * foreshorten * morph.x / 2.0, size.y * world.scale.y.abs() * morph.y / 2.0);
    Some((Vec2::new(yawed_x, part_pos.y), half))
}

fn part_at(character: &Character, hidden: &HashSet<String>, locked: &HashSet<String>, world: Vec2) -> Option<String> {
    let mut best: Option<(String, i32)> = None;
    for (id, part) in &character.parts {
        if locked.contains(id) || hidden.contains(id) || character.is_part_hidden_by_group(id) {
            continue;
        }
        let Some((center, half)) = part_screen_box(character, part) else { continue };
        if (center.x - world.x).abs() <= half.x.max(2.0) && (center.y - world.y).abs() <= half.y.max(2.0) {
            let better = best.as_ref().map(|(bid, l)| (part.layer, id) > (*l, bid)).unwrap_or(true);
            if better {
                best = Some((id.clone(), part.layer));
            }
        }
    }
    best.map(|(id, _)| id)
}

fn character_bounds(character: &Character, hidden: &HashSet<String>) -> Option<(Vec2, Vec2)> {
    let mut acc: Option<(Vec2, Vec2)> = None;
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

fn file_stem_for(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    if s.is_empty() {
        "pony".into()
    } else {
        s
    }
}

fn xfl_import_dir(doc_name: &str, symbol: Option<&str>, frame: usize) -> std::path::PathBuf {
    let clean = |s: &str| -> String { s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect() };
    std::path::Path::new("imported").join(clean(doc_name)).join(format!("{}_f{frame}", clean(symbol.unwrap_or("main"))))
}

fn default_xfl_symbol(doc: &pony_core::xfl::XflDocument) -> Option<String> {
    let names = doc.symbol_names();
    names
        .iter()
        .find(|n| n.starts_with('~') && n.to_lowercase().contains("character"))
        .or_else(|| names.iter().find(|n| n.starts_with('~')))
        .map(|n| n.to_string())
}

fn shape_kind_label(shape: &VectorShape) -> &'static str {
    match shape {
        VectorShape::Rect { .. } => "Rect",
        VectorShape::Ellipse { .. } => "Ellipse",
        VectorShape::Polygon { .. } => "Polygon",
        VectorShape::Path { closed: true, .. } => "Path (closed)",
        VectorShape::Path { .. } => "Path (open)",
        VectorShape::Line { .. } => "Line",
        VectorShape::Polyline { .. } => "Polyline",
        VectorShape::Instance { .. } => "Instance",
    }
}

fn source_label(source: &PartSource) -> String {
    match source {
        PartSource::Png { path } => format!("[PNG] {path}"),
        PartSource::Vector { path } => format!("[SVG] {path}"),
        PartSource::Psd { path, layer: Some(name) } => format!("[PSD] {path}#{name}"),
        PartSource::Psd { path, layer: None } => format!("[PSD] {path}"),
        PartSource::Kra { path, layer_file: Some(name) } => format!("[KRA] {path}!{name}"),
        PartSource::Kra { path, layer_file: None } => format!("[KRA] {path}"),
        PartSource::Mesh { path } => format!("[MESH] {path}"),
    }
}

fn rgba_json(c: RgbaColor) -> Value {
    json!([c.r, c.g, c.b, c.a])
}

fn rgb_u8(c: [f32; 3]) -> [u8; 3] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

// ------------------------------------------------------------ аргументы

trait Args {
    fn s(&self, k: &str) -> Option<&str>;
    fn f(&self, k: &str) -> Option<f32>;
    fn b(&self, k: &str) -> bool;
    fn i(&self, k: &str) -> Option<i64>;
    fn color(&self, k: &str) -> Option<RgbaColor>;
    fn rgb(&self, k: &str) -> Option<[f32; 3]>;
    fn pt(&self, kx: &str, ky: &str) -> Vec2;
}

impl Args for Value {
    fn s(&self, k: &str) -> Option<&str> {
        self.get(k).and_then(Value::as_str)
    }
    fn f(&self, k: &str) -> Option<f32> {
        self.get(k).and_then(Value::as_f64).map(|v| v as f32).filter(|v| v.is_finite())
    }
    fn b(&self, k: &str) -> bool {
        self.get(k).and_then(Value::as_bool).unwrap_or(false)
    }
    fn i(&self, k: &str) -> Option<i64> {
        self.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64)))
    }
    fn color(&self, k: &str) -> Option<RgbaColor> {
        let a = self.get(k)?.as_array()?;
        let c = |i: usize| a.get(i).and_then(Value::as_u64).map(|v| v.min(255) as u8);
        Some(RgbaColor::new(c(0)?, c(1)?, c(2)?, c(3).unwrap_or(255)))
    }
    fn rgb(&self, k: &str) -> Option<[f32; 3]> {
        self.color(k).map(|c| [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0])
    }
    fn pt(&self, kx: &str, ky: &str) -> Vec2 {
        Vec2::new(self.f(kx).unwrap_or(0.0), self.f(ky).unwrap_or(0.0))
    }
}

// ------------------------------------------------------------ App

struct XflState {
    path: String,
    doc: Option<pony_core::xfl::XflDocument>,
    symbol: Option<String>,
    frame: usize,
    opts: pony_core::xfl::ImportOptions,
    info: String,
    show: u32,
}

pub struct App {
    character: Character,
    player: AnimationPlayer,
    others: Vec<Slot>,
    active_name: String,
    new_character_count: u32,
    timeline_all: bool,

    camera: pony_core::Camera,
    script_engine: pony_script::ScriptEngine,
    script_log: String,
    playing: bool,
    elapsed: f32,
    hidden: HashSet<String>,
    locked: HashSet<String>,
    selected_layer: Option<String>,
    tool: Tool,
    fill: RgbaColor,
    stroke: RgbaColor,
    status: String,

    lighting: pony_core::Lighting,
    sun_enabled: bool,
    sun_color: [f32; 3],
    sun_intensity: f32,
    point_enabled: bool,
    point_pos: Vec2,
    point_color: [f32; 3],
    point_intensity: f32,
    point_radius: f32,

    particles_enabled: bool,
    particle_kind: pony_core::ParticleKind,
    emitter: pony_core::ParticleEmitter,

    vector_doc: VectorDoc,
    history: History,
    saved_revision: u64,
    svg_save_count: u32,
    import_count: u32,
    blank_layer_count: u32,
    asset_path: String,
    xfl: XflState,

    poly_sides: u32,
    selected_bone: Option<String>,
    placing_bone: bool,
    show_skeleton: bool,
    bone_count: u32,
    selected_shape: Option<usize>,
    dragging_point: Option<usize>,
    multi_selected: HashSet<String>,
    editing_part: Option<String>,
    /// viewBox SVG-файла редактируемой части (x, y, w, h): по нему холст
    /// накладывается поверх самой части (см. `doc_map`).
    edit_vb: Option<[f32; 4]>,
    timeline_channel: BoneChannel,
    selected_keyframe: Option<(String, i64)>,
    blink_timer: f32,
    grab_offset: Option<Vec2>,
    fit_count: u32,
    fit_kind: &'static str,
    pub exit_requested: bool,
    /// Долгая работа (экспорт) — выполняется в отдельном потоке (см. main.rs),
    /// чтобы окно не замирало.
    jobs: Vec<Job>,
    exporting: bool,

    last_sent: HashMap<&'static str, String>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let character = empty_character();
        let vector_doc = VectorDoc::new();
        let history = History::new(&character, &vector_doc);
        let particle_kind = pony_core::ParticleKind::Snow;
        Self {
            character,
            player: AnimationPlayer::new(),
            others: Vec::new(),
            active_name: "Pony 1".into(),
            new_character_count: 1,
            timeline_all: false,
            camera: pony_core::Camera::default(),
            script_engine: pony_script::ScriptEngine::new(),
            script_log: String::new(),
            playing: false,
            elapsed: 0.0,
            hidden: HashSet::new(),
            locked: HashSet::new(),
            selected_layer: None,
            tool: Tool::Selection,
            fill: RgbaColor::new(222, 150, 195, 255),
            stroke: RgbaColor::new(90, 60, 70, 255),
            status: "Готово".into(),
            lighting: pony_core::Lighting::default(),
            sun_enabled: false,
            sun_color: [1.0, 230.0 / 255.0, 180.0 / 255.0],
            sun_intensity: 0.3,
            point_enabled: false,
            point_pos: Vec2::ZERO,
            point_color: [1.0, 140.0 / 255.0, 40.0 / 255.0],
            point_intensity: 1.5,
            point_radius: 120.0,
            particles_enabled: false,
            particle_kind,
            emitter: pony_core::ParticleEmitter::new(particle_kind, Vec2::new(0.0, 90.0), 8.0),
            vector_doc,
            history,
            saved_revision: 0,
            svg_save_count: 0,
            import_count: 0,
            blank_layer_count: 0,
            asset_path: String::new(),
            xfl: XflState { path: String::new(), doc: None, symbol: None, frame: 0, opts: pony_core::xfl::ImportOptions { scale: 1.0, expand_containers: true }, info: String::new(), show: 0 },
            poly_sides: 6,
            selected_bone: None,
            placing_bone: false,
            show_skeleton: true,
            bone_count: 0,
            selected_shape: None,
            dragging_point: None,
            multi_selected: HashSet::new(),
            editing_part: None,
            edit_vb: None,
            timeline_channel: BoneChannel::PositionY,
            selected_keyframe: None,
            blink_timer: 0.0,
            grab_offset: None,
            fit_count: 1,
            fit_kind: "character",
            exit_requested: false,
            jobs: Vec::new(),
            exporting: false,
            last_sent: HashMap::new(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.history.revision != self.saved_revision
    }

    pub fn title(&self) -> String {
        let doc_name = if self.asset_path.is_empty() { "Без имени".to_string() } else { self.asset_path.clone() };
        format!("Pony Studio — {} — {doc_name}{}", self.active_name, if self.dirty() { " *" } else { "" })
    }

    fn busy(&self) -> bool {
        self.playing || self.particles_enabled || self.camera.shake_intensity > 0.0 || self.blink_timer > 0.0
    }

    fn fit(&mut self, kind: &'static str) {
        self.fit_count += 1;
        self.fit_kind = kind;
    }

    fn current_anim(&self) -> Option<&Animation> {
        self.player.current_name().and_then(|n| self.character.animations.get(n))
    }

    fn selected_part_bone(&self) -> Option<String> {
        self.selected_layer.as_ref().and_then(|id| self.character.parts.get(id)).and_then(|p| p.bone.clone())
    }

    fn update_lighting(&mut self) {
        self.lighting.sun = self.sun_enabled.then_some(pony_core::SunLight { color: self.sun_color, intensity: self.sun_intensity });
        self.lighting.points = if self.point_enabled {
            vec![pony_core::PointLight { position: self.point_pos, color: self.point_color, intensity: self.point_intensity, radius: self.point_radius }]
        } else {
            Vec::new()
        };
    }

    /// Персонаж, каким его видно: без скрытых слоёв/групп, с морганием.
    fn visible_character(&self) -> Character {
        let mut visible = self.character.clone();
        visible.parts.retain(|id, _| !self.hidden.contains(id) && !self.character.is_part_hidden_by_group(id));
        if self.blink_timer > 0.0 {
            let phase = 1.0 - self.blink_timer / BLINK_SECONDS;
            let closed = (phase * std::f32::consts::PI).sin();
            let current = visible.default_morph.get("Blink");
            visible.default_morph.set("Blink", current.max(closed));
        }
        visible
    }

    fn take_blink(&mut self) {
        if self.character.default_morph.get("Blink") > 0.0 {
            self.character.default_morph.set("Blink", 0.0);
            self.blink_timer = BLINK_SECONDS;
        }
    }

    /// Координаты холста рисования (SVG, Y вниз) -> мир. Обычно это просто
    /// отражение по Y, а пока правится SVG-часть — ещё и её viewBox,
    /// натянутый на квад части: фигуры видно ровно там, где нарисована часть.
    fn doc_map(&self) -> scene::Affine {
        let flip = scene::Affine([1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);
        let (Some(id), Some([vx, vy, vw, vh])) = (&self.editing_part, self.edit_vb) else { return flip };
        let Some(part) = self.character.parts.get(id) else { return flip };
        let model = scene::Affine::from_mat4(&pony_render::part_model_matrix(part, &self.character));
        let to_quad = scene::Affine([1.0 / vw, 0.0, 0.0, -1.0 / vh, -vx / vw - 0.5, vy / vh + 0.5]);
        model.mul(&to_quad)
    }

    /// Мировая точка -> координаты холста рисования.
    fn to_doc(&self, w: Vec2) -> (f32, f32) {
        self.doc_map().inverse().apply(w.x, w.y)
    }

    fn doc_points(&self, a: &Value) -> Vec<(f32, f32)> {
        let inv = self.doc_map().inverse();
        a.get("points")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(|p| Some(inv.apply(p.get(0)?.as_f64()? as f32, p.get(1)?.as_f64()? as f32))).collect())
            .unwrap_or_default()
    }

    /// Сбросить выделение и прочее, что относится к прежнему персонажу.
    fn clear_selection(&mut self) {
        self.selected_layer = None;
        self.selected_bone = None;
        self.selected_keyframe = None;
        self.multi_selected.clear();
    }

    // -------------------------------------------------------- протокол

    /// Обработать одно сообщение веб-интерфейса и вернуть ответ.
    pub fn handle(&mut self, msg: &Value) -> Value {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let cmd = msg.s("cmd").unwrap_or("").to_string();
        let args = msg.get("args").cloned().unwrap_or(Value::Null);
        let down = msg.b("down");
        let result = self.dispatch(&cmd, &args);
        if let Err(e) = &result {
            self.status = e.clone();
        }
        if !down {
            self.history.commit_if_changed(&self.character, &self.vector_doc, &self.player);
        }
        let mut reply = json!({ "id": id, "patch": self.patch() });
        match result {
            Ok(v) => reply["result"] = v,
            Err(e) => reply["error"] = json!(e),
        }
        reply
    }

    /// Изменившиеся разделы состояния.
    fn patch(&mut self) -> Value {
        let mut out = serde_json::Map::new();
        let sections: [(&'static str, Value); 9] = [
            ("ui", self.sec_ui()),
            ("doc", self.sec_doc()),
            ("anim", self.sec_anim()),
            ("play", self.sec_play()),
            ("frame", self.sec_frame()),
            ("vec", self.sec_vec()),
            ("env", self.sec_env()),
            ("xfl", self.sec_xfl()),
            ("xfl_preview", self.sec_xfl_preview()),
        ];
        for (name, value) in sections {
            let text = value.to_string();
            if self.last_sent.get(name) != Some(&text) {
                self.last_sent.insert(name, text);
                out.insert(name.to_string(), value);
            }
        }
        Value::Object(out)
    }

    fn dispatch(&mut self, cmd: &str, a: &Value) -> Result<Value, String> {
        match cmd {
            "hello" => {
                self.last_sent.clear();
                Ok(json!({ "scene": [SCENE_WIDTH, SCENE_HEIGHT], "fps": FPS }))
            }
            "log" => {
                eprintln!("ui: {}", a.s("text").unwrap_or(""));
                Ok(Value::Null)
            }
            "tick" => {
                self.tick(a.f("dt").unwrap_or(0.0));
                Ok(Value::Null)
            }
            // ---- файлы
            "new" | "demo" => {
                self.character = if cmd == "demo" { build_walking_pony() } else { empty_character() };
                self.asset_path.clear();
                anim::pick_default_animation(&self.character, &mut self.player);
                self.player.apply(&mut self.character);
                self.playing = false;
                self.vector_doc.clear();
                self.editing_part = None;
                self.edit_vb = None;
                self.selected_shape = None;
                self.clear_selection();
                self.hidden.clear();
                self.locked.clear();
                self.fit("character");
                self.status = if cmd == "demo" { "Демо-пони (Ctrl+Z — вернуть предыдущего)".into() } else { "Новый пустой персонаж (Ctrl+Z — вернуть предыдущего)".into() };
                Ok(Value::Null)
            }
            "open" => self.open_path(a.s("path").unwrap_or("")),
            "save" => self.save(a.s("path")),
            "import_layer" => self.import_layer(a.s("path").unwrap_or("")),
            "export" => self.export(a.s("kind").unwrap_or("gif"), a.s("path").unwrap_or("")),
            "exit" => {
                self.exit_requested = true;
                Ok(Value::Null)
            }
            "fs_list" => {
                let dir = match a.s("dir") {
                    Some(d) if !d.is_empty() => std::path::PathBuf::from(d),
                    _ => crate::files::start_dir(a.s("start")),
                };
                Ok(crate::files::list(&dir))
            }
            "fs_stat" => Ok(crate::files::stat(a.s("path").unwrap_or(""))),
            "fs_join" => Ok(json!(crate::files::join(a.s("dir").unwrap_or(""), a.s("name").unwrap_or("")))),
            "fs_mkdir" => crate::files::mkdir(a.s("dir").unwrap_or(""), a.s("name").unwrap_or("")).map(Value::from),
            "fs_remember" => {
                crate::files::remember(std::path::Path::new(a.s("dir").unwrap_or("")));
                Ok(Value::Null)
            }
            // ---- Adobe Animate
            "xfl_load" => {
                self.xfl_load(a.s("path").unwrap_or(""));
                Ok(Value::Null)
            }
            "xfl_select" => {
                self.xfl.symbol = a.s("symbol").map(str::to_string);
                self.xfl.frame = 0;
                Ok(Value::Null)
            }
            "xfl_frame" => {
                let count = self.xfl.doc.as_ref().and_then(|d| d.timeline(self.xfl.symbol.as_deref())).map(|t| t.frame_count()).unwrap_or(1);
                self.xfl.frame = (a.i("frame").unwrap_or(0).max(0) as usize).min(count.saturating_sub(1));
                Ok(Value::Null)
            }
            "xfl_opts" => {
                if let Some(s) = a.f("scale") {
                    self.xfl.opts.scale = s.clamp(0.01, 10.0);
                }
                if let Some(e) = a.get("expand").and_then(Value::as_bool) {
                    self.xfl.opts.expand_containers = e;
                }
                if let (Some(h), Some(doc)) = (a.get("hidden").and_then(Value::as_bool), self.xfl.doc.as_mut()) {
                    doc.include_hidden_layers = h;
                }
                Ok(Value::Null)
            }
            "xfl_import" => self.xfl_import(),
            // ---- правка
            "undo" | "redo" => {
                self.undo_redo(cmd == "undo");
                Ok(Value::Null)
            }
            "delete_layer" => {
                if let Some(id) = self.selected_layer.take() {
                    self.character.parts.remove(&id);
                    self.hidden.remove(&id);
                    self.locked.remove(&id);
                    self.status = format!("Слой '{id}' удалён");
                }
                Ok(Value::Null)
            }
            "duplicate_layer" => {
                if let Some(id) = self.selected_layer.clone() {
                    if let Some(src) = self.character.parts.get(&id).cloned() {
                        let new_id = anim::unique_name(&format!("{id}_copy"), |c| self.character.parts.contains_key(c));
                        let mut copy = src.clone();
                        copy.id = new_id.clone();
                        copy.layer = src.layer + 1;
                        self.character.parts.insert(new_id.clone(), copy);
                        self.selected_layer = Some(new_id.clone());
                        self.status = format!("Создана копия '{new_id}'");
                    }
                }
                Ok(Value::Null)
            }
            "deselect" => {
                self.selected_layer = None;
                self.selected_shape = None;
                self.selected_keyframe = None;
                Ok(Value::Null)
            }
            "escape" => {
                if self.placing_bone {
                    self.placing_bone = false;
                } else if self.tool == Tool::FreeTransform && self.selected_bone.is_some() {
                    self.selected_bone = None;
                    self.status = "Xform: выбор кости снят".into();
                } else if !self.multi_selected.is_empty() {
                    self.multi_selected.clear();
                } else {
                    self.selected_layer = None;
                    self.selected_shape = None;
                    self.selected_keyframe = None;
                }
                Ok(Value::Null)
            }
            "multi" => {
                let op = a.s("op").unwrap_or("");
                match op {
                    "hide" => self.hidden.extend(self.multi_selected.iter().cloned()),
                    "show" => {
                        for id in &self.multi_selected {
                            self.hidden.remove(id);
                        }
                    }
                    "delete" => {
                        for id in self.multi_selected.drain() {
                            self.character.parts.remove(&id);
                            self.hidden.remove(&id);
                            self.locked.remove(&id);
                        }
                        self.status = "Группа удалена".into();
                    }
                    "clear" => self.multi_selected.clear(),
                    "group" => {
                        let ids: Vec<String> = self.multi_selected.iter().cloned().collect();
                        let n = self.character.groups.groups.len() + 1;
                        let group_id = self.character.group_parts(format!("Group {n}"), &ids);
                        self.status = format!("Группа '{group_id}' создана из {} частей", ids.len());
                        self.multi_selected.clear();
                    }
                    _ => return Err(format!("неизвестная операция '{op}'")),
                }
                Ok(Value::Null)
            }
            // ---- вставка / изменение
            "insert_keyframe" => {
                self.insert_keyframe_here();
                Ok(Value::Null)
            }
            "blank_layer" => {
                self.blank_layer_count += 1;
                let id = anim::unique_name(&format!("layer_{}", self.blank_layer_count), |c| self.character.parts.contains_key(c));
                let max_layer = self.character.parts.values().map(|p| p.layer).max().unwrap_or(0);
                let bone = default_bone(&self.character);
                self.character.add_part(Part::new(&id, PartKind::Custom, PartSource::Png { path: String::new() }).with_bone(bone).with_layer(max_layer + 1));
                self.selected_layer = Some(id.clone());
                self.status = format!("Добавлен пустой слой '{id}'");
                Ok(Value::Null)
            }
            "modify" => {
                let op = a.s("op").unwrap_or("");
                let bone_id = self.selected_part_bone();
                match op {
                    "flip_h" | "flip_v" | "reset" => {
                        if let Some(b) = bone_id.and_then(|id| self.character.skeleton.bones.iter_mut().find(|b| b.id == id)) {
                            match op {
                                "flip_h" => b.local_transform.scale.x = -b.local_transform.scale.x,
                                "flip_v" => b.local_transform.scale.y = -b.local_transform.scale.y,
                                _ => b.local_transform = pony_core::skeleton::Transform2D::default(),
                            }
                        }
                    }
                    "up" | "down" => {
                        if let Some(p) = self.selected_layer.clone().and_then(|id| self.character.parts.get_mut(&id)) {
                            p.layer += if op == "up" { 1 } else { -1 };
                        }
                    }
                    _ => return Err(format!("неизвестная операция '{op}'")),
                }
                Ok(Value::Null)
            }
            "script_run" => {
                match self.script_engine.run(a.s("text").unwrap_or("")) {
                    Ok(commands) => {
                        let n = commands.len();
                        pony_script::apply_commands(&mut self.character, &mut self.camera, &mut self.player, &commands);
                        self.take_blink();
                        self.script_log = format!("OK: выполнено {n} команд(ы)");
                    }
                    Err(err) => self.script_log = format!("Ошибка: {err}"),
                }
                self.status = self.script_log.clone();
                Ok(Value::Null)
            }
            // ---- воспроизведение
            "toggle_play" => {
                if !self.playing && self.player.is_finished(&self.character) && self.player.is_valid(&self.character) {
                    self.player.seek(&self.character, 0.0);
                }
                self.playing = !self.playing && self.player.is_valid(&self.character);
                if !self.player.is_valid(&self.character) {
                    self.status = "Нет анимации — создай её кнопкой «+» на таймлайне".into();
                }
                Ok(Value::Null)
            }
            "seek" => {
                let op = a.s("op").unwrap_or("");
                self.playing = false;
                let duration = self.current_anim().map(|a| a.duration).unwrap_or(0.0);
                let frame = (self.player.time() * FPS).round();
                let t = match op {
                    "start" => 0.0,
                    "end" => duration,
                    "prev" => (frame - 1.0) / FPS,
                    "next" => (frame + 1.0) / FPS,
                    _ => a.i("frame").unwrap_or(0).max(0) as f32 / FPS,
                };
                self.player.seek(&self.character, t.min(duration.max(0.0)));
                self.player.apply(&mut self.character);
                Ok(Value::Null)
            }
            "play_anim" => {
                let name = a.s("name").unwrap_or("").to_string();
                if self.character.animations.contains_key(&name) {
                    self.player.play(&name);
                    self.player.apply(&mut self.character);
                    self.selected_keyframe = None;
                    if a.b("start") {
                        self.playing = true;
                        self.status = format!("Играет '{name}'");
                    }
                }
                Ok(Value::Null)
            }
            "anim_new" => {
                let name = anim::unique_animation_name(&self.character);
                self.character.add_animation(Animation { name: name.clone(), duration: 1.0, looping: true, tracks: Vec::new() });
                self.player.play(&name);
                self.playing = false;
                self.selected_keyframe = None;
                Ok(Value::Null)
            }
            "anim_set" => {
                let name = self.player.current_name().map(str::to_string);
                if let Some(anim) = name.and_then(|n| self.character.animations.get_mut(&n)) {
                    if let Some(d) = a.f("duration") {
                        anim.duration = d.clamp(1.0 / FPS, 600.0);
                    }
                    if let Some(l) = a.get("looping").and_then(Value::as_bool) {
                        anim.looping = l;
                    }
                }
                Ok(Value::Null)
            }
            "channel" => {
                if let Some(ch) = a.s("ch").and_then(anim::channel_from_key) {
                    self.timeline_channel = ch;
                    self.selected_keyframe = None;
                }
                Ok(Value::Null)
            }
            "tl_drag_start" => {
                self.selected_keyframe = match (a.s("label"), a.i("frame")) {
                    (Some(l), Some(f)) => Some((l.to_string(), f)),
                    _ => None,
                };
                Ok(Value::Null)
            }
            "tl_drag" => {
                self.tl_drag(a.i("frame").unwrap_or(0).max(0));
                Ok(Value::Null)
            }
            "tl_click" => {
                let frame = a.i("frame").unwrap_or(0).max(0);
                let label = a.s("label").map(str::to_string);
                if let Some(l) = &label {
                    if self.character.parts.contains_key(l) {
                        self.selected_layer = Some(l.clone());
                    }
                }
                self.selected_keyframe = if a.b("key") { label.map(|l| (l, frame)) } else { None };
                self.seek_frame(frame);
                Ok(Value::Null)
            }
            "tl_dblclick" => {
                self.tl_add_key(a.s("label").unwrap_or(""), a.i("frame").unwrap_or(0).max(0));
                Ok(Value::Null)
            }
            "tl_delete_key" => {
                self.tl_delete_key();
                Ok(Value::Null)
            }
            "toggle_hidden" | "toggle_lock" => {
                let id = a.s("id").unwrap_or("").to_string();
                let set = if cmd == "toggle_hidden" { &mut self.hidden } else { &mut self.locked };
                if !set.remove(&id) {
                    set.insert(id);
                }
                Ok(Value::Null)
            }
            "select_layer" => {
                self.selected_layer = a.s("id").filter(|id| self.character.parts.contains_key(*id)).map(str::to_string);
                Ok(Value::Null)
            }
            "timeline_mode" => {
                self.timeline_all = a.b("all");
                Ok(Value::Null)
            }
            "char_new" => {
                self.new_character_count += 1;
                let new_name = format!("Pony {}", self.new_character_count);
                self.push_active_and_replace(empty_character(), new_name);
                self.fit("character");
                self.status = format!("Создан персонаж '{}'", self.active_name);
                Ok(Value::Null)
            }
            "char_delete" => {
                if let Some(next) = self.others.pop() {
                    self.character = next.character;
                    self.player = next.player;
                    let deleted = std::mem::replace(&mut self.active_name, next.name);
                    self.selected_layer = None;
                    self.selected_bone = None;
                    self.history.reset(&self.character, &self.vector_doc);
                    self.status = format!("Персонаж '{deleted}' удалён, активен '{}'", self.active_name);
                }
                Ok(Value::Null)
            }
            "char_switch" => {
                let i = a.i("index").unwrap_or(-1);
                if i >= 0 && (i as usize) < self.others.len() {
                    let picked = self.others.remove(i as usize);
                    self.others.push(Slot {
                        character: std::mem::replace(&mut self.character, picked.character),
                        player: std::mem::replace(&mut self.player, picked.player),
                        name: std::mem::replace(&mut self.active_name, picked.name),
                    });
                    self.selected_layer = None;
                    self.selected_bone = None;
                    self.timeline_all = false;
                    self.history.reset(&self.character, &self.vector_doc);
                    self.status = format!("Активен '{}'", self.active_name);
                }
                Ok(Value::Null)
            }
            // ---- инструменты и холст
            "set_tool" => {
                if let Some(t) = a.s("tool").and_then(Tool::from_key) {
                    self.tool = t;
                }
                Ok(Value::Null)
            }
            "set_colors" => {
                if let Some(c) = a.color("fill") {
                    self.fill = c;
                }
                if let Some(c) = a.color("stroke") {
                    self.stroke = c;
                }
                Ok(Value::Null)
            }
            "poly_sides" => {
                self.poly_sides = a.i("n").unwrap_or(6).clamp(3, 12) as u32;
                Ok(Value::Null)
            }
            "show_skeleton" => {
                self.show_skeleton = a.b("on");
                Ok(Value::Null)
            }
            "placing_bone" => {
                self.placing_bone = a.b("on");
                Ok(Value::Null)
            }
            "stage_click" => {
                self.stage_click(a);
                Ok(Value::Null)
            }
            "stage_drag_start" => {
                self.stage_drag_start(a);
                Ok(Value::Null)
            }
            "stage_drag" => {
                self.stage_drag(a);
                Ok(Value::Null)
            }
            "stage_drag_end" => {
                self.stage_drag_end(a);
                Ok(Value::Null)
            }
            "add_polyline" => {
                let points = self.doc_points(a);
                if points.len() >= 2 {
                    self.vector_doc.add(VectorShape::Polyline { points, stroke: self.stroke, stroke_width: if a.b("brush") { 4.0 } else { 1.5 } });
                }
                Ok(Value::Null)
            }
            "pen_commit" => {
                let points = self.doc_points(a);
                if a.b("closed") && points.len() >= 3 {
                    self.vector_doc.add(VectorShape::Polygon { points, fill: self.fill, stroke: self.stroke, stroke_width: 1.5, fill_gradient: None });
                    self.status = "Путь Pen замкнут в многоугольник".into();
                } else if points.len() >= 2 {
                    self.vector_doc.add(VectorShape::Polyline { points, stroke: self.stroke, stroke_width: 1.5 });
                    self.status = "Открытый путь Pen добавлен".into();
                }
                Ok(Value::Null)
            }
            "delete_shape" => {
                if let Some(idx) = self.selected_shape.take() {
                    if idx < self.vector_doc.shapes.len() {
                        self.vector_doc.shapes.remove(idx);
                        self.status = "Фигура удалена".into();
                    }
                }
                Ok(Value::Null)
            }
            "select_shape" => {
                self.selected_shape = a.i("index").filter(|i| *i >= 0 && (*i as usize) < self.vector_doc.shapes.len()).map(|i| i as usize);
                Ok(Value::Null)
            }
            "save_drawing" => {
                self.save_drawing();
                Ok(Value::Null)
            }
            "cancel_edit" => {
                self.editing_part = None;
                self.edit_vb = None;
                self.vector_doc.clear();
                self.selected_shape = None;
                self.status = "Редактирование отменено".into();
                Ok(Value::Null)
            }
            "clear_canvas" => {
                self.vector_doc.clear();
                self.selected_shape = None;
                Ok(Value::Null)
            }
            "edit_svg" => {
                self.start_editing_svg_part(a.s("id").unwrap_or(""));
                Ok(Value::Null)
            }
            // ---- свойства
            "part_set" => {
                self.part_set(a);
                Ok(Value::Null)
            }
            "facing_yaw" => {
                self.character.facing_yaw = a.f("deg").unwrap_or(0.0).clamp(-89.0, 89.0).to_radians();
                Ok(Value::Null)
            }
            "select_bone" => {
                self.selected_bone = a.s("id").filter(|id| self.character.skeleton.find(id).is_some()).map(str::to_string);
                Ok(Value::Null)
            }
            "bone_set" => {
                let id = a.s("id").unwrap_or("");
                if let Some(bone) = self.character.skeleton.bones.iter_mut().find(|b| b.id == id) {
                    let t = &mut bone.local_transform;
                    if let Some(v) = a.f("x") {
                        t.position.x = v;
                    }
                    if let Some(v) = a.f("y") {
                        t.position.y = v;
                    }
                    if let Some(v) = a.f("rot") {
                        t.rotation = v.to_radians();
                    }
                    if let Some(v) = a.f("sx") {
                        t.scale.x = v;
                    }
                    if let Some(v) = a.f("sy") {
                        t.scale.y = v;
                    }
                    if let Some(v) = a.f("length") {
                        bone.length = v.clamp(0.0, 200.0);
                    }
                }
                Ok(Value::Null)
            }
            "bone_delete" => {
                self.bone_delete(a.s("id").unwrap_or(""));
                Ok(Value::Null)
            }
            "bone_rename" => self.bone_rename(a.s("id").unwrap_or(""), a.s("name").unwrap_or("")),
            "bone_reparent" => self.bone_reparent(a.s("id").unwrap_or(""), a.s("parent").unwrap_or("")),
            "ik_set" => {
                let id = a.s("id").unwrap_or("");
                if let Some(c) = self.character.skeleton.find_ik_constraint_mut(id) {
                    if let Some(v) = a.get("enabled").and_then(Value::as_bool) {
                        c.enabled = v;
                    }
                    if let Some(v) = a.f("tx") {
                        c.target.x = v;
                    }
                    if let Some(v) = a.f("ty") {
                        c.target.y = v;
                    }
                    if let Some(v) = a.f("weight") {
                        c.weight = v.clamp(0.0, 1.0);
                    }
                    match a.get("pole") {
                        Some(Value::Null) => c.pole_target = None,
                        Some(Value::Array(p)) if p.len() == 2 => {
                            c.pole_target = Some(Vec2::new(p[0].as_f64().unwrap_or(0.0) as f32, p[1].as_f64().unwrap_or(0.0) as f32));
                        }
                        Some(Value::Bool(true)) => c.pole_target = Some(c.target + Vec2::new(0.0, 20.0)),
                        _ => {}
                    }
                }
                Ok(Value::Null)
            }
            "ik_remove" => {
                let id = a.s("id").unwrap_or("");
                self.character.skeleton.remove_ik_constraint(id);
                self.status = format!("IK-констрейнт '{id}' удалён");
                Ok(Value::Null)
            }
            "ik_create" => self.ik_create(a.s("root").unwrap_or(""), a.s("mid").unwrap_or(""), a.s("tip").unwrap_or("")),
            "env_set" => {
                self.env_set(a);
                Ok(Value::Null)
            }
            // ---- символы, группы, градиенты, булевы операции
            "symbol_convert" => self.symbol_convert(a.s("name").unwrap_or("")),
            "symbol_place" => self.symbol_place(a.s("name").unwrap_or("")),
            "symbol_break" => {
                if let Some(idx) = self.selected_shape {
                    if self.vector_doc.break_apart_symbol_instance(idx) {
                        self.status = "Инстанс разорван — теперь независимая копия геометрии".into();
                    }
                }
                Ok(Value::Null)
            }
            "group_hidden" => {
                if let Some(g) = self.character.groups.find_mut(a.s("id").unwrap_or("")) {
                    g.hidden = a.b("hidden");
                }
                Ok(Value::Null)
            }
            "ungroup" => {
                self.character.ungroup(a.s("id").unwrap_or(""));
                self.status = "Группа разгруппирована — части остались на месте".into();
                Ok(Value::Null)
            }
            "gradient_save" => self.gradient_save(a),
            "gradient_delete" => {
                let name = a.s("name").unwrap_or("").to_string();
                self.vector_doc.remove_gradient(&name);
                self.status = format!("Градиент '{name}' удалён (фигуры, ссылавшиеся на него, откатились на плоский fill)");
                Ok(Value::Null)
            }
            "gradient_apply" => {
                let name = a.s("name").unwrap_or("").to_string();
                if let (Some(idx), Some(def)) = (self.selected_shape, self.vector_doc.find_gradient(&name).cloned()) {
                    if let Some(shape) = self.vector_doc.shapes.get_mut(idx) {
                        shape.set_fill_gradient(Some(name.clone()), def.average_color());
                        self.status = format!("Градиент '{name}' применён к выбранной фигуре");
                    }
                }
                Ok(Value::Null)
            }
            "boolean_run" => self.boolean_run(a),
            _ => Err(format!("неизвестная команда '{cmd}'")),
        }
    }

    // -------------------------------------------------------- время

    fn tick(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        self.elapsed += dt;
        if self.playing {
            self.player.advance(&self.character, dt);
            self.player.apply(&mut self.character);
            if self.player.is_finished(&self.character) {
                self.playing = false;
            }
        }
        self.camera.shake_intensity = (self.camera.shake_intensity - dt * 0.6).max(0.0);
        if self.blink_timer > 0.0 {
            self.blink_timer = (self.blink_timer - dt).max(0.0);
        }
        if self.particles_enabled {
            self.emitter.update(dt);
        }
    }

    fn seek_frame(&mut self, frame: i64) {
        self.playing = false;
        let duration = self.current_anim().map(|a| a.duration).unwrap_or(1.0).max(0.01);
        self.player.seek(&self.character, (frame.max(0) as f32 / FPS).min(duration));
        self.player.apply(&mut self.character);
    }

    // -------------------------------------------------------- файлы

    fn open_path(&mut self, path: &str) -> Result<Value, String> {
        let p = std::path::Path::new(path);
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext == "fla" || ext == "xfl" || p.is_dir() {
            self.xfl_load(path);
            return Ok(Value::Null);
        }
        let loaded = Character::load_from_file(path).map_err(|err| format!("Ошибка загрузки {path}: {err}"))?;
        self.asset_path = path.to_string();
        self.character = loaded;
        anim::pick_default_animation(&self.character, &mut self.player);
        self.player.apply(&mut self.character);
        self.playing = false;
        self.clear_selection();
        self.hidden.clear();
        self.locked.clear();
        self.history.reset(&self.character, &self.vector_doc);
        self.saved_revision = self.history.revision;
        self.fit("character");
        self.status = format!("Загружено из {path}");
        Ok(Value::Null)
    }

    fn save(&mut self, path: Option<&str>) -> Result<Value, String> {
        if let Some(p) = path.filter(|p| !p.is_empty()) {
            self.asset_path = p.to_string();
        }
        if self.asset_path.is_empty() {
            return Ok(json!({ "need_path": true, "name": format!("{}.asset", file_stem_for(&self.active_name)) }));
        }
        match self.character.save_to_file(&self.asset_path) {
            Ok(()) => {
                self.history.commit_if_changed(&self.character, &self.vector_doc, &self.player);
                self.saved_revision = self.history.revision;
                self.status = format!("Сохранено в {}", self.asset_path);
                Ok(json!({ "saved": true }))
            }
            Err(err) => Err(format!("Ошибка сохранения {}: {err}", self.asset_path)),
        }
    }

    /// Имена файлов по умолчанию для проводника.
    fn default_names(&self) -> Value {
        let anim_name = self.player.current_name().unwrap_or("animation");
        let stem = file_stem_for(&self.active_name);
        json!({
            "asset": format!("{stem}.asset"),
            "gif": format!("{stem}_{}.gif", file_stem_for(anim_name)),
            "png": format!("{stem}_{}.png", file_stem_for(anim_name)),
        })
    }

    fn import_layer(&mut self, path: &str) -> Result<Value, String> {
        let p = std::path::Path::new(path);
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let source = match ext.as_str() {
            "png" => PartSource::Png { path: path.into() },
            "svg" => PartSource::Vector { path: path.into() },
            "psd" => PartSource::Psd { path: path.into(), layer: None },
            "kra" => PartSource::Kra { path: path.into(), layer_file: None },
            _ => return Err(format!("Неподдерживаемый формат: {path} (нужны .png, .svg, .psd, .kra)")),
        };
        self.import_count += 1;
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| format!("imported_{}", self.import_count));
        let part_id = anim::unique_name(&stem, |c| self.character.parts.contains_key(c));
        let max_layer = self.character.parts.values().map(|p| p.layer).max().unwrap_or(0);
        let bone = default_bone(&self.character);
        self.character.add_part(Part::new(&part_id, PartKind::Custom, source).with_bone(bone).with_layer(max_layer + 1));
        self.selected_layer = Some(part_id.clone());
        self.fit("character");
        self.status = format!("Импортировано '{path}' как слой '{part_id}'");
        Ok(Value::Null)
    }

    /// GIF / спрайт-лист текущей анимации. Рендер — CPU (resvg), тем же
    /// списком отрисовки, что и Stage; скрытые слои не попадают в файл.
    fn export(&mut self, kind: &str, path: &str) -> Result<Value, String> {
        if self.exporting {
            return Err("Экспорт уже идёт — дождись его окончания".into());
        }
        let what = if kind == "gif" { "GIF" } else { "спрайт-листа" };
        let name = self.player.current_name().ok_or_else(|| format!("Экспорт {what} не выполнен: нет активной анимации — создай её на таймлайне"))?.to_string();
        if !self.character.animations.contains_key(&name) {
            return Err(format!("Экспорт {what} не выполнен: анимация '{name}' не найдена"));
        }
        let mut temp = self.character.clone();
        temp.parts.retain(|id, _| !self.hidden.contains(id) && !self.character.is_part_hidden_by_group(id));
        let (lighting, camera, kind, path) = (self.lighting.clone(), self.camera, kind.to_string(), path.to_string());
        self.jobs.push(Box::new(move || render_export(temp, &name, &lighting, &camera, &kind, &path)));
        self.exporting = true;
        self.status = format!("Экспорт {what}… (кадры рендерятся в фоне)");
        Ok(json!({ "started": true }))
    }

    /// Забрать задачи для фоновых потоков.
    pub fn take_jobs(&mut self) -> Vec<Job> {
        std::mem::take(&mut self.jobs)
    }

    /// Фоновая задача закончилась: показать результат. Возвращает сообщение
    /// для интерфейса (без id — это не ответ на команду).
    pub fn job_done(&mut self, result: Result<String, String>) -> Value {
        self.exporting = false;
        self.status = match result {
            Ok(s) | Err(s) => s,
        };
        json!({ "id": null, "patch": self.patch() })
    }

    fn xfl_load(&mut self, path: &str) {
        let path = path.trim().to_string();
        self.xfl.path = path.clone();
        self.xfl.show += 1;
        let t = std::time::Instant::now();
        match pony_core::xfl::XflDocument::open(&path) {
            Ok(doc) => {
                self.xfl.info = format!(
                    "{}: {} символов, сцена {}×{}, {} fps — загружено за {:.1} с{}",
                    doc.name,
                    doc.symbols.len(),
                    doc.width,
                    doc.height,
                    doc.frame_rate,
                    t.elapsed().as_secs_f32(),
                    if doc.warnings.is_empty() { String::new() } else { format!(" (пропущено битых символов: {})", doc.warnings.len()) }
                );
                self.xfl.symbol = default_xfl_symbol(&doc);
                self.xfl.frame = 0;
                self.status = format!("Adobe Animate: открыт {path}");
                self.xfl.doc = Some(doc);
            }
            Err(err) => {
                self.xfl.info = format!("Не удалось открыть {path}: {err}");
                self.status = self.xfl.info.clone();
            }
        }
    }

    fn xfl_import(&mut self) -> Result<Value, String> {
        let doc = self.xfl.doc.as_ref().ok_or("Сначала открой файл Adobe Animate")?;
        let out_dir = xfl_import_dir(&doc.name, self.xfl.symbol.as_deref(), self.xfl.frame);
        let (imported, report) = pony_core::xfl::import_to_character(doc, self.xfl.symbol.as_deref(), self.xfl.frame, &self.xfl.opts, &out_dir).map_err(|err| format!("Импорт не удался: {err}"))?;
        self.new_character_count += 1;
        let name = format!("{} {}", imported.name, self.new_character_count);
        self.push_active_and_replace(imported, name);
        self.playing = false;
        self.hidden.clear();
        self.locked.clear();
        self.show_skeleton = false;
        self.fit("character");
        self.status = format!(
            "Импортировано: {} частей -> {}{}",
            report.parts,
            report.out_dir.display(),
            if report.unsupported > 0 { format!(" (пропущено текста/картинок: {})", report.unsupported) } else { String::new() }
        );
        Ok(json!({ "close": true }))
    }

    /// Текущий активный персонаж уходит в список остальных, его место
    /// занимает `character`.
    fn push_active_and_replace(&mut self, character: Character, name: String) {
        self.others.push(Slot {
            character: std::mem::replace(&mut self.character, character),
            player: std::mem::replace(&mut self.player, AnimationPlayer::new()),
            name: std::mem::replace(&mut self.active_name, name),
        });
        self.clear_selection();
        self.history.reset(&self.character, &self.vector_doc);
    }

    fn undo_redo(&mut self, undo: bool) {
        let restored = if undo { self.history.undo(&self.character, &self.vector_doc) } else { self.history.redo(&self.character, &self.vector_doc) };
        match restored {
            Some(state) => {
                self.character = state.character;
                self.vector_doc = state.vector_doc;
                if self.selected_layer.as_ref().is_some_and(|id| !self.character.parts.contains_key(id)) {
                    self.selected_layer = None;
                }
                if self.selected_bone.as_ref().is_some_and(|id| self.character.skeleton.find(id).is_none()) {
                    self.selected_bone = None;
                }
                if self.selected_shape.is_some_and(|i| i >= self.vector_doc.shapes.len()) {
                    self.selected_shape = None;
                }
                if !self.player.is_valid(&self.character) {
                    anim::pick_default_animation(&self.character, &mut self.player);
                }
                self.selected_keyframe = None;
                self.status = if undo { "Отменено".into() } else { "Возвращено".into() };
            }
            None => self.status = if undo { "Нечего отменять".into() } else { "Нечего возвращать".into() },
        }
    }

    // -------------------------------------------------------- таймлайн

    fn insert_keyframe_here(&mut self) {
        let bone = if self.tool == Tool::FreeTransform && self.selected_bone.as_ref().is_some_and(|b| self.character.skeleton.find(b).is_some()) {
            self.selected_bone.clone()
        } else {
            self.selected_part_bone()
        };
        match (self.selected_layer.clone(), bone) {
            (Some(layer_id), Some(bone_id)) => {
                let channel = self.timeline_channel;
                let value = self.character.skeleton.find(&bone_id).map(|b| anim::bone_channel_value(b, channel)).unwrap_or(0.0);
                let anim_name = anim::ensure_animation(&mut self.character, &mut self.player);
                let frame = (self.player.time() * FPS).round();
                if let Some(a) = self.character.animations.get_mut(&anim_name) {
                    anim::insert_keyframe(a, &bone_id, channel, frame / FPS, value);
                    self.selected_keyframe = Some((layer_id, frame as i64));
                    self.status = format!("Ключ на кадре {frame} для кости '{bone_id}' ({})", anim::channel_label(channel));
                }
            }
            (Some(_), None) => self.status = "У выбранного слоя нет кости — ключ ставить не к чему".into(),
            (None, _) => self.status = "Сначала выбери слой (на Stage или в Timeline)".into(),
        }
    }

    fn tl_drag(&mut self, new_frame: i64) {
        let Some((label, orig_frame)) = self.selected_keyframe.clone() else {
            self.seek_frame(new_frame);
            return;
        };
        if new_frame == orig_frame {
            return;
        }
        let channel = self.timeline_channel;
        let (Some(bone_id), Some(name)) = (anim::row_bone(&self.character, &label), self.player.current_name().map(str::to_string)) else { return };
        if let Some(a) = self.character.animations.get_mut(&name) {
            let new_t = new_frame as f32 / FPS;
            if !anim::has_keyframe_at(a, &bone_id, channel, new_t) {
                if let Some(value) = anim::remove_keyframe(a, &bone_id, channel, orig_frame as f32 / FPS) {
                    anim::insert_keyframe(a, &bone_id, channel, new_t, value);
                    self.selected_keyframe = Some((label, new_frame));
                }
            }
        }
    }

    fn tl_add_key(&mut self, label: &str, frame: i64) {
        if self.character.parts.contains_key(label) {
            self.selected_layer = Some(label.to_string());
        }
        let Some(bone_id) = anim::row_bone(&self.character, label) else { return };
        let channel = self.timeline_channel;
        let value = self.character.skeleton.find(&bone_id).map(|b| anim::bone_channel_value(b, channel)).unwrap_or(0.0);
        let name = anim::ensure_animation(&mut self.character, &mut self.player);
        if let Some(a) = self.character.animations.get_mut(&name) {
            anim::insert_keyframe(a, &bone_id, channel, frame as f32 / FPS, value);
            self.selected_keyframe = Some((label.to_string(), frame));
        }
        self.seek_frame(frame);
    }

    fn tl_delete_key(&mut self) {
        let Some((label, frame)) = self.selected_keyframe.clone() else { return };
        let channel = self.timeline_channel;
        if let (Some(bone_id), Some(name)) = (anim::row_bone(&self.character, &label), self.player.current_name().map(str::to_string)) {
            if let Some(a) = self.character.animations.get_mut(&name) {
                if anim::remove_keyframe(a, &bone_id, channel, frame as f32 / FPS).is_some() {
                    self.selected_keyframe = None;
                }
            }
        }
    }

    // -------------------------------------------------------- Stage

    fn stage_click(&mut self, a: &Value) {
        let w = a.pt("x", "y");
        let ppw = a.f("ppw").unwrap_or(1.0);
        if self.placing_bone {
            self.place_bone(w);
            self.placing_bone = false;
            return;
        }
        match self.tool {
            Tool::Selection => self.selected_layer = part_at(&self.character, &self.hidden, &self.locked, w),
            Tool::FreeTransform => self.xform_pick(w, a.b("alt"), ppw),
            Tool::Eyedropper => self.eyedrop(w),
            Tool::PaintBucket | Tool::InkBottle => {
                let (dx, dy) = self.to_doc(w);
                if let Some(idx) = self.vector_doc.shape_at(dx, dy) {
                    if let Some(shape) = self.vector_doc.shapes.get_mut(idx) {
                        if self.tool == Tool::PaintBucket {
                            shape.set_fill(self.fill);
                        } else {
                            shape.set_stroke(self.stroke);
                        }
                    }
                }
            }
            Tool::SubSelection => {
                if self.dragging_point.is_none() {
                    let (dx, dy) = self.to_doc(w);
                    self.selected_shape = self.vector_doc.shape_at(dx, dy);
                }
            }
            _ => {}
        }
    }

    fn stage_drag_start(&mut self, a: &Value) {
        let w = a.pt("x", "y");
        let ppw = a.f("ppw").unwrap_or(1.0);
        match self.tool {
            Tool::Selection => {
                self.grab_offset = None;
                if let Some(hit) = part_at(&self.character, &self.hidden, &self.locked, w) {
                    self.selected_layer = Some(hit);
                }
                if let Some(part) = self.selected_layer.as_ref().filter(|id| !self.locked.contains(*id)).and_then(|id| self.character.parts.get(id)) {
                    if let Some(world) = part.bone.as_ref().and_then(|b| self.character.skeleton.world_transform_with_ik(b)) {
                        self.grab_offset = Some(pony_render::part_world_position(part, &world) - w);
                    }
                }
            }
            Tool::FreeTransform => self.xform_pick(w, a.b("alt"), ppw),
            Tool::SubSelection => {
                self.dragging_point = None;
                if let Some(shape) = self.selected_shape.and_then(|i| self.vector_doc.shapes.get(i)) {
                    let map = self.doc_map();
                    self.dragging_point = shape.control_points().iter().position(|(px, py)| {
                        let (x, y) = map.apply(*px, *py);
                        Vec2::new(x - w.x, y - w.y).length() < 8.0 * ppw
                    });
                }
            }
            _ => {}
        }
    }

    fn stage_drag(&mut self, a: &Value) {
        let w = a.pt("x", "y");
        let prev = a.pt("px", "py");
        match self.tool {
            Tool::Selection => {
                if let (Some(id), Some(offset)) = (self.selected_layer.clone(), self.grab_offset) {
                    let target = w + offset;
                    if let Some(bone_id) = self.character.parts.get(&id).and_then(|p| p.bone.clone()) {
                        let bone_world = self.character.skeleton.world_transform_with_ik(&bone_id).unwrap_or_default();
                        let new_pivot = pony_render::pivot_for_world_position(target, &bone_world);
                        if let Some(p) = self.character.parts.get_mut(&id) {
                            p.pivot = new_pivot;
                        }
                    }
                }
            }
            Tool::FreeTransform => {
                let layer_locked = self.selected_layer.as_ref().is_some_and(|id| self.locked.contains(id));
                if layer_locked {
                    return;
                }
                let Some(bone_id) = self.selected_bone.clone().filter(|b| self.character.skeleton.find(b).is_some()) else { return };
                if a.b("shift") {
                    // Поворот вокруг точки кости на угол между прошлой и
                    // текущей позицией курсора.
                    if let Some(center) = self.character.skeleton.world_transform_with_ik(&bone_id).map(|t| t.position) {
                        let (p, c) = (prev - center, w - center);
                        if p.length() > 1e-3 && c.length() > 1e-3 {
                            let mut da = c.y.atan2(c.x) - p.y.atan2(p.x);
                            if da > std::f32::consts::PI {
                                da -= std::f32::consts::TAU;
                            } else if da < -std::f32::consts::PI {
                                da += std::f32::consts::TAU;
                            }
                            if let Some(bone) = self.character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                                bone.local_transform.rotation += da;
                            }
                        }
                    }
                } else {
                    // Локальная позиция кости — в пространстве родителя.
                    let delta = w - prev;
                    let sk = &self.character.skeleton;
                    let parent_world = sk.find(&bone_id).and_then(|b| b.parent.clone()).and_then(|p| sk.world_transform_with_ik(&p));
                    let bone_world = sk.world_transform_with_ik(&bone_id);
                    if let Some(bone) = self.character.skeleton.bones.iter_mut().find(|b| b.id == bone_id) {
                        match (parent_world, bone_world) {
                            (Some(pw), Some(bw)) => bone.local_transform.position = pony_render::pivot_for_world_position(bw.position + delta, &pw),
                            _ => bone.local_transform.position += delta,
                        }
                    }
                }
            }
            Tool::SubSelection => {
                if let (Some(si), Some(pi)) = (self.selected_shape, self.dragging_point) {
                    let p = self.to_doc(w);
                    if let Some(shape) = self.vector_doc.shapes.get_mut(si) {
                        shape.set_control_point(pi, p);
                    }
                }
            }
            _ => {}
        }
    }

    fn stage_drag_end(&mut self, a: &Value) {
        let end = a.pt("x", "y");
        let start = a.pt("sx", "sy");
        match self.tool {
            Tool::Selection => self.grab_offset = None,
            Tool::SubSelection => self.dragging_point = None,
            Tool::Lasso => {
                let (lo, hi) = (start.min(end), start.max(end));
                self.multi_selected.clear();
                for (id, part) in &self.character.parts {
                    let Some(world) = part.bone.as_ref().and_then(|b| self.character.skeleton.world_transform_with_ik(b)) else { continue };
                    let pos = pony_render::part_world_position(part, &world);
                    if pos.x >= lo.x && pos.x <= hi.x && pos.y >= lo.y && pos.y <= hi.y {
                        self.multi_selected.insert(id.clone());
                    }
                }
                self.status = format!("Лассо: выделено частей {}", self.multi_selected.len());
            }
            Tool::Rectangle | Tool::Oval | Tool::Line | Tool::PolyStar => {
                let (fill, stroke) = (self.fill, self.stroke);
                // Фигура строится в координатах холста (SVG: Y вниз).
                let (s0, e0) = (self.to_doc(start), self.to_doc(end));
                let shape = match self.tool {
                    Tool::Rectangle => VectorShape::Rect {
                        x: s0.0.min(e0.0),
                        y: s0.1.min(e0.1),
                        w: (e0.0 - s0.0).abs(),
                        h: (e0.1 - s0.1).abs(),
                        fill,
                        stroke,
                        stroke_width: 1.5,
                        fill_gradient: None,
                    },
                    Tool::Oval => VectorShape::Ellipse {
                        cx: (s0.0 + e0.0) / 2.0,
                        cy: (s0.1 + e0.1) / 2.0,
                        rx: ((e0.0 - s0.0).abs() / 2.0).max(1.0),
                        ry: ((e0.1 - s0.1).abs() / 2.0).max(1.0),
                        fill,
                        stroke,
                        stroke_width: 1.5,
                        fill_gradient: None,
                    },
                    Tool::Line => VectorShape::Line { x1: s0.0, y1: s0.1, x2: e0.0, y2: e0.1, stroke, stroke_width: 2.0 },
                    _ => {
                        let r = (end - start).length().max(1.0);
                        let n = self.poly_sides;
                        let points = (0..n)
                            .map(|i| {
                                let ang = i as f32 / n as f32 * std::f32::consts::TAU + std::f32::consts::FRAC_PI_2;
                                self.to_doc(start + Vec2::new(ang.cos(), ang.sin()) * r)
                            })
                            .collect();
                        VectorShape::Polygon { points, fill, stroke, stroke_width: 1.5, fill_gradient: None }
                    }
                };
                self.vector_doc.add(shape);
            }
            _ => {}
        }
    }

    /// Free Transform: что двигать/вращать — точка кости под курсором (если
    /// скелет показан), иначе кость части (уже выбранный предок остаётся
    /// выбранным; с Alt — кость-родитель, у паппетов Animate это контейнер).
    fn xform_pick(&mut self, w: Vec2, alt: bool, ppw: f32) {
        let sk = &self.character.skeleton;
        let bone_hit = if self.show_skeleton {
            sk.bones
                .iter()
                .filter_map(|b| sk.world_transform_with_ik(&b.id).map(|t| (b.id.clone(), (t.position - w).length())))
                .filter(|(_, d)| *d < 8.0 * ppw)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(id, _)| id)
        } else {
            None
        };
        if let Some(b) = bone_hit {
            self.selected_bone = Some(b);
        } else {
            if let Some(hit) = part_at(&self.character, &self.hidden, &self.locked, w) {
                self.selected_layer = Some(hit);
            }
            let sk = &self.character.skeleton;
            let mut bone = self.selected_part_bone();
            let mut ancestor = bone.as_ref().and_then(|b| sk.find(b)).and_then(|b| b.parent.clone());
            let mut guard = 0;
            while let Some(anc) = ancestor.clone() {
                if self.selected_bone.as_deref() == Some(anc.as_str()) && sk.find(&anc).is_some_and(|b| b.parent.is_some()) {
                    bone = Some(anc);
                    break;
                }
                ancestor = sk.find(&anc).and_then(|b| b.parent.clone());
                guard += 1;
                if guard > 256 {
                    break;
                }
            }
            if alt {
                if let Some(parent) = bone.as_ref().and_then(|b| sk.find(b)).and_then(|b| b.parent.clone()) {
                    if sk.find(&parent).is_some_and(|p| p.parent.is_some()) {
                        bone = Some(parent);
                    }
                }
            }
            if bone.is_some() {
                self.selected_bone = bone;
            }
        }
        if let Some(b) = &self.selected_bone {
            self.status = format!("Xform: кость '{b}' (тащить — двигать, Shift — вращать, Alt+клик — кость-родитель)");
        }
    }

    fn place_bone(&mut self, target: Vec2) {
        let parent_id = self.selected_bone.clone().filter(|b| self.character.skeleton.find(b).is_some()).unwrap_or_else(|| default_root(&self.character));
        let Some(parent_world) = self.character.skeleton.world_transform_with_ik(&parent_id) else {
            self.status = "Родительская кость не найдена".into();
            return;
        };
        self.bone_count += 1;
        let new_id = anim::unique_name(&format!("Bone_{}", self.bone_count), |c| self.character.skeleton.find(c).is_some());
        let local_pos = pony_render::pivot_for_world_position(target, &parent_world);
        self.character.skeleton.add_bone(pony_core::skeleton::Bone {
            id: new_id.clone(),
            parent: Some(parent_id),
            local_transform: pony_core::skeleton::Transform2D { position: local_pos, ..Default::default() },
            length: 8.0,
        });
        self.selected_bone = Some(new_id.clone());
        self.status = format!("Кость '{new_id}' добавлена");
    }

    /// Пипетка: цвет кадра (персонаж + частицы на фоне неба) под точкой.
    fn eyedrop(&mut self, w: Vec2) {
        let visible = self.visible_character();
        let items = scene::draw_list(&visible, &self.lighting);
        let particles = if self.particles_enabled { scene::particle_list(&self.emitter) } else { Vec::new() };
        let view = scene::Affine::translate(0.5, 0.5).mul(&scene::Affine::scale(1.0, -1.0)).mul(&scene::Affine::translate(-w.x, -w.y));
        let svg = scene::scene_svg(&items, &particles, &view, 1, 1, &mut scene::DataUris::default());
        match scene::rasterize(&svg, 1, 1) {
            Ok(px) if px.len() >= 4 => {
                self.fill = RgbaColor::new(px[0], px[1], px[2], 255);
                self.status = format!("Пипетка: Fill = #{:02x}{:02x}{:02x}", px[0], px[1], px[2]);
            }
            Ok(_) => {}
            Err(e) => self.status = format!("Пипетка: {e}"),
        }
    }

    // -------------------------------------------------------- рисунок

    fn save_drawing(&mut self) {
        if self.vector_doc.is_empty() {
            return;
        }
        let (min_x, min_y, max_x, max_y) = self.vector_doc.bounds().unwrap_or((0.0, 0.0, 1.0, 1.0));
        let draw_center = Vec2::new((min_x + max_x) / 2.0, -(min_y + max_y) / 2.0);
        let draw_size = Vec2::new((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));
        if let Some(edit_id) = self.editing_part.clone() {
            // Обновляем существующую часть на месте: тот же id и файл.
            let Some(part) = self.character.parts.get(&edit_id).cloned() else {
                self.editing_part = None;
                self.edit_vb = None;
                self.status = "Редактируемая часть больше не существует — сохранение отменено".into();
                return;
            };
            if let PartSource::Vector { path } = &part.source {
                match std::fs::write(path, self.vector_doc.to_svg_string()) {
                    Ok(()) => {
                        // Новый viewBox (габарит фигур) в координатах прежнего
                        // квада части: часть остаётся там же и того же масштаба.
                        let (pivot, size) = match self.edit_vb {
                            Some([vx, vy, vw, vh]) => {
                                let (nx, ny, nw, nh) = self.vector_doc.view_box();
                                let old_size = pony_render::part_render_size(&part);
                                let qx = (nx + nw / 2.0 - vx) / vw - 0.5;
                                let qy = 0.5 - (ny + nh / 2.0 - vy) / vh;
                                (part.pivot + Vec2::new(qx * old_size.x, qy * old_size.y), Vec2::new(nw / vw * old_size.x, nh / vh * old_size.y))
                            }
                            None => {
                                let bone_world = part.bone.as_ref().and_then(|b| self.character.skeleton.world_transform_with_ik(b)).unwrap_or_default();
                                (pony_render::pivot_for_world_position(draw_center, &bone_world), draw_size)
                            }
                        };
                        if let Some(p) = self.character.parts.get_mut(&edit_id) {
                            p.pivot = pivot;
                            p.size = Some(size);
                        }
                        self.edit_vb = None;
                        self.vector_doc.clear();
                        self.selected_shape = None;
                        self.editing_part = None;
                        self.status = format!("'{edit_id}' обновлён ({path})");
                    }
                    Err(err) => self.status = format!("Ошибка сохранения SVG: {err}"),
                }
            }
            return;
        }
        self.svg_save_count += 1;
        let part_id = anim::unique_name(&format!("drawn_{}", self.svg_save_count), |c| self.character.parts.contains_key(c) || std::path::Path::new(&format!("{c}.svg")).exists());
        let path = format!("{part_id}.svg");
        match std::fs::write(&path, self.vector_doc.to_svg_string()) {
            Ok(()) => {
                let bone_id = self.selected_part_bone().unwrap_or_else(|| default_bone(&self.character));
                let bone_world = self.character.skeleton.world_transform_with_ik(&bone_id).unwrap_or_default();
                let pivot = pony_render::pivot_for_world_position(draw_center, &bone_world);
                let max_layer = self.character.parts.values().map(|p| p.layer).max().unwrap_or(0);
                self.character.add_part(
                    Part::new(&part_id, PartKind::Custom, PartSource::Vector { path: path.clone() }).with_bone(&bone_id).with_pivot(pivot).with_size(draw_size).with_layer(max_layer + 1),
                );
                self.vector_doc.clear();
                self.selected_shape = None;
                self.selected_layer = Some(part_id.clone());
                self.status = format!("'{part_id}' -> {path}, кость '{bone_id}', {:.0}x{:.0}", draw_size.x, draw_size.y);
            }
            Err(err) => self.status = format!("Ошибка сохранения SVG: {err}"),
        }
    }

    fn start_editing_svg_part(&mut self, part_id: &str) {
        let Some(PartSource::Vector { path }) = self.character.parts.get(part_id).map(|p| p.source.clone()) else {
            self.status = format!("'{part_id}' — не SVG-часть");
            return;
        };
        self.status = match std::fs::read_to_string(&path) {
            Ok(text) => match VectorDoc::from_svg_str(&text) {
                Ok(doc) => {
                    let count = doc.shapes.len();
                    let note = if doc.unsupported.is_empty() { String::new() } else { format!(" (пропущены нередактируемые элементы: {})", doc.unsupported.join(", ")) };
                    self.edit_vb = Some(parse_view_box(&text).unwrap_or_else(|| {
                        let (x, y, w, h) = doc.view_box();
                        [x, y, w, h]
                    }));
                    self.vector_doc = doc;
                    self.editing_part = Some(part_id.to_string());
                    self.tool = Tool::SubSelection;
                    self.selected_shape = None;
                    format!("Редактирую '{part_id}' ({path}) — {count} фигур(ы), инструмент SubSel{note}")
                }
                Err(err) => format!("Не удалось разобрать SVG '{part_id}' ({path}): {err}"),
            },
            Err(err) => format!("Не удалось прочитать {path}: {err}"),
        };
    }

    // -------------------------------------------------------- свойства

    fn part_set(&mut self, a: &Value) {
        let id = a.s("id").unwrap_or("").to_string();
        let Some(part) = self.character.parts.get(&id).cloned() else { return };
        if let Some(target_bone) = a.s("bone").filter(|b| Some(*b) != part.bone.as_deref() && self.character.skeleton.find(b).is_some()) {
            // Часть остаётся на месте на экране: смещение пересчитывается.
            let old_world = part.bone.as_ref().and_then(|b| self.character.skeleton.world_transform_with_ik(b)).unwrap_or_default();
            let keep_at = pony_render::part_world_position(&part, &old_world);
            let new_world = self.character.skeleton.world_transform_with_ik(target_bone).unwrap_or_default();
            let new_pivot = pony_render::pivot_for_world_position(keep_at, &new_world);
            if let Some(p) = self.character.parts.get_mut(&id) {
                p.bone = Some(target_bone.to_string());
                p.pivot = new_pivot;
            }
            self.status = format!("'{id}' перепривязан к кости '{target_bone}'");
        }
        let Some(p) = self.character.parts.get_mut(&id) else { return };
        if let Some(v) = a.f("px") {
            p.pivot.x = v;
        }
        if let Some(v) = a.f("py") {
            p.pivot.y = v;
        }
        if a.f("w").is_some() || a.f("h").is_some() {
            let mut size = pony_render::part_render_size(p);
            if let Some(v) = a.f("w") {
                size.x = v.clamp(1.0, 1000.0);
            }
            if let Some(v) = a.f("h") {
                size.y = v.clamp(1.0, 1000.0);
            }
            p.size = Some(size);
        }
        if a.b("reset_size") {
            p.size = None;
        }
        if let Some(v) = a.i("layer") {
            p.layer = v as i32;
        }
        if let Some(mask) = a.get("clip_by") {
            let target = mask.as_str().filter(|m| !m.is_empty()).map(str::to_string);
            if target != p.clip_by {
                p.clip_by = target.clone();
                self.status = match target {
                    Some(m) => format!("'{id}' теперь маскируется частью '{m}'"),
                    None => format!("'{id}': маска снята"),
                };
            }
        }
    }

    fn bone_delete(&mut self, bid: &str) {
        let sk = &self.character.skeleton;
        let Some(bone) = sk.find(bid) else { return };
        let Some(parent_id) = bone.parent.clone() else { return };
        let doomed: Vec<String> = sk.bones.iter().filter(|b| sk.is_descendant_of(&b.id, bid)).map(|b| b.id.clone()).collect();
        let keep_at: Vec<(String, Vec2)> = self
            .character
            .parts
            .values()
            .filter(|p| p.bone.as_ref().is_some_and(|b| doomed.contains(b)))
            .filter_map(|p| {
                let w = sk.world_transform_with_ik(p.bone.as_ref()?)?;
                Some((p.id.clone(), pony_render::part_world_position(p, &w)))
            })
            .collect();
        let removed = self.character.skeleton.remove_subtree(bid);
        let parent_world = self.character.skeleton.world_transform_with_ik(&parent_id).unwrap_or_default();
        for (part_id, world_pos) in keep_at {
            if let Some(part) = self.character.parts.get_mut(&part_id) {
                part.bone = Some(parent_id.clone());
                part.pivot = pony_render::pivot_for_world_position(world_pos, &parent_world);
            }
        }
        self.selected_bone = None;
        self.status = format!("Удалено костей: {}", removed.len());
    }

    fn bone_rename(&mut self, bid: &str, new_id: &str) -> Result<Value, String> {
        let new_id = new_id.trim().to_string();
        if new_id.is_empty() || new_id == bid {
            return Ok(Value::Null);
        }
        if !self.character.skeleton.rename_bone(bid, &new_id) {
            return Err(format!("Не удалось переименовать: имя '{new_id}' занято"));
        }
        for part in self.character.parts.values_mut() {
            if part.bone.as_deref() == Some(bid) {
                part.bone = Some(new_id.clone());
            }
        }
        for anim in self.character.animations.values_mut() {
            for track in &mut anim.tracks {
                if let AnimTarget::Bone { id, .. } = &mut track.target {
                    if id == bid {
                        *id = new_id.clone();
                    }
                }
            }
        }
        self.selected_bone = Some(new_id.clone());
        self.status = format!("Кость '{bid}' -> '{new_id}'");
        Ok(Value::Null)
    }

    fn bone_reparent(&mut self, bid: &str, np: &str) -> Result<Value, String> {
        let current = self.character.skeleton.find(bid).and_then(|b| b.parent.clone());
        if current.as_deref() == Some(np) {
            return Ok(Value::Null);
        }
        let world_before = self.character.skeleton.world_transform(bid);
        if !self.character.skeleton.reparent(bid, np) {
            return Err("Нельзя: это создало бы цикл в иерархии".into());
        }
        if let (Some(before), Some(parent_world)) = (world_before, self.character.skeleton.world_transform(np)) {
            if let Some(bone) = self.character.skeleton.bones.iter_mut().find(|b| b.id == bid) {
                bone.local_transform.position = pony_render::pivot_for_world_position(before.position, &parent_world);
                bone.local_transform.rotation = before.rotation - parent_world.rotation;
            }
        }
        self.status = format!("'{bid}' теперь дочерняя для '{np}'");
        Ok(Value::Null)
    }

    fn ik_create(&mut self, root: &str, mid: &str, tip: &str) -> Result<Value, String> {
        if root.is_empty() || mid.is_empty() || tip.is_empty() {
            return Err("Выбери Root, Mid и Tip".into());
        }
        if root == mid || mid == tip || root == tip {
            return Err("Root, Mid и Tip должны быть разными костями".into());
        }
        let tip_world = self.character.skeleton.world_transform_with_ik(tip).unwrap_or_default();
        let sk = &self.character.skeleton;
        let ik_id = anim::unique_name(&format!("ik_{}", sk.ik_constraints.len() + 1), |c| sk.ik_constraints.iter().any(|k| k.id == c));
        let mut constraint = pony_core::IkConstraint::new(ik_id, root, mid, tip);
        constraint.target = tip_world.position;
        self.character.skeleton.add_ik_constraint(constraint);
        self.status = "IK-констрейнт создан".into();
        Ok(Value::Null)
    }

    fn env_set(&mut self, a: &Value) {
        if let Some(v) = a.f("ambient_intensity") {
            self.lighting.ambient.intensity = v.clamp(0.0, 2.0);
        }
        if let Some(c) = a.rgb("ambient_color") {
            self.lighting.ambient.color = c;
        }
        if let Some(v) = a.get("sun_enabled").and_then(Value::as_bool) {
            self.sun_enabled = v;
        }
        if let Some(c) = a.rgb("sun_color") {
            self.sun_color = c;
        }
        if let Some(v) = a.f("sun_intensity") {
            self.sun_intensity = v.clamp(0.0, 2.0);
        }
        if let Some(v) = a.get("point_enabled").and_then(Value::as_bool) {
            self.point_enabled = v;
        }
        if let Some(c) = a.rgb("point_color") {
            self.point_color = c;
        }
        if let Some(v) = a.f("point_intensity") {
            self.point_intensity = v.clamp(0.0, 3.0);
        }
        if let Some(v) = a.f("point_radius") {
            self.point_radius = v.clamp(10.0, 300.0);
        }
        if let Some(v) = a.f("point_x") {
            self.point_pos.x = v;
        }
        if let Some(v) = a.f("point_y") {
            self.point_pos.y = v;
        }
        self.update_lighting();
        if let Some(v) = a.get("particles").and_then(Value::as_bool) {
            self.particles_enabled = v;
        }
        if let Some(kind) = a.s("kind").and_then(particle_kind_from) {
            if kind != self.particle_kind {
                self.particle_kind = kind;
                let (pos, rate) = (self.emitter.position, self.emitter.rate);
                self.emitter = pony_core::ParticleEmitter::new(kind, pos, rate);
            }
        }
        if let Some(v) = a.f("rate") {
            self.emitter.rate = v.clamp(0.0, 60.0);
        }
        if let Some(v) = a.f("lifetime") {
            self.emitter.lifetime = v.clamp(0.1, 5.0);
        }
        if let Some(v) = a.f("spread") {
            self.emitter.spread = v.clamp(0.0, 150.0);
        }
        if let Some(v) = a.f("ex") {
            self.emitter.position.x = v;
        }
        if let Some(v) = a.f("ey") {
            self.emitter.position.y = v;
        }
    }

    fn symbol_convert(&mut self, name: &str) -> Result<Value, String> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("Введи имя символа".into());
        }
        let idx = self.selected_shape.ok_or("Выбери фигуру инструментом SubSelection")?;
        let shape = self.vector_doc.shapes.get(idx).cloned().ok_or("Фигура не найдена")?;
        if matches!(shape, VectorShape::Instance { .. }) {
            return Err("Это уже инстанс символа".into());
        }
        self.vector_doc.upsert_symbol(name.clone(), vec![shape]);
        self.vector_doc.shapes[idx] = VectorShape::Instance { symbol: name.clone(), transform: (1.0, 0.0, 0.0, 1.0, 0.0, 0.0), fill_override: None };
        self.status = format!("Символ '{name}' создан, фигура заменена на инстанс");
        Ok(Value::Null)
    }

    fn symbol_place(&mut self, name: &str) -> Result<Value, String> {
        let (min_x, min_y, max_x, max_y) = self.vector_doc.bounds().unwrap_or((0.0, 0.0, 100.0, 100.0));
        let inst = self.vector_doc.new_instance_centered_at(name, (min_x + max_x) / 2.0, (min_y + max_y) / 2.0).ok_or_else(|| format!("Символ '{name}' не найден"))?;
        self.vector_doc.add(inst);
        self.selected_shape = Some(self.vector_doc.shapes.len() - 1);
        self.tool = Tool::SubSelection;
        self.status = format!("Инстанс символа '{name}' размещён");
        Ok(Value::Null)
    }

    fn gradient_save(&mut self, a: &Value) -> Result<Value, String> {
        let name = a.s("name").unwrap_or("").trim().to_string();
        if name.is_empty() {
            return Err("Введи имя градиента".into());
        }
        let kind = if a.b("radial") {
            pony_core::GradientKind::Radial { cx: a.f("cx").unwrap_or(50.0), cy: a.f("cy").unwrap_or(50.0), r: a.f("r").unwrap_or(50.0).max(0.01) }
        } else {
            pony_core::GradientKind::Linear { x1: a.f("x1").unwrap_or(0.0), y1: a.f("y1").unwrap_or(0.0), x2: a.f("x2").unwrap_or(100.0), y2: a.f("y2").unwrap_or(0.0) }
        };
        let stops: Vec<pony_core::GradientStop> = a
            .get("stops")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(|s| Some(pony_core::GradientStop { offset: s.f("offset")?.clamp(0.0, 1.0), color: s.color("color")? })).collect())
            .unwrap_or_default();
        if stops.is_empty() {
            return Err("Нужна хотя бы одна остановка цвета".into());
        }
        self.vector_doc.upsert_gradient(pony_core::GradientDef::new(name.clone(), kind, stops));
        self.vector_doc.sync_gradient_fallback_colors();
        self.status = format!("Градиент '{name}' сохранён");
        Ok(Value::Null)
    }

    fn boolean_run(&mut self, a: &Value) -> Result<Value, String> {
        let (fi, bi) = (a.i("front").unwrap_or(-1), a.i("back").unwrap_or(-1));
        if fi < 0 || bi < 0 {
            return Err("Выбери обе фигуры".into());
        }
        if fi == bi {
            return Err("Верхняя и нижняя фигура не могут совпадать".into());
        }
        let op = match a.s("op").unwrap_or("union") {
            "union" => pony_core::BooleanOp::Union,
            "difference" => pony_core::BooleanOp::Difference,
            "intersection" => pony_core::BooleanOp::Intersection,
            "xor" => pony_core::BooleanOp::Xor,
            _ => pony_core::BooleanOp::Divide,
        };
        let (fi, bi) = (fi as usize, bi as usize);
        let (Some(front), Some(back)) = (self.vector_doc.shapes.get(fi).cloned(), self.vector_doc.shapes.get(bi).cloned()) else {
            return Err("Фигура не найдена".into());
        };
        let result = pony_core::boolean_op(&front, &back, op).ok_or("Эти фигуры не поддерживают булевы операции")?;
        let mut to_remove = [fi, bi];
        to_remove.sort_unstable();
        self.vector_doc.shapes.remove(to_remove[1]);
        self.vector_doc.shapes.remove(to_remove[0]);
        let pieces = result.pieces.len();
        for piece in &result.pieces {
            self.vector_doc.shapes.push(pony_core::piece_to_shape(piece));
        }
        self.selected_shape = None;
        self.status = if result.dropped_holes > 0 {
            format!("Boolean-операция: получено {pieces} кусков(а), но {} внутренних контуров (дырок) отброшено — VectorShape::Polygon не хранит дырки", result.dropped_holes)
        } else if pieces == 0 {
            "Boolean-операция: результат пуст (фигуры не пересекаются либо взаимно уничтожились)".to_string()
        } else {
            format!("Boolean-операция выполнена: получено {pieces} куска(ов)")
        };
        Ok(json!({ "done": true }))
    }

    // -------------------------------------------------------- разделы

    fn sec_ui(&self) -> Value {
        let strip = |c: &Character, p: &AnimationPlayer| {
            let anim = p.current_name().and_then(|n| c.animations.get(n));
            json!({
                "duration": anim.map(|a| a.duration).unwrap_or(1.0),
                "keys": anim.map(|a| a.tracks.iter().flat_map(|t| t.keyframes.iter().map(|k| k.time)).collect::<Vec<_>>()).unwrap_or_default(),
                "time": p.time(),
            })
        };
        let mut characters = vec![json!({
            "name": self.active_name, "active": true, "parts": self.character.parts.len(), "bones": self.character.skeleton.bones.len(),
            "playing": self.playing, "strip": strip(&self.character, &self.player),
        })];
        for (i, slot) in self.others.iter().enumerate() {
            characters.push(json!({
                "name": slot.name, "active": false, "index": i, "parts": slot.character.parts.len(), "bones": slot.character.skeleton.bones.len(),
                "playing": slot.player.current_name().is_some(), "strip": strip(&slot.character, &slot.player),
            }));
        }
        let sorted = |s: &HashSet<String>| s.iter().cloned().collect::<BTreeSet<_>>();
        let sel_world = self
            .selected_part_bone()
            .and_then(|b| self.character.skeleton.world_transform_with_ik(&b))
            .map(|w| json!([w.position.x, w.position.y]));
        json!({
            "tool": self.tool.key(),
            "fill": rgba_json(self.fill),
            "stroke": rgba_json(self.stroke),
            "poly_sides": self.poly_sides,
            "selected_layer": self.selected_layer,
            "selected_bone": self.selected_bone,
            "sel_world": sel_world,
            "multi": sorted(&self.multi_selected),
            "selected_shape": self.selected_shape,
            "editing_part": self.editing_part,
            "hidden": sorted(&self.hidden),
            "locked": sorted(&self.locked),
            "show_skeleton": self.show_skeleton,
            "placing_bone": self.placing_bone,
            "status": self.status,
            "undo": self.history.undo.len(),
            "redo": self.history.redo.len(),
            "asset_path": self.asset_path,
            "dirty": self.dirty(),
            "title": self.title(),
            "active_name": self.active_name,
            "timeline_all": self.timeline_all,
            "characters": characters,
            "channel": anim::channel_key(self.timeline_channel),
            "channel_label": anim::channel_label(self.timeline_channel),
            "selected_keyframe": self.selected_keyframe,
            "playing": self.playing,
            "busy": self.busy(),
            "exporting": self.exporting,
            "fit": { "n": self.fit_count, "kind": self.fit_kind },
            "script_log": self.script_log,
            "names": self.default_names(),
            "camera_zoom": self.camera.zoom,
        })
    }

    fn sec_doc(&self) -> Value {
        let c = &self.character;
        let mut parts: Vec<&Part> = c.parts.values().collect();
        parts.sort_by(|a, b| a.layer.cmp(&b.layer).then_with(|| a.id.cmp(&b.id)));
        let parts: Vec<Value> = parts
            .iter()
            .map(|p| {
                let size = pony_render::part_render_size(p);
                json!({
                    "id": p.id, "kind": format!("{:?}", p.kind), "bone": p.bone, "layer": p.layer,
                    "pivot": [p.pivot.x, p.pivot.y], "size": [size.x, size.y], "has_size": p.size.is_some(),
                    "clip_by": p.clip_by, "group": p.group, "source": source_label(&p.source),
                    "svg": match &p.source { PartSource::Vector { path } => Some(path.clone()), _ => None },
                })
            })
            .collect();
        let bones: Vec<Value> = c
            .skeleton
            .bones
            .iter()
            .map(|b| {
                let t = &b.local_transform;
                json!({ "id": b.id, "parent": b.parent, "x": t.position.x, "y": t.position.y, "rot": t.rotation.to_degrees(), "sx": t.scale.x, "sy": t.scale.y, "length": b.length })
            })
            .collect();
        let ik: Vec<Value> = c
            .skeleton
            .ik_constraints
            .iter()
            .map(|k| json!({ "id": k.id, "root": k.root, "mid": k.mid, "tip": k.tip, "tx": k.target.x, "ty": k.target.y, "weight": k.weight, "enabled": k.enabled, "pole": k.pole_target.map(|p| [p.x, p.y]) }))
            .collect();
        let groups: Vec<Value> = c
            .groups
            .groups
            .iter()
            .map(|g| json!({ "id": g.id, "name": g.name, "parent": g.parent, "hidden": g.hidden, "members": c.parts.values().filter(|p| p.group.as_deref() == Some(g.id.as_str())).count() }))
            .collect();
        let mut anims: Vec<&String> = c.animations.keys().collect();
        anims.sort();
        let mut library: Vec<String> = c.parts.values().map(|p| source_label(&p.source)).collect();
        library.sort();
        library.dedup();
        json!({
            "name": c.name, "parts": parts, "bones": bones, "ik": ik, "groups": groups, "anims": anims,
            "facing_yaw": c.facing_yaw.to_degrees(), "library": library,
            "counts": { "parts": c.parts.len(), "bones": c.skeleton.bones.len(), "anims": c.animations.len(), "shapes": self.vector_doc.shapes.len() },
        })
    }

    fn sec_anim(&self) -> Value {
        let anim = self.current_anim();
        let rows = anim::timeline_rows(&self.character, anim, self.timeline_channel, self.selected_bone.as_deref());
        let duration = anim.map(|a| a.duration).unwrap_or(1.0).max(0.01);
        json!({
            "name": self.player.current_name(),
            "duration": anim.map(|a| a.duration),
            "looping": anim.map(|a| a.looping).unwrap_or(false),
            "total_frames": (duration * FPS).round().max(1.0) as i64,
            "rows": rows,
        })
    }

    fn sec_play(&self) -> Value {
        let total = self.current_anim().map(|a| (a.duration * FPS).round() as i64).unwrap_or(0);
        json!({ "time": self.player.time(), "frame": (self.player.time() * FPS).round() as i64, "total": total, "playing": self.playing })
    }

    fn sec_frame(&self) -> Value {
        let mut visible = self.visible_character();
        // Пока часть правится на холсте, её старая картинка не рисуется —
        // иначе фигуры холста лежали бы поверх своей же копии.
        if let Some(id) = &self.editing_part {
            visible.parts.remove(id);
        }
        let items: Vec<Value> = scene::draw_list(&visible, &self.lighting)
            .into_iter()
            .map(|it| {
                let css = |c: [u8; 4]| format!("rgba({},{},{},{})", c[0], c[1], c[2], c[3] as f32 / 255.0);
                json!({
                    "id": it.id,
                    "href": it.asset.as_ref().map(scene::Asset::url),
                    "fill": css(it.fallback),
                    "m": it.model.0,
                    "light": it.light,
                    "mask": it.mask.map(|m| json!({ "href": m.asset.as_ref().map(scene::Asset::url), "fill": css(m.fallback), "m": m.model.0 })),
                })
            })
            .collect();
        let particles: Vec<Value> = if self.particles_enabled {
            scene::particle_list(&self.emitter).into_iter().map(|p| json!([p.x, p.y, p.size, format!("rgba({},{},{},{})", p.color[0], p.color[1], p.color[2], p.color[3] as f32 / 255.0)])).collect()
        } else {
            Vec::new()
        };
        let sk = &self.character.skeleton;
        let bones: Vec<Value> = sk
            .bones
            .iter()
            .filter_map(|b| {
                let w = sk.world_transform_with_ik(&b.id)?;
                let parent = b.parent.as_ref().and_then(|p| sk.world_transform_with_ik(p)).map(|p| [p.position.x, p.position.y]);
                Some(json!({ "id": b.id, "x": w.position.x, "y": w.position.y, "parent": parent }))
            })
            .collect();
        let boxed = |id: &String| -> Option<Value> {
            let part = self.character.parts.get(id)?;
            let (c, h) = part_screen_box(&self.character, part)?;
            let bone = sk.world_transform_with_ik(part.bone.as_ref()?)?.position;
            Some(json!({ "c": [c.x, c.y], "h": [h.x, h.y], "bone": [bone.x, bone.y] }))
        };
        let shake = self.camera.shake_offset(self.elapsed);
        json!({
            "items": items,
            "particles": particles,
            "camera": { "x": self.camera.position.x + shake.x, "y": self.camera.position.y + shake.y, "rot": self.camera.rotation, "zoom": self.camera.zoom.max(0.0001) },
            "bones": bones,
            "sel": self.selected_layer.as_ref().and_then(boxed),
            "multi": self.multi_selected.iter().filter_map(boxed).collect::<Vec<_>>(),
            "bounds": character_bounds(&self.character, &self.hidden).map(|(lo, hi)| [lo.x, lo.y, hi.x, hi.y]),
        })
    }

    fn sec_vec(&self) -> Value {
        let svg = self.vector_doc.to_svg_string();
        // Внутренность <svg>…</svg>: интерфейс вставляет фигуры прямо в Stage.
        let markup = svg.find('>').and_then(|start| svg.rfind("</svg>").map(|end| svg[start + 1..end].to_string())).unwrap_or_default();
        let shapes: Vec<Value> = self
            .vector_doc
            .shapes
            .iter()
            .map(|s| {
                json!({
                    "label": shape_kind_label(s),
                    "flat": pony_core::flatten_shape_to_contour(s).is_some(),
                    "instance": matches!(s, VectorShape::Instance { .. }),
                    "gradable": !matches!(s, VectorShape::Instance { .. } | VectorShape::Line { .. } | VectorShape::Polyline { .. }),
                })
            })
            .collect();
        let symbols: Vec<Value> = self
            .vector_doc
            .symbols
            .iter()
            .map(|d| json!({ "name": d.name, "shapes": d.shapes.len(), "instances": self.vector_doc.shapes.iter().filter(|s| matches!(s, VectorShape::Instance { symbol, .. } if symbol == &d.name)).count() }))
            .collect();
        let gradients: Vec<Value> = self
            .vector_doc
            .gradients
            .iter()
            .map(|g| {
                let stops: Vec<Value> = g.stops.iter().map(|s| json!({ "offset": s.offset, "color": rgba_json(s.color) })).collect();
                match g.kind {
                    pony_core::GradientKind::Linear { x1, y1, x2, y2 } => json!({ "name": g.name, "radial": false, "x1": x1, "y1": y1, "x2": x2, "y2": y2, "stops": stops }),
                    pony_core::GradientKind::Radial { cx, cy, r } => json!({ "name": g.name, "radial": true, "cx": cx, "cy": cy, "r": r, "stops": stops }),
                }
            })
            .collect();
        let map = self.doc_map();
        let cps: Vec<[f32; 2]> = self
            .selected_shape
            .and_then(|i| self.vector_doc.shapes.get(i))
            .map(|s| s.control_points().into_iter().map(|(x, y)| map.apply(x, y).into()).collect())
            .unwrap_or_default();
        json!({ "markup": markup, "map": map.0, "shapes": shapes, "symbols": symbols, "gradients": gradients, "cps": cps, "empty": self.vector_doc.is_empty() })
    }

    fn sec_env(&self) -> Value {
        let l = &self.lighting;
        json!({
            "ambient_intensity": l.ambient.intensity, "ambient_color": rgb_u8(l.ambient.color),
            "sun_enabled": self.sun_enabled, "sun_color": rgb_u8(self.sun_color), "sun_intensity": self.sun_intensity,
            "point_enabled": self.point_enabled, "point_color": rgb_u8(self.point_color), "point_intensity": self.point_intensity,
            "point_radius": self.point_radius, "point_x": self.point_pos.x, "point_y": self.point_pos.y,
            "particles": self.particles_enabled, "kind": format!("{:?}", self.particle_kind), "rate": self.emitter.rate,
            "lifetime": self.emitter.lifetime, "spread": self.emitter.spread, "ex": self.emitter.position.x, "ey": self.emitter.position.y,
            "alive": self.emitter.particles.len(),
        })
    }

    fn sec_xfl(&self) -> Value {
        let x = &self.xfl;
        let Some(doc) = &x.doc else {
            return json!({ "loaded": false, "info": x.info, "path": x.path, "show": x.show });
        };
        let mut names = doc.symbol_names();
        names.sort_by_key(|n| (!n.starts_with('~'), n.to_lowercase()));
        let symbols: Vec<Value> = names.iter().map(|n| json!([n, doc.symbols[*n].frame_count()])).collect();
        let tl = doc.timeline(x.symbol.as_deref());
        json!({
            "loaded": true, "info": x.info, "path": x.path, "show": x.show, "name": doc.name,
            "symbols": symbols, "main_frames": doc.main.frame_count(),
            "symbol": x.symbol, "frame": x.frame, "count": tl.map(|t| t.frame_count()).unwrap_or(1),
            "labels": tl.map(|t| t.frame_labels()).unwrap_or_default(),
            "scale": x.opts.scale, "expand": x.opts.expand_containers, "hidden": doc.include_hidden_layers,
        })
    }

    fn sec_xfl_preview(&self) -> Value {
        let x = &self.xfl;
        match &x.doc {
            Some(doc) => json!(doc.render_svg(x.symbol.as_deref(), x.frame).map(|r| r.svg)),
            None => Value::Null,
        }
    }
}

/// GIF / спрайт-лист: рендер — CPU (resvg), тем же списком отрисовки, что и
/// Stage. Выполняется в фоновом потоке.
fn render_export(mut temp: Character, name: &str, lighting: &pony_core::Lighting, camera: &pony_core::Camera, kind: &str, path: &str) -> Result<String, String> {
    let anim = temp.animations.get(name).ok_or_else(|| format!("анимация '{name}' не найдена"))?;
    let total_frames = (anim.duration * FPS).round().max(1.0) as usize;
    let mut player = AnimationPlayer::new();
    player.play(name);
    let mut uris = scene::DataUris::default();
    let mut frames = Vec::with_capacity(total_frames);
    for _ in 0..total_frames {
        player.apply(&mut temp);
        let items = scene::draw_list(&temp, lighting);
        let view = scene::camera_view(camera, player.time(), SCENE_WIDTH as f32, SCENE_HEIGHT as f32);
        let svg = scene::scene_svg(&items, &[], &view, SCENE_WIDTH, SCENE_HEIGHT, &mut uris);
        let rgba = scene::rasterize(&svg, SCENE_WIDTH, SCENE_HEIGHT).map_err(|e| format!("Ошибка рендера кадра: {e}"))?;
        frames.push(pony_render::FrameOutput { width: SCENE_WIDTH, height: SCENE_HEIGHT, rgba, rendered_on: "CPU (resvg)".into() });
        player.advance(&temp, 1.0 / FPS);
    }
    if kind == "gif" {
        let delay_cs = (100.0 / FPS).round() as u16;
        pony_render::export_gif(path, &frames, delay_cs).map_err(|e| format!("Ошибка экспорта GIF: {e}"))?;
        Ok(format!("GIF сохранён: {path} ({} кадров)", frames.len()))
    } else {
        let layout = pony_render::export_spritesheet(path, &frames, 6).map_err(|e| format!("Ошибка экспорта спрайт-листа: {e}"))?;
        Ok(format!("Спрайт-лист сохранён: {path} ({}x{} кадров)", layout.columns, layout.rows))
    }
}

/// viewBox из текста SVG-файла.
fn parse_view_box(svg: &str) -> Option<[f32; 4]> {
    let start = svg.find("viewBox=\"")? + "viewBox=\"".len();
    let end = start + svg[start..].find('"')?;
    let v: Vec<f32> = svg[start..end].split(|c: char| c == ' ' || c == ',').filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
    (v.len() == 4 && v[2] > 0.0 && v[3] > 0.0).then(|| [v[0], v[1], v[2], v[3]])
}

fn default_root(character: &Character) -> String {
    character.skeleton.bones.iter().find(|b| b.parent.is_none()).map(|b| b.id.clone()).unwrap_or_else(|| "Root".into())
}

fn particle_kind_from(s: &str) -> Option<pony_core::ParticleKind> {
    use pony_core::ParticleKind::*;
    [Dust, Snow, Rain, Spark, Magic, Smoke, Cloud].into_iter().find(|k| format!("{k:?}") == s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(app: &mut App, name: &str, args: Value) -> Value {
        app.handle(&json!({ "id": 1, "cmd": name, "args": args }))
    }

    #[test]
    fn hello_sends_every_section_and_then_only_changes() {
        let mut app = App::new();
        let r = cmd(&mut app, "hello", json!({}));
        for s in ["ui", "doc", "anim", "play", "frame", "vec", "env", "xfl"] {
            assert!(r["patch"].get(s).is_some(), "раздел {s} должен прийти при hello");
        }
        let r = cmd(&mut app, "set_tool", json!({ "tool": "rect" }));
        assert_eq!(r["patch"]["ui"]["tool"], "rect");
        assert!(r["patch"].get("doc").is_none(), "не изменившийся раздел не отправляется");
    }

    #[test]
    fn drawing_a_rectangle_then_saving_creates_an_svg_part_and_undo_removes_it() {
        let dir = std::env::temp_dir().join(format!("pony_studio_draw_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let mut app = App::new();
        cmd(&mut app, "set_tool", json!({ "tool": "rect" }));
        cmd(&mut app, "stage_drag_end", json!({ "sx": -10.0, "sy": 10.0, "x": 30.0, "y": -20.0 }));
        assert_eq!(app.vector_doc.shapes.len(), 1);
        let (x0, y0, x1, y1) = app.vector_doc.bounds().unwrap();
        cmd(&mut app, "save_drawing", json!({}));
        std::env::set_current_dir(old).unwrap();
        let part = app.character.parts.get("drawn_1").expect("часть создана");
        assert_eq!(part.size, Some(Vec2::new(x1 - x0, y1 - y0)), "размер части — габарит рисунка");
        assert!((x1 - x0 - 40.0).abs() < 4.0 && (y1 - y0 - 30.0).abs() < 4.0);
        assert!(app.vector_doc.is_empty());
        cmd(&mut app, "undo", json!({}));
        assert!(!app.character.parts.contains_key("drawn_1"));
        assert_eq!(app.vector_doc.shapes.len(), 1, "отмена возвращает и холст");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn timeline_double_click_adds_a_key_and_dragging_moves_it() {
        let mut app = App::new();
        cmd(&mut app, "demo", json!({}));
        cmd(&mut app, "tl_dblclick", json!({ "label": "head", "frame": 5 }));
        let rows = anim::timeline_rows(&app.character, app.current_anim(), BoneChannel::PositionY, None);
        assert!(rows.iter().find(|r| r.label == "head").unwrap().frames.contains(&5));
        cmd(&mut app, "tl_drag_start", json!({ "label": "head", "frame": 5 }));
        app.handle(&json!({ "id": 2, "cmd": "tl_drag", "args": { "frame": 7 }, "down": true }));
        cmd(&mut app, "tl_drag", json!({ "frame": 8 }));
        let rows = anim::timeline_rows(&app.character, app.current_anim(), BoneChannel::PositionY, None);
        let frames = &rows.iter().find(|r| r.label == "head").unwrap().frames;
        assert!(frames.contains(&8) && !frames.contains(&5), "{frames:?}");
        assert_eq!(app.selected_keyframe, Some(("head".into(), 8)));
    }

    #[test]
    fn selection_drag_moves_the_part_by_the_grabbed_point() {
        let mut app = App::new();
        cmd(&mut app, "demo", json!({}));
        let body = app.character.parts["body"].clone();
        let steps = app.history.undo.len();
        let (c0, _) = part_screen_box(&app.character, &body).unwrap();
        cmd(&mut app, "stage_drag_start", json!({ "x": c0.x + 3.0, "y": c0.y, "ppw": 1.0 }));
        let grabbed = app.selected_layer.clone().expect("под точкой есть часть");
        let (c, _) = part_screen_box(&app.character, &app.character.parts[&grabbed]).unwrap();
        cmd(&mut app, "stage_drag", json!({ "x": c0.x + 13.0, "y": c0.y + 5.0, "px": c0.x + 3.0, "py": c0.y }));
        let (c2, _) = part_screen_box(&app.character, &app.character.parts[&grabbed]).unwrap();
        assert!((c2 - c - Vec2::new(10.0, 5.0)).length() < 1e-3, "{c} -> {c2}");
        assert_eq!(app.history.undo.len(), steps + 1, "жест — один шаг отмены");
    }

    #[test]
    fn editing_an_svg_part_overlays_it_and_saving_keeps_it_in_place() {
        let dir = std::env::temp_dir().join(format!("pony_studio_edit_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.svg").display().to_string();
        std::fs::write(&path, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="100 200 50 40"><rect x="110" y="210" width="30" height="20" fill="red"/></svg>"#).unwrap();
        let mut app = App::new();
        app.character.add_part(Part::new("p", PartKind::Custom, PartSource::Vector { path: path.clone() }).with_bone("Root").with_size(Vec2::new(10.0, 8.0)).with_pivot(Vec2::new(5.0, 5.0)));
        let before = scene::Affine::from_mat4(&pony_render::part_model_matrix(&app.character.parts["p"], &app.character));
        cmd(&mut app, "edit_svg", json!({ "id": "p" }));
        assert_eq!(app.edit_vb, Some([100.0, 200.0, 50.0, 40.0]));
        // угол viewBox (100, 200) — верхний левый угол квада части
        let (x, y) = app.doc_map().apply(100.0, 200.0);
        let (ex, ey) = before.apply(-0.5, 0.5);
        assert!((x - ex).abs() < 1e-4 && (y - ey).abs() < 1e-4);
        // мировая точка центра части -> центр viewBox
        let (cx, cy) = app.to_doc(Vec2::new(before.0[4], before.0[5]));
        assert!((cx - 125.0).abs() < 1e-3 && (cy - 220.0).abs() < 1e-3);
        cmd(&mut app, "save_drawing", json!({}));
        let p = &app.character.parts["p"];
        let after = scene::Affine::from_mat4(&pony_render::part_model_matrix(p, &app.character));
        // прямоугольник 110..140 x 210..230 остался на том же месте в мире
        let (wx, wy) = before.apply((125.0 - 100.0) / 50.0 - 0.5, 0.5 - (220.0 - 200.0) / 40.0);
        let (ax, ay) = (after.0[4], after.0[5]);
        assert!((wx - ax).abs() < 0.05 && (wy - ay).abs() < 0.05, "{wx},{wy} vs {ax},{ay}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn eyedropper_picks_sky_on_empty_stage_and_part_color_on_a_part() {
        let mut app = App::new();
        cmd(&mut app, "set_tool", json!({ "tool": "drop" }));
        cmd(&mut app, "stage_click", json!({ "x": 0.0, "y": 0.0 }));
        assert_eq!([app.fill.r, app.fill.g, app.fill.b], scene::SKY);
        cmd(&mut app, "blank_layer", json!({}));
        cmd(&mut app, "stage_click", json!({ "x": 0.0, "y": 0.0 }));
        let custom = scene::DrawItem { id: String::new(), asset: None, fallback: [153, 153, 153, 255], model: scene::Affine::IDENTITY, light: None, mask: None };
        assert_eq!([app.fill.r, app.fill.g, app.fill.b], [custom.fallback[0]; 3]);
    }

    #[test]
    fn export_gif_without_gpu_writes_a_file() {
        let mut app = App::new();
        cmd(&mut app, "demo", json!({}));
        let path = std::env::temp_dir().join(format!("pony_studio_{}.gif", std::process::id()));
        let r = cmd(&mut app, "export", json!({ "kind": "gif", "path": path.display().to_string() }));
        assert!(r.get("error").is_none(), "{r}");
        assert!(cmd(&mut app, "export", json!({ "kind": "gif", "path": "x.gif" })).get("error").is_some(), "второй экспорт, пока идёт первый, — ошибка");
        let jobs = app.take_jobs();
        assert_eq!(jobs.len(), 1);
        let done = app.job_done(jobs.into_iter().next().unwrap()());
        assert!(done["patch"]["ui"]["status"].as_str().unwrap().starts_with("GIF сохранён"), "{done}");
        assert!(std::fs::metadata(&path).unwrap().len() > 1000);
        let _ = std::fs::remove_file(path);
    }
}
