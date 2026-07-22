//! Морфинг (раздел 7 ТЗ): вместо рисования десятков кадров лица —
//! именованные веса выражений + непрерывные параметры глаз.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Именованный пресет выражения (Smile, Sad, Open, Wide, Blink, Angry, Fear, Sleep, ...).
/// Пользователь может добавлять свои — поэтому не enum, а строка.
pub type MorphName = String;

/// Непрерывные параметры глаз, которых нет смысла делать булевыми выражениями.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EyeParams {
    pub radius: f32,
    pub height: f32,
    pub pupil: f32,
    pub rotation: f32,
}

impl Default for EyeParams {
    fn default() -> Self {
        Self {
            radius: 1.0,
            height: 1.0,
            pupil: 1.0,
            rotation: 0.0,
        }
    }
}

/// Текущее состояние морфинга персонажа: вес каждого именованного
/// выражения (0..1, смешиваются) + параметры глаз.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MorphState {
    pub weights: HashMap<MorphName, f32>,
    pub eyes: EyeParams,
}

impl MorphState {
    pub fn set(&mut self, name: impl Into<String>, weight: f32) {
        self.weights.insert(name.into(), weight.clamp(0.0, 1.0));
    }

    pub fn get(&self, name: &str) -> f32 {
        self.weights.get(name).copied().unwrap_or(0.0)
    }
}

/// Список стандартных выражений из ТЗ — используется как подсказка
/// в редакторе, не ограничивает пользователя.
pub const STANDARD_EXPRESSIONS: &[&str] = &[
    "Smile", "Sad", "Open", "Wide", "Blink", "Angry", "Fear", "Sleep",
];
