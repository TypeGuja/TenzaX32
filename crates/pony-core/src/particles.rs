//! Частицы (раздел 13 ТЗ): Dust/Snow/Rain/Spark/Magic/Smoke/Cloud.
//!
//! Упрощение, честно: частица — не текстурированный спрайт с альфа-затуханием,
//! а плоский цветной квад, который со временем СЖИМАЕТСЯ до нуля (см.
//! `Particle::size_factor`), а не тает прозрачностью. Так рендер переиспользует
//! уже готовый `Renderer`/пайплайн частей (та же текстура-заглушка нужного
//! цвета из `TextureCache`) без отдельного альфа-блендинга по частицам.
//! Симуляция (эта часть) — чистая, без GPU, полностью протестирована.

use glam::Vec2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticleKind {
    Dust,
    Snow,
    Rain,
    Spark,
    Magic,
    Smoke,
    Cloud,
}

impl ParticleKind {
    pub fn base_color(self) -> [f32; 4] {
        match self {
            ParticleKind::Dust => [0.7, 0.65, 0.55, 1.0],
            ParticleKind::Snow => [0.95, 0.95, 1.0, 1.0],
            ParticleKind::Rain => [0.6, 0.7, 0.9, 1.0],
            ParticleKind::Spark => [1.0, 0.85, 0.3, 1.0],
            ParticleKind::Magic => [0.7, 0.4, 0.9, 1.0],
            ParticleKind::Smoke => [0.5, 0.5, 0.5, 1.0],
            ParticleKind::Cloud => [0.9, 0.9, 0.92, 1.0],
        }
    }

    /// Гравитация по умолчанию, в единицах/с². Отрицательный Y — вниз (тот
    /// же знак, что и везде в движке, см. например анимацию покачивания
    /// головы в pony-cli, где "вниз" — тоже отрицательный Y).
    /// Rain/Dust падают, Smoke/Magic поднимаются, Snow/Cloud почти невесомы.
    pub fn default_gravity(self) -> Vec2 {
        match self {
            ParticleKind::Rain => Vec2::new(0.0, -220.0),
            ParticleKind::Dust => Vec2::new(0.0, -20.0),
            ParticleKind::Spark => Vec2::new(0.0, -60.0),
            ParticleKind::Snow => Vec2::new(0.0, -15.0),
            ParticleKind::Cloud => Vec2::new(0.0, 0.0),
            ParticleKind::Smoke => Vec2::new(0.0, 25.0),
            ParticleKind::Magic => Vec2::new(0.0, 10.0),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Particle {
    pub position: Vec2,
    pub velocity: Vec2,
    pub age: f32,
    pub lifetime: f32,
    pub base_size: f32,
}

impl Particle {
    pub fn is_alive(&self) -> bool {
        self.age < self.lifetime
    }

    /// 1.0 в начале жизни, 0.0 в конце (и не меньше) — линейное сжатие.
    pub fn size_factor(&self) -> f32 {
        (1.0 - (self.age / self.lifetime.max(1e-6))).clamp(0.0, 1.0)
    }

    pub fn current_size(&self) -> f32 {
        self.base_size * self.size_factor()
    }
}

/// Эмиттер: копит время (`spawn_accum`) и рождает целое число частиц за
/// тик по `rate` (частиц/сек) — не рандомное количество, а детерминированный
/// накопитель, поэтому `update(dt)` с одним и тем же `dt` всегда рождает
/// одно и то же число частиц (важно для тестов и воспроизводимости).
#[derive(Debug, Clone)]
pub struct ParticleEmitter {
    pub kind: ParticleKind,
    pub position: Vec2,
    pub rate: f32,
    pub spread: f32,
    pub base_speed: f32,
    pub lifetime: f32,
    pub base_size: f32,
    pub gravity: Vec2,
    pub particles: Vec<Particle>,
    spawn_accum: f32,
    /// Простой LCG вместо внешней rand-зависимости — детерминированный
    /// (важно для тестов) и достаточно "случайный на вид" для разброса
    /// начальной скорости частиц.
    rng_state: u32,
}

impl ParticleEmitter {
    pub fn new(kind: ParticleKind, position: Vec2, rate: f32) -> Self {
        Self {
            kind,
            position,
            rate,
            spread: 20.0,
            base_speed: 30.0,
            lifetime: 1.5,
            base_size: 4.0,
            gravity: kind.default_gravity(),
            particles: Vec::new(),
            spawn_accum: 0.0,
            rng_state: 0x9E3779B9,
        }
    }

    fn next_rand01(&mut self) -> f32 {
        self.rng_state = self.rng_state.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.rng_state >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Продвинуть симуляцию на `dt` секунд: родить новые частицы (сколько
    /// набежало по `rate`), сдвинуть существующие по гравитации, убрать
    /// умершие (`age >= lifetime`).
    pub fn update(&mut self, dt: f32) {
        self.spawn_accum += self.rate * dt;
        while self.spawn_accum >= 1.0 {
            self.spawn_accum -= 1.0;
            let jitter_x = (self.next_rand01() - 0.5) * 2.0 * self.spread;
            let initial_vy = self.base_speed * self.gravity.y.signum() * 0.2;
            self.particles.push(Particle {
                position: self.position,
                velocity: Vec2::new(jitter_x, initial_vy),
                age: 0.0,
                lifetime: self.lifetime,
                base_size: self.base_size,
            });
        }
        for p in &mut self.particles {
            p.velocity += self.gravity * dt;
            p.position += p.velocity * dt;
            p.age += dt;
        }
        self.particles.retain(|p| p.is_alive());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawns_particles_proportional_to_rate_and_time() {
        let mut emitter = ParticleEmitter::new(ParticleKind::Dust, Vec2::ZERO, 10.0);
        emitter.update(1.0); // 10 частиц/с * 1с = ровно 10
        assert_eq!(emitter.particles.len(), 10);
    }

    #[test]
    fn spawn_accumulator_carries_over_across_small_steps() {
        let mut emitter = ParticleEmitter::new(ParticleKind::Dust, Vec2::ZERO, 10.0);
        // 20 шагов по 0.05с = 1.0с суммарно, тот же результат, что один
        // большой шаг — накопитель не теряет и не удваивает дробные части.
        for _ in 0..20 {
            emitter.update(0.05);
        }
        assert_eq!(emitter.particles.len(), 10, "должно быть столько же, сколько за один шаг в 1.0с");
    }

    #[test]
    fn particles_die_after_lifetime() {
        let mut emitter = ParticleEmitter::new(ParticleKind::Spark, Vec2::ZERO, 20.0);
        emitter.lifetime = 0.5;
        emitter.update(0.1); // rate=20/с * 0.1с = 2 частицы родится
        assert!(!emitter.particles.is_empty());
        emitter.update(1.0); // намного больше lifetime — все должны умереть
        assert!(emitter.particles.is_empty(), "частицы должны исчезнуть после lifetime");
    }

    #[test]
    fn rain_falls_downward() {
        let mut emitter = ParticleEmitter::new(ParticleKind::Rain, Vec2::ZERO, 1.0);
        emitter.spread = 0.0; // без горизонтального разброса, чтобы проверять чисто Y
        emitter.update(1.0); // рождает 1 частицу и сразу продвигает на 1с
        let p = emitter.particles[0];
        assert!(p.position.y < 0.0, "дождь должен упасть вниз (отрицательный Y), y={}", p.position.y);
    }

    #[test]
    fn smoke_rises_upward() {
        let mut emitter = ParticleEmitter::new(ParticleKind::Smoke, Vec2::ZERO, 1.0);
        emitter.spread = 0.0;
        emitter.update(1.0);
        let p = emitter.particles[0];
        assert!(p.position.y > 0.0, "дым должен подниматься вверх (положительный Y), y={}", p.position.y);
    }

    #[test]
    fn size_factor_shrinks_from_one_to_zero_over_lifetime() {
        let mut particle = Particle { position: Vec2::ZERO, velocity: Vec2::ZERO, age: 0.0, lifetime: 2.0, base_size: 10.0 };
        assert!((particle.size_factor() - 1.0).abs() < 1e-4);
        particle.age = 1.0;
        assert!((particle.size_factor() - 0.5).abs() < 1e-4);
        particle.age = 2.0;
        assert!((particle.size_factor() - 0.0).abs() < 1e-4);
    }
}
