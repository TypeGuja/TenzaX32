//! Части тела (раздел 5 ТЗ): Head, Eyes, Mouth, Ear, Wing, Tail, Horn, Body, ноги.
//! Каждая часть — отдельный слой (вектор или PNG), с pivot-точкой и
//! привязкой к кости скелета.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::skeleton::BoneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartKind {
    Body,
    Head,
    ManeFront,
    ManeBack,
    Tail,
    Eyes,
    Mouth,
    Ear,
    Wing,
    Horn,
    LegFL,
    LegFR,
    LegBL,
    LegBR,
    Custom,
}

/// Источник изображения части — растр или вектор.
/// Оба поддерживаются, т.к. ТЗ (раздел 16) требует импорт PNG/SVG/PSD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PartSource {
    Png { path: String },
    Vector { path: String },
    /// PSD (раздел 16 ТЗ). `layer` — имя конкретного слоя (`None` — взять
    /// сведённое изображение всех видимых слоёв, `psd.rgba()`).
    Psd { path: String, layer: Option<String> },
    /// KRA (раздел 16 ТЗ, формат Krita — по сути zip-архив). `layer_file` —
    /// имя конкретного PNG-файла внутри архива (обычно `layers/<имя>.png`),
    /// `None` — взять сведённый превью-слой `mergedimage.png`, который
    /// Krita всегда пишет в корень архива.
    Kra { path: String, layer_file: Option<String> },
    /// Меш без текстуры — для случаев, где форма важнее заливки цветом.
    Mesh { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Part {
    pub id: String,
    pub kind: PartKind,
    pub source: PartSource,
    /// Смещение части относительно кости, к которой она прикреплена
    /// (в единицах сцены; поворачивается и масштабируется вместе с костью).
    ///
    /// Раньше это поле существовало, но рендер его игнорировал — из-за чего
    /// все части на одной кости рисовались ровно друг на друге и часть
    /// нельзя было сдвинуть, не двигая кость вместе со всем остальным, что
    /// к ней прикреплено. Теперь это настоящее смещение (см. `render_character`).
    pub pivot: Vec2,
    /// Явный размер части на сцене. `None` — размер по умолчанию для её
    /// `PartKind` (см. `nominal_part_size` в pony-render). Нужен, чтобы
    /// нарисованная или импортированная часть отображалась того размера,
    /// какого её сделали, а не подгонялась под жёсткую таблицу видов.
    #[serde(default)]
    pub size: Option<Vec2>,
    /// Порядок отрисовки (выше — поверх).
    pub layer: i32,
    /// К какой кости прикреплена часть.
    pub bone: Option<BoneId>,
}

impl Part {
    pub fn new(id: impl Into<String>, kind: PartKind, source: PartSource) -> Self {
        Self {
            id: id.into(),
            kind,
            source,
            pivot: Vec2::ZERO,
            size: None,
            layer: 0,
            bone: None,
        }
    }

    pub fn with_bone(mut self, bone: impl Into<String>) -> Self {
        self.bone = Some(bone.into());
        self
    }

    pub fn with_layer(mut self, layer: i32) -> Self {
        self.layer = layer;
        self
    }

    pub fn with_pivot(mut self, pivot: Vec2) -> Self {
        self.pivot = pivot;
        self
    }

    pub fn with_size(mut self, size: Vec2) -> Self {
        self.size = Some(size);
        self
    }
}
