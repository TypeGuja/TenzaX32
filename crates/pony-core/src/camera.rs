//! Камера (раздел 11 ТЗ): Move/Rotate/Zoom/Depth/Shake/Blur.
//! Теперь реально влияет на рендер — см. `Renderer::render_character` в
//! pony-render, который берёт `position`/`rotation`/`zoom`/`shake_offset()`
//! и строит из них view-матрицу поверх обычной ортографической проекции.
//! Depth/Blur всё ещё только состояние — пост-эффекты (глубина резкости,
//! блюр) не реализованы, рендер их не читает.

use glam::Vec2;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Camera {
    pub position: Vec2,
    pub rotation: f32,
    pub zoom: f32,
    /// Условная "сила тряски" — накапливается вызовами `shake()`. Сама
    /// камера не хранит время и не решает, как быстро это должно затухать —
    /// затухание (например, `shake_intensity *= 0.9` за кадр) остаётся на
    /// вызывающей стороне (см. `pony-gui`, который делает это в игровом
    /// цикле). `shake_offset()` — чистая функция, детерминированная по `t`.
    pub shake_intensity: f32,
    pub depth: f32,
    pub blur: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec2::ZERO,
            rotation: 0.0,
            zoom: 1.0,
            shake_intensity: 0.0,
            depth: 0.0,
            blur: 0.0,
        }
    }
}

impl Camera {
    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.position += Vec2::new(dx, dy);
    }

    pub fn rotate_by(&mut self, radians: f32) {
        self.rotation += radians;
    }

    /// Множитель, а не абсолютное значение — `zoom(2.0)` удваивает текущий.
    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).max(0.0001);
    }

    pub fn shake(&mut self, intensity: f32) {
        self.shake_intensity += intensity;
    }

    /// Смещение камеры от тряски в момент времени `t` (секунды, любой
    /// монотонно растущий счётчик подойдёт). Две независимые частоты по осям
    /// (37 и 53 — взаимно простые, не кратные), чтобы траектория не была
    /// вырожденным эллипсом, а выглядела как дрожание. Детерминированная
    /// функция — не PRNG, специально: одинаковый `t` даёт одинаковый сдвиг
    /// (полезно для тестов и воспроизводимости), а не только "выглядит похоже".
    pub fn shake_offset(&self, t: f32) -> Vec2 {
        if self.shake_intensity <= 0.0 {
            return Vec2::ZERO;
        }
        Vec2::new((t * 37.0).sin(), (t * 53.0).cos()) * self.shake_intensity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_shake_gives_zero_offset() {
        let cam = Camera::default();
        assert_eq!(cam.shake_offset(1.23), Vec2::ZERO);
    }

    #[test]
    fn shake_offset_scales_with_intensity() {
        let mut cam = Camera::default();
        cam.shake(2.0);
        let offset = cam.shake_offset(0.5);
        assert!(offset.length() > 0.0);
        // Тот же t даёт тот же сдвиг — детерминированность, не рандом.
        assert_eq!(offset, cam.shake_offset(0.5));
        // Вдвое больше интенсивность -> вдвое больше сдвиг (при том же t).
        let mut cam2 = Camera::default();
        cam2.shake(4.0);
        let offset2 = cam2.shake_offset(0.5);
        assert!((offset2.length() - offset.length() * 2.0).abs() < 1e-4);
    }

    #[test]
    fn zoom_by_is_multiplicative() {
        let mut cam = Camera::default();
        cam.zoom_by(2.0);
        cam.zoom_by(3.0);
        assert!((cam.zoom - 6.0).abs() < 1e-4);
    }
}

