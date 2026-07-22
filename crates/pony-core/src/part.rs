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
    /// Меш без текстуры — для случаев, где форма важнее заливки цветом.
    Mesh { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Part {
    pub id: String,
    pub kind: PartKind,
    pub source: PartSource,
    /// Точка вращения относительно локального центра изображения.
    pub pivot: Vec2,
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
}
