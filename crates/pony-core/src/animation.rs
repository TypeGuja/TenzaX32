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
    pub fn sample(&self, t: f32) -> Option<f32> {
        let n = self.keyframes.len();
        if n == 0 {
            return None;
        }
        if n == 1 || t <= self.keyframes[0].time {
            return Some(as_f32(&self.keyframes[0].value));
        }
        if t >= self.keyframes[n - 1].time {
            return Some(as_f32(&self.keyframes[n - 1].value));
        }

        for i in 0..n - 1 {
            let a = &self.keyframes[i];
            let b = &self.keyframes[i + 1];
            if t < a.time || t > b.time {
                continue;
            }
            let span = (b.time - a.time).max(f32::EPSILON);
            let local_t = (t - a.time) / span;
            let va = as_f32(&a.value);
            let vb = as_f32(&b.value);

            return Some(match a.interpolation {
                Interpolation::Step => va,
                Interpolation::Linear => va + (vb - va) * local_t,
                Interpolation::Bezier { c1, c2 } => eval_bezier(va, vb, c1, c2, local_t),
                Interpolation::Hermite => {
                    let m0 = neighbor_tangent(self, i);
                    let m1 = neighbor_tangent(self, i + 1);
                    eval_hermite(va, vb, m0, m1, local_t)
                }
                Interpolation::CatmullRom => {
                    let p_prev = if i > 0 {
                        as_f32(&self.keyframes[i - 1].value)
                    } else {
                        va - (vb - va) // зеркалим за пределами массива, чтобы не ломать касательную на границе
                    };
                    let p_next = if i + 2 < n {
                        as_f32(&self.keyframes[i + 2].value)
                    } else {
                        vb + (vb - va)
                    };
                    eval_catmull_rom(p_prev, va, vb, p_next, local_t)
                }
            });
        }
        Some(as_f32(&self.keyframes[n - 1].value))
    }
}

/// Кубическая интерполяция Эрмита между двумя точками с явными касательными.
/// `m0`/`m1` — касательные (скорость изменения) в начале и конце сегмента,
/// уже отнормированные под длительность сегмента (0..1 по `t`).
fn eval_hermite(p0: f32, p1: f32, m0: f32, m1: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    (2.0 * t3 - 3.0 * t2 + 1.0) * p0
        + (t3 - 2.0 * t2 + t) * m0
        + (-2.0 * t3 + 3.0 * t2) * p1
        + (t3 - t2) * m1
}

/// Catmull-Rom — это Hermite с касательными, вычисленными автоматически
/// по соседним точкам (центральная разность), а не заданными вручную.
fn eval_catmull_rom(p_prev: f32, p0: f32, p1: f32, p_next: f32, t: f32) -> f32 {
    let m0 = (p1 - p_prev) * 0.5;
    let m1 = (p_next - p0) * 0.5;
    eval_hermite(p0, p1, m0, m1, t)
}

/// Касательная в ключе `idx` дорожки по соседним ключам — то же правило,
/// что и в Catmull-Rom, используется для Hermite-сегментов, когда явных
/// входящих/исходящих касательных не задано. На границах массива —
/// односторонняя разность вместо центральной.
fn neighbor_tangent(track: &Track, idx: usize) -> f32 {
    let n = track.keyframes.len();
    let v = |k: usize| as_f32(&track.keyframes[k].value);
    if n < 2 {
        0.0
    } else if idx == 0 {
        v(1) - v(0)
    } else if idx + 1 >= n {
        v(n - 1) - v(n - 2)
    } else {
        (v(idx + 1) - v(idx - 1)) * 0.5
    }
}

/// Кубическая парам. Bezier-кривая в плоскости (время, значение), где
/// `c1`/`c2` — координаты контрольных точек в нормализованном пространстве
/// сегмента: `.0` — доля времени (0..1 внутри сегмента), `.1` — значение
/// в той же шкале, что и `va`/`vb` (не смещение, а абсолютная величина).
/// Поскольку Bezier не является функцией t напрямую, сперва решаем
/// x(u) = local_t методом Ньютона (с откатом на клэмп), затем берём y(u).
/// Корректно для типичных монотонных по времени handle'ов (как в
/// большинстве редакторов кривых); экзотические самопересекающиеся
/// кривые не поддерживаются — они и не нужны для easing-кривых анимации.
fn eval_bezier(va: f32, vb: f32, c1: (f32, f32), c2: (f32, f32), local_t: f32) -> f32 {
    let x = |u: f32| {
        let mu = 1.0 - u;
        3.0 * mu * mu * u * c1.0 + 3.0 * mu * u * u * c2.0 + u * u * u
    };
    let dx = |u: f32| {
        let mu = 1.0 - u;
        3.0 * mu * mu * c1.0 + 6.0 * mu * u * (c2.0 - c1.0) + 3.0 * u * u * (1.0 - c2.0)
    };

    let mut u = local_t; // хорошее начальное приближение для монотонных кривых
    for _ in 0..8 {
        let fx = x(u) - local_t;
        if fx.abs() < 1e-5 {
            break;
        }
        let dfx = dx(u);
        if dfx.abs() < 1e-6 {
            break;
        }
        let next = (u - fx / dfx).clamp(0.0, 1.0);
        u = next;
    }

    let mu = 1.0 - u;
    mu * mu * mu * va + 3.0 * mu * mu * u * c1.1 + 3.0 * mu * u * u * c2.1 + u * u * u * vb
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

#[cfg(test)]
mod tests {
    use super::*;

    fn track_with(interp: Interpolation, values: &[(f32, f32)]) -> Track {
        Track {
            target: AnimTarget::Morph { name: "test".into() },
            keyframes: values
                .iter()
                .map(|(time, value)| Keyframe {
                    time: *time,
                    value: AnimValue::Float(*value),
                    interpolation: interp,
                })
                .collect(),
        }
    }

    #[test]
    fn linear_matches_endpoints_and_midpoint() {
        let track = track_with(Interpolation::Linear, &[(0.0, 0.0), (1.0, 10.0)]);
        assert_eq!(track.sample(0.0), Some(0.0));
        assert_eq!(track.sample(1.0), Some(10.0));
        assert_eq!(track.sample(0.5), Some(5.0));
    }

    #[test]
    fn step_holds_start_value_until_next_key() {
        let track = track_with(Interpolation::Step, &[(0.0, 1.0), (1.0, 9.0)]);
        assert_eq!(track.sample(0.0), Some(1.0));
        assert_eq!(track.sample(0.99), Some(1.0));
    }

    #[test]
    fn bezier_hits_exact_endpoints() {
        let track = Track {
            target: AnimTarget::Morph { name: "test".into() },
            keyframes: vec![
                Keyframe {
                    time: 0.0,
                    value: AnimValue::Float(0.0),
                    interpolation: Interpolation::Bezier { c1: (0.25, 0.1), c2: (0.75, 0.9) },
                },
                Keyframe { time: 1.0, value: AnimValue::Float(1.0), interpolation: Interpolation::Linear },
            ],
        };
        let start = track.sample(0.0).unwrap();
        let end = track.sample(1.0).unwrap();
        assert!((start - 0.0).abs() < 1e-4, "start={start}");
        assert!((end - 1.0).abs() < 1e-4, "end={end}");
    }

    #[test]
    fn bezier_ease_in_out_is_slower_at_edges_than_linear() {
        // Классический ease-in-out: у краёв должно быть МЕДЛЕННЕЕ, чем
        // линейная интерполяция (значение ближе к предыдущему ключу).
        let track = Track {
            target: AnimTarget::Morph { name: "test".into() },
            keyframes: vec![
                Keyframe {
                    time: 0.0,
                    value: AnimValue::Float(0.0),
                    interpolation: Interpolation::Bezier { c1: (0.42, 0.0), c2: (0.58, 1.0) },
                },
                Keyframe { time: 1.0, value: AnimValue::Float(1.0), interpolation: Interpolation::Linear },
            ],
        };
        let near_start = track.sample(0.1).unwrap();
        assert!(near_start < 0.1, "ease-in-out should lag linear near start, got {near_start}");
    }

    #[test]
    fn catmull_rom_passes_through_keyframes() {
        let track = track_with(
            Interpolation::CatmullRom,
            &[(0.0, 0.0), (1.0, 5.0), (2.0, 2.0), (3.0, 8.0)],
        );
        // Должна проходить точно через каждый заданный ключ.
        assert!((track.sample(0.0).unwrap() - 0.0).abs() < 1e-4);
        assert!((track.sample(1.0).unwrap() - 5.0).abs() < 1e-4);
        assert!((track.sample(2.0).unwrap() - 2.0).abs() < 1e-4);
        assert!((track.sample(3.0).unwrap() - 8.0).abs() < 1e-4);
    }

    #[test]
    fn hermite_passes_through_keyframes() {
        let track = track_with(Interpolation::Hermite, &[(0.0, 0.0), (1.0, 4.0), (2.0, -2.0)]);
        assert!((track.sample(0.0).unwrap() - 0.0).abs() < 1e-4);
        assert!((track.sample(1.0).unwrap() - 4.0).abs() < 1e-4);
        assert!((track.sample(2.0).unwrap() - (-2.0)).abs() < 1e-4);
    }

    #[test]
    fn sample_before_first_and_after_last_clamps() {
        let track = track_with(Interpolation::Linear, &[(1.0, 100.0), (2.0, 200.0)]);
        assert_eq!(track.sample(0.0), Some(100.0));
        assert_eq!(track.sample(5.0), Some(200.0));
    }
}
