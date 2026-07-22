//! Анимация (разделы 9-10 ТЗ): ключевые кадры + интерполяция
//! (Bezier / Hermite / Catmull-Rom), таймлайн из дорожек (кости, морфы, камера).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Interpolation {
    Linear,
    Bezier { c1: (f32, f32), c2: (f32, f32) },
    Hermite,
    CatmullRom,
    Step,
}

/// Значение, которое анимируется. Расширяемо под трансформы, морфы,
/// параметры камеры/света/частиц.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnimValue {
    Float(f32),
    Vec2(f32, f32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyframe {
    /// Время в секундах от начала анимации.
    pub time: f32,
    pub value: AnimValue,
    /// Интерполяция ОТ этого ключа К следующему.
    pub interpolation: Interpolation,
}

/// Что именно анимирует дорожка.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnimTarget {
    Bone { id: String, channel: BoneChannel },
    Morph { name: String },
    EyeParam { channel: String },
    Camera { channel: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BoneChannel {
    PositionX,
    PositionY,
    Rotation,
    ScaleX,
    ScaleY,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub target: AnimTarget,
    pub keyframes: Vec<Keyframe>,
}

impl Track {
    /// Вычислить значение дорожки в момент времени `t`.
    /// Линейная и step интерполяция реализованы полностью;
    /// Bezier/Hermite/CatmullRom — точки уже хранятся, оценка кривой
    /// будет достроена вместе с рендером (нужны соседние ключи).
    pub fn sample(&self, t: f32) -> Option<f32> {
        if self.keyframes.is_empty() {
            return None;
        }
        if t <= self.keyframes[0].time {
            return Some(as_f32(&self.keyframes[0].value));
        }
        for pair in self.keyframes.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if t >= a.time && t <= b.time {
                let span = (b.time - a.time).max(f32::EPSILON);
                let local_t = (t - a.time) / span;
                let (va, vb) = (as_f32(&a.value), as_f32(&b.value));
                return Some(match a.interpolation {
                    Interpolation::Step => va,
                    _ => va + (vb - va) * local_t, // TODO: Bezier/Hermite/CatmullRom
                });
            }
        }
        Some(as_f32(&self.keyframes.last().unwrap().value))
    }
}

fn as_f32(v: &AnimValue) -> f32 {
    match v {
        AnimValue::Float(f) => *f,
        AnimValue::Vec2(x, _) => *x,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Animation {
    pub name: String,
    pub duration: f32,
    pub tracks: Vec<Track>,
    /// Зациклена ли анимация (Walk, Idle) или проигрывается один раз (Blink).
    pub looping: bool,
}
