//! Освещение (раздел 12 ТЗ): Sun/Point/Ambient/Glow/Shadow — "даже несмотря
//! на 2D". Честное ограничение сразу: части персонажа — плоские текстурные
//! квады без нормалей, поэтому это НЕ полноценное освещение с диффузным
//! отражением по нормали, а более простая (и распространённая в 2D-движках)
//! модель — **на весь квад одна и та же итоговая яркость/цвет**, посчитанная
//! по позиции его центра (не per-pixel). Реализованы Ambient/Sun/Point.
//! **Glow и Shadow НЕ реализованы** — глоу потребовал бы отдельного
//! bloom-прохода поверх кадра, тени — трассировку от источников к другим
//! частям, ни того ни другого здесь нет.

use glam::Vec2;

#[derive(Debug, Clone, Copy)]
pub struct AmbientLight {
    pub color: [f32; 3],
    pub intensity: f32,
}

impl Default for AmbientLight {
    /// Белый свет интенсивностью 1.0 — рендер без изменений по сравнению
    /// с "без освещения вообще" (полная проверка на регрессию: старые
    /// демо/тесты, которые передают `Lighting::default()`, должны получать
    /// точно такие же пиксели, что и до появления этого модуля).
    fn default() -> Self {
        Self { color: [1.0, 1.0, 1.0], intensity: 1.0 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SunLight {
    pub color: [f32; 3],
    pub intensity: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct PointLight {
    pub position: Vec2,
    pub color: [f32; 3],
    pub intensity: f32,
    /// За пределами радиуса вклад точечного света — точно ноль (не
    /// бесконечно убывающий 1/r², чтобы результат было легко тестировать
    /// и не иметь дальних "хвостов" от каждого источника на всю сцену).
    pub radius: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Lighting {
    pub ambient: AmbientLight,
    pub sun: Option<SunLight>,
    pub points: Vec<PointLight>,
}

/// Итоговый цветовой множитель (RGB, без альфы) в точке `position` —
/// то, на что должен домножиться цвет текстуры части в этой точке.
/// Не клампится сверху (сцена может быть "пересвечена", как в реальности) —
/// клампинг, если нужен, делает потребитель (например, GPU при записи в
/// 8-битный цвет и так же обрежет).
pub fn shade_at(position: Vec2, lighting: &Lighting) -> [f32; 3] {
    let mut result = [
        lighting.ambient.color[0] * lighting.ambient.intensity,
        lighting.ambient.color[1] * lighting.ambient.intensity,
        lighting.ambient.color[2] * lighting.ambient.intensity,
    ];

    if let Some(sun) = &lighting.sun {
        // Солнце — направленный источник "из бесконечности": в отсутствие
        // нормалей у плоских квадов у него нет диффузного затухания по
        // углу, поэтому вклад одинаков в любой точке сцены (как ambient,
        // только отдельным цветом/интенсивностью — например, тёплый
        // "дневной" оттенок поверх нейтрального ambient).
        result[0] += sun.color[0] * sun.intensity;
        result[1] += sun.color[1] * sun.intensity;
        result[2] += sun.color[2] * sun.intensity;
    }

    for point in &lighting.points {
        let dist = (position - point.position).length();
        if dist >= point.radius {
            continue;
        }
        // Квадратичное затухание до нуля на границе радиуса — не 1/r²
        // (которое никогда не доходит до нуля и потребовало бы искусственного
        // обрезания), а плавное (1 - d/r)² — ноль ровно на границе, без разрыва.
        let falloff = (1.0 - dist / point.radius).powi(2);
        let strength = point.intensity * falloff;
        result[0] += point.color[0] * strength;
        result[1] += point.color[1] * strength;
        result[2] += point.color[2] * strength;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_lighting_is_neutral_full_brightness() {
        let lighting = Lighting::default();
        let color = shade_at(Vec2::new(123.0, -45.0), &lighting);
        assert_eq!(color, [1.0, 1.0, 1.0], "default должен быть нейтральным — без изменений рендера");
    }

    #[test]
    fn ambient_scales_uniformly_everywhere() {
        let lighting = Lighting { ambient: AmbientLight { color: [1.0, 0.5, 0.2], intensity: 0.5 }, sun: None, points: vec![] };
        let a = shade_at(Vec2::new(0.0, 0.0), &lighting);
        let b = shade_at(Vec2::new(999.0, -999.0), &lighting);
        assert_eq!(a, b, "ambient не должен зависеть от позиции");
        assert!((a[0] - 0.5).abs() < 1e-4);
        assert!((a[1] - 0.25).abs() < 1e-4);
    }

    #[test]
    fn sun_adds_flat_contribution_regardless_of_position() {
        let lighting = Lighting {
            ambient: AmbientLight { color: [0.2, 0.2, 0.2], intensity: 1.0 },
            sun: Some(SunLight { color: [1.0, 0.9, 0.7], intensity: 0.3 }),
            points: vec![],
        };
        let near = shade_at(Vec2::new(0.0, 0.0), &lighting);
        let far = shade_at(Vec2::new(500.0, 500.0), &lighting);
        assert_eq!(near, far, "солнце (без нормалей) должно давать одинаковый вклад везде");
        assert!((near[0] - (0.2 + 0.3)).abs() < 1e-4);
    }

    #[test]
    fn point_light_falls_off_with_distance_and_vanishes_at_radius() {
        let lighting = Lighting {
            ambient: AmbientLight { color: [0.0, 0.0, 0.0], intensity: 0.0 },
            sun: None,
            points: vec![PointLight { position: Vec2::ZERO, color: [1.0, 1.0, 1.0], intensity: 1.0, radius: 100.0 }],
        };
        let at_center = shade_at(Vec2::ZERO, &lighting);
        let at_half_radius = shade_at(Vec2::new(50.0, 0.0), &lighting);
        let at_radius = shade_at(Vec2::new(100.0, 0.0), &lighting);
        let beyond_radius = shade_at(Vec2::new(150.0, 0.0), &lighting);

        assert!(at_center[0] > at_half_radius[0], "ближе к источнику должно быть ярче");
        assert!(at_half_radius[0] > 0.0);
        assert_eq!(at_radius, [0.0, 0.0, 0.0], "ровно на границе радиуса — уже ноль");
        assert_eq!(beyond_radius, [0.0, 0.0, 0.0], "за радиусом — ноль, без бесконечного хвоста");
    }

    #[test]
    fn multiple_point_lights_sum_contributions() {
        let lighting = Lighting {
            ambient: AmbientLight { color: [0.0, 0.0, 0.0], intensity: 0.0 },
            sun: None,
            points: vec![
                PointLight { position: Vec2::new(-10.0, 0.0), color: [1.0, 0.0, 0.0], intensity: 1.0, radius: 50.0 },
                PointLight { position: Vec2::new(10.0, 0.0), color: [0.0, 1.0, 0.0], intensity: 1.0, radius: 50.0 },
            ],
        };
        let midpoint = shade_at(Vec2::ZERO, &lighting);
        assert!(midpoint[0] > 0.0 && midpoint[1] > 0.0, "в точке между двумя источниками должен быть вклад от обоих: {midpoint:?}");
    }
}
