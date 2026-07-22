//! Персонаж (раздел 4 ТЗ): Name, Version, Parts, Skeleton, Morphs,
//! Animations, Physics, Metadata — всё, что раньше было бы пятью тысячами
//! PNG, теперь одно описание.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::animation::Animation;
use crate::morph::MorphState;
use crate::part::Part;
use crate::skeleton::Skeleton;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub author: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PhysicsConfig {
    /// Кости, которые качаются пассивно (грива, хвост) — по имени кости
    /// и коэффициенту "мягкости" (0 = жёсткая, 1 = максимально свободная).
    pub soft_bones: HashMap<String, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    pub name: String,
    pub version: String,
    pub parts: HashMap<String, Part>,
    pub skeleton: Skeleton,
    /// Морфинг хранится как дефолтное состояние; во время анимации
    /// поверх него применяются дорожки типа AnimTarget::Morph.
    pub default_morph: MorphState,
    pub animations: HashMap<String, Animation>,
    pub physics: PhysicsConfig,
    pub metadata: Metadata,
}

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ron serialize error: {0}")]
    RonSer(#[from] ron::Error),
    #[error("ron deserialize error: {0}")]
    RonDe(#[from] ron::error::SpannedError),
}

impl Character {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: "0.1.0".into(),
            parts: HashMap::new(),
            skeleton: Skeleton::new(),
            default_morph: MorphState::default(),
            animations: HashMap::new(),
            physics: PhysicsConfig::default(),
            metadata: Metadata::default(),
        }
    }

    pub fn add_part(&mut self, part: Part) -> &mut Self {
        self.parts.insert(part.id.clone(), part);
        self
    }

    pub fn add_animation(&mut self, anim: Animation) -> &mut Self {
        self.animations.insert(anim.name.clone(), anim);
        self
    }

    /// Сохранить как `name.asset` (человекочитаемый RON).
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), AssetError> {
        let pretty = ron::ser::PrettyConfig::new().depth_limit(8);
        let s = ron::ser::to_string_pretty(self, pretty)?;
        std::fs::write(path, s)?;
        Ok(())
    }

    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, AssetError> {
        let s = std::fs::read_to_string(path)?;
        let character: Character = ron::from_str(&s)?;
        Ok(character)
    }
}
