use pony_core::animation::{AnimTarget, AnimValue, Animation, BoneChannel, Interpolation, Keyframe, Track};
use pony_core::part::{Part, PartKind, PartSource};
use pony_core::skeleton::default_pony_skeleton;
use pony_core::{Camera, Character};
use pony_render::{export_gif, export_spritesheet, RenderCluster, Renderer};
use pony_script::ScriptEngine;
use pony_system::{GpuAssignment, SystemProfile, WorkloadPolicy};
use std::io::Write;

/// Определяем систему, считаем политику ресурсов и поднимаем то, что
/// реально доступно (0 GPU -> CPU-only, N GPU -> кластер контекстов).
/// Ничего не захардкожено под конкретную машину.
fn report_system_and_build_runtime() -> (SystemProfile, WorkloadPolicy, rayon::ThreadPool, RenderCluster) {
    let profile = SystemProfile::detect();
    let policy = WorkloadPolicy::from_profile(&profile);

    println!("=== Профиль системы ===");
    println!(
        "CPU: {} логических / {} физических ядер",
        profile.cpu.logical_cores, profile.cpu.physical_cores
    );
    println!(
        "Память: {:.1} ГиБ доступно из {:.1} ГиБ всего",
        profile.memory.available_bytes as f64 / 1e9,
        profile.memory.total_bytes as f64 / 1e9
    );
    if profile.gpus.is_empty() {
        println!("GPU: не обнаружено — работаем в CPU-only режиме");
    } else {
        for gpu in &profile.gpus {
            println!(
                "GPU[{}]: {} ({}, {:?}, вес {:.2})",
                gpu.index,
                gpu.name,
                gpu.backend,
                gpu.device_type,
                gpu.device_type.relative_weight()
            );
        }
    }

    println!("\n=== Политика ресурсов (вычислена из профиля) ===");
    println!("Рабочих потоков rayon: {}", policy.worker_threads);
    println!(
        "Бюджет памяти под кэш движка: {:.1} ГиБ",
        policy.memory_budget_bytes as f64 / 1e9
    );
    match &policy.gpu_assignment {
        GpuAssignment::None => println!("GPU-назначение: нет (CPU-only рендер)"),
        GpuAssignment::Single(i) => println!("GPU-назначение: один адаптер #{i}"),
        GpuAssignment::Multi(w) => println!("GPU-назначение: несколько адаптеров, доли {w:?}"),
    }

    let thread_pool = pony_system::build_thread_pool(&policy);
    let render_cluster = pollster::block_on(RenderCluster::initialize(&profile, &policy.gpu_assignment))
        .unwrap_or_default();

    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер: пуст, реального устройства не поднято (ожидаемо без GPU-драйверов в этой среде)");
    } else {
        println!("Рендер-кластер: {} активных GPU-контекст(ов)", render_cluster.contexts.len());
    }

    (profile, policy, thread_pool, render_cluster)
}

fn main() {
    let (_profile, policy, thread_pool, render_cluster) = report_system_and_build_runtime();

    // Демонстрация того, зачем нужен пул: параллельно считаем мировые
    // трансформации костей для N "виртуальных" персонажей на сцене —
    // именно так в реальном рендере распараллелится скелетная анимация
    // множества персонажей между доступными ядрами.
    let skeleton = default_pony_skeleton();
    let scene_characters = 64usize;
    let bone_ids: Vec<&str> = skeleton.bones.iter().map(|b| b.id.as_str()).collect();

    let world_positions: Vec<usize> = thread_pool.install(|| {
        use rayon::prelude::*;
        (0..scene_characters)
            .into_par_iter()
            .map(|_char_idx| {
                // На каждого персонажа — пересчёт всех костей скелета.
                bone_ids
                    .iter()
                    .filter_map(|id| skeleton.world_transform(id))
                    .count()
            })
            .collect()
    });
    println!(
        "\nПересчитано {} костей суммарно по {} персонажам на {} rayon-потоках",
        world_positions.iter().sum::<usize>(),
        scene_characters,
        policy.worker_threads
    );

    println!();
    render_scene_demo(&render_cluster);

    println!();
    script_demo(&render_cluster);

    println!();
    gif_export_demo(&render_cluster);

    println!();
    orientation_demo(&render_cluster);

    println!();
    particles_demo(&render_cluster);

    println!();
    lighting_demo(&render_cluster);

    println!();
    psd_import_demo(&render_cluster);

    println!();
    kra_import_demo(&render_cluster);

    println!();
    memory_budget_demo(&render_cluster);

    println!();
    look_demo(&render_cluster);

    println!();
    mask_demo(&render_cluster);

    println!();
    run_asset_demo();
}

/// Строит простого пони с фиксированным набором частей, сдвинутого по X —
/// для сцены из нескольких персонажей рядом друг с другом.
fn build_demo_character(name: &str, x_offset: f32) -> Character {
    let mut character = Character::new(name);
    let mut skeleton = default_pony_skeleton();
    if let Some(root) = skeleton.bones.iter_mut().find(|b| b.id == "Root") {
        root.local_transform.position.x = x_offset;
    }
    character.skeleton = skeleton;

    character
        .add_part(
            Part::new("body", PartKind::Body, PartSource::Png { path: "assets/pony/body.png".into() })
                .with_bone("Body")
                .with_layer(0),
        )
        .add_part(
            Part::new("head", PartKind::Head, PartSource::Png { path: "assets/pony/head.png".into() })
                .with_bone("Head")
                .with_layer(1),
        )
        .add_part(
            Part::new("horn", PartKind::Horn, PartSource::Png { path: "assets/pony/horn.png".into() })
                .with_bone("Horn")
                .with_layer(2),
        )
        .add_part(
            Part::new("ear_l", PartKind::Ear, PartSource::Png { path: "assets/pony/ear.png".into() })
                .with_bone("EarL")
                .with_layer(2),
        )
        .add_part(
            Part::new("ear_r", PartKind::Ear, PartSource::Png { path: "assets/pony/ear.png".into() })
                .with_bone("EarR")
                .with_layer(2),
        )
        .add_part(
            Part::new("eye_l", PartKind::Eyes, PartSource::Png { path: "assets/pony/eye.png".into() })
                .with_bone("Head")
                .with_layer(2),
        )
        .add_part(
            Part::new("leg_fl", PartKind::LegFL, PartSource::Png { path: "assets/pony/leg.png".into() })
                .with_bone("LowerLegFL")
                .with_layer(0),
        )
        .add_part(
            Part::new("leg_fr", PartKind::LegFR, PartSource::Png { path: "assets/pony/leg.png".into() })
                .with_bone("LowerLegFR")
                .with_layer(0),
        )
        // Эти три — SVG (растеризуются через resvg), остальные выше — PNG.
        // Специально смешиваю оба источника на одном персонаже, чтобы
        // проверить, что диспетчер в Renderer корректно разводит их по
        // разным загрузчикам, а не только "какой-то один".
        .add_part(
            Part::new("wing_l", PartKind::Wing, PartSource::Vector { path: "assets/pony_svg/wing.svg".into() })
                .with_bone("Body")
                .with_layer(0),
        )
        .add_part(
            Part::new("tail", PartKind::Tail, PartSource::Vector { path: "assets/pony_svg/tail.svg".into() })
                .with_bone("Body")
                .with_layer(0),
        )
        .add_part(
            Part::new("mouth", PartKind::Mouth, PartSource::Vector { path: "assets/pony_svg/mouth.svg".into() })
                .with_bone("Head")
                .with_layer(2),
        );

    character
}

/// Настоящий render pass: раскладываем персонажей сцены по GPU-контекстам
/// через RenderCluster::pick() (взвешенный round-robin) и рендерим каждого
/// в offscreen-текстуру. С одним GPU все job'ы естественно попадут на него
/// же — это ожидаемо и проверяемо; с несколькими — пойдут пропорционально
/// весам адаптеров.
fn render_scene_demo(render_cluster: &RenderCluster) {
    println!("=== Рендер сцены через RenderCluster ===");
    if render_cluster.is_cpu_only() {
        println!("Кластер пуст (нет GPU-контекста) — рендер пропущен, нужен CPU-фоллбек рендерер.");
        return;
    }

    let characters = vec![
        build_demo_character("Pony_A", -70.0),
        build_demo_character("Pony_B", 0.0),
        build_demo_character("Pony_C", 70.0),
    ];

    // Renderer привязан к конкретному GpuContext (свой pipeline + кэш
    // текстур на своём device), поэтому держим по одному на контекст и
    // переиспользуем; кэш текстур требует &mut при первой загрузке ассета.
    let mut renderers: Vec<Renderer> = render_cluster.contexts.iter().map(Renderer::new).collect();

    for (i, character) in characters.iter().enumerate() {
        let ctx_idx = render_cluster
            .contexts
            .iter()
            .position(|c| std::ptr::eq(c, render_cluster.pick(i).unwrap()))
            .unwrap();
        let ctx = &render_cluster.contexts[ctx_idx];
        let renderer = &mut renderers[ctx_idx];

        let frame = renderer.render_character(ctx, character, 320, 240, &pony_core::Camera::default(), 0.0, &pony_core::Lighting::default(), None);
        let out_path = format!("scene_{}.ppm", character.name.to_lowercase());
        save_ppm(&out_path, frame.width, frame.height, &frame.rgba);
        println!(
            "{}: отрисован на GPU '{}' -> {}",
            character.name, frame.rendered_on, out_path
        );
    }
}

fn save_ppm(path: &str, width: u32, height: u32, rgba: &[u8]) {
    let mut file = std::fs::File::create(path).expect("failed to create ppm file");
    write!(file, "P6\n{width} {height}\n255\n").unwrap();
    for px in rgba.chunks_exact(4) {
        file.write_all(&px[0..3]).unwrap(); // отбрасываем alpha, PPM его не поддерживает
    }
}

/// Демонстрация скриптового слоя (раздел 15 ТЗ): скрипт на rhai описывает
/// намерение через `pony.*`/`camera.*`, а `apply_commands` применяет
/// получившиеся команды к реальному `Character`/`Camera`.
fn script_demo(render_cluster: &RenderCluster) {
    println!("=== Скриптовый слой (rhai) + проигрыватель анимаций ===");

    let script = r#"
        pony.Move(12.0, -3.0);
        pony.Smile(0.8);
        pony.Blink();
        pony.Walk();
        camera.Zoom(1.5);
        camera.Shake(0.2);
        camera.Move(0.0, 5.0);
    "#;

    let engine = ScriptEngine::new();
    let commands = match engine.run(script) {
        Ok(cmds) => cmds,
        Err(err) => {
            eprintln!("[pony-script] ошибка выполнения скрипта: {err}");
            return;
        }
    };
    println!("Скрипт испустил {} команд(ы)", commands.len());

    let mut character = build_demo_character("ScriptedPony", 0.0);
    // Даём персонажу реальную анимацию "Walk" — без неё pony.Walk() из
    // скрипта был бы просто предупреждением в stderr (см. apply_commands).
    // Голова покачивается по Y — тот же приём, что и Idle в run_asset_demo,
    // только под другим именем и с бОльшей амплитудой, чтобы разница между
    // кадрами была заметна и на глаз, и в цифрах.
    character.add_animation(Animation {
        name: "Walk".into(),
        duration: 0.6,
        looping: true,
        tracks: vec![Track {
            target: AnimTarget::Bone { id: "Head".into(), channel: BoneChannel::PositionY },
            keyframes: vec![
                Keyframe { time: 0.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
                Keyframe { time: 0.3, value: AnimValue::Float(-6.0), interpolation: Interpolation::Linear },
                Keyframe { time: 0.6, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
            ],
        }],
    });

    let mut camera = Camera::default();
    let mut player = pony_core::AnimationPlayer::new();
    pony_script::apply_commands(&mut character, &mut camera, &mut player, &commands);

    let root_pos = character.skeleton.find("Root").unwrap().local_transform.position;
    println!(
        "После скрипта: позиция персонажа = {:?}, Smile = {:.2}, Blink = {:.2}",
        root_pos,
        character.default_morph.get("Smile"),
        character.default_morph.get("Blink"),
    );
    println!(
        "Камера: позиция = {:?}, zoom = {:.2}, shake = {:.2}",
        camera.position, camera.zoom, camera.shake_intensity
    );

    if !player.is_valid(&character) {
        println!("pony.Walk() не запустил анимацию (не должно случиться — Walk только что добавлена)");
        return;
    }
    println!("pony.Walk() запустил анимацию '{}', проигрываем 3 кадра:", player.current_name().unwrap());

    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер пуст — покажу только числа (Head.y), без кадров.");
    }
    let mut renderer = render_cluster.contexts.first().map(Renderer::new);

    for step in 0..3 {
        player.advance(&character, 0.2); // 0.6с анимации / 0.2с шаг = 3 разных фазы
        player.apply(&mut character);
        let head_y = character.skeleton.find("Head").unwrap().local_transform.position.y;
        print!("  t={:.2}: Head.y = {:.2}", player.time(), head_y);

        if let (Some(renderer), Some(ctx)) = (renderer.as_mut(), render_cluster.contexts.first()) {
            let frame = renderer.render_character(ctx, &character, 320, 240, &camera, player.time(), &pony_core::Lighting::default(), None);
            let path = format!("walk_frame_{step}.ppm");
            save_ppm(&path, frame.width, frame.height, &frame.rgba);
            print!(" -> {path}");
        }
        println!();
    }
}

/// Экспорт в GIF (раздел 14 ТЗ). Рендерит полный цикл анимации "Walk" кадр
/// за кадром (24 fps, вся длительность 0.8с зацикленной анимации) и
/// кодирует результат в настоящий анимированный `.gif` через
/// `pony_render::export_gif` — не заглушку, файл открывается любым
/// просмотрщиком.
fn gif_export_demo(render_cluster: &RenderCluster) {
    println!("=== Экспорт: GIF + спрайт-лист (раздел 14 ТЗ) ===");
    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер пуст — экспорт пропущен, нужен CPU-фоллбек рендерер.");
        return;
    }

    let mut character = build_demo_character("GifPony", 0.0);
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

    let mut player = pony_core::AnimationPlayer::new();
    player.play("Walk");
    let camera = Camera::default();

    const FPS: f32 = 24.0;
    let total_frames = (0.8 * FPS).round() as usize;
    let dt = 1.0 / FPS;

    let ctx = &render_cluster.contexts[0];
    let mut renderer = Renderer::new(ctx);

    let mut frames = Vec::with_capacity(total_frames);
    for _ in 0..total_frames {
        player.apply(&mut character);
        let frame = renderer.render_character(ctx, &character, 240, 180, &camera, player.time(), &pony_core::Lighting::default(), None);
        frames.push(frame);
        player.advance(&character, dt);
    }

    let gif_path = "walk_cycle.gif";
    // 100/FPS сотых долей секунды на кадр — единица измерения самого GIF.
    let delay_cs = (100.0 / FPS).round() as u16;
    match export_gif(gif_path, &frames, delay_cs) {
        Ok(()) => {
            let size = std::fs::metadata(gif_path).map(|m| m.len()).unwrap_or(0);
            println!("Экспортировано {} кадров -> {gif_path} ({size} байт)", frames.len());
        }
        Err(err) => println!("Ошибка экспорта GIF: {err}"),
    }

    let sheet_path = "walk_cycle_sheet.png";
    match export_spritesheet(sheet_path, &frames, 5) {
        Ok(layout) => {
            let size = std::fs::metadata(sheet_path).map(|m| m.len()).unwrap_or(0);
            println!(
                "Спрайт-лист {}x{} кадров ({}x{} каждый) -> {sheet_path} ({size} байт)",
                layout.columns, layout.rows, layout.frame_width, layout.frame_height
            );
        }
        Err(err) => println!("Ошибка экспорта спрайт-листа: {err}"),
    }
}

/// Автоматический поворот (раздел 8 ТЗ): рендерит одного персонажа под
/// тремя углами (анфас, 45°, 80° — почти профиль) и печатает ширину
/// непрозрачного силуэта на каждом кадре — должна монотонно сжиматься
/// (foreshortening), а не оставаться одинаковой (что означало бы, что
/// поворот ничего не делает).
fn orientation_demo(render_cluster: &RenderCluster) {
    println!("=== Автоматический поворот, 2.5D (раздел 8 ТЗ) ===");
    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер пуст — демо пропущено, нужен CPU-фоллбек рендерер.");
        return;
    }

    let ctx = &render_cluster.contexts[0];
    let mut renderer = Renderer::new(ctx);
    let camera = Camera::default();

    for (i, yaw_deg) in [0.0f32, 45.0, 80.0].iter().enumerate() {
        let mut character = build_demo_character("YawPony", 0.0);
        character.facing_yaw = yaw_deg.to_radians();
        let frame = renderer.render_character(ctx, &character, 240, 180, &camera, 0.0, &pony_core::Lighting::default(), None);
        let path = format!("yaw_{i}.ppm");
        save_ppm(&path, frame.width, frame.height, &frame.rgba);

        // Ширина непрозрачного силуэта — считаем по первому/последнему
        // столбцу, отличающемуся от цвета неба в левом верхнем углу.
        let sky = &frame.rgba[0..4];
        let mut min_x = frame.width;
        let mut max_x = 0u32;
        for y in 0..frame.height {
            for x in 0..frame.width {
                let idx = ((y * frame.width + x) * 4) as usize;
                let px = &frame.rgba[idx..idx + 4];
                if px != sky {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        let width = max_x.saturating_sub(min_x);
        println!("yaw={yaw_deg:.0}°: ширина силуэта = {width}px -> {path}");
    }
}

/// Частицы (раздел 13 ТЗ): эмиттер снега над сценой, симулируем 1.5с и
/// рендерим 3 кадра, проверяя число живых частиц (должно расти, потом
/// стабилизироваться/колебаться у равновесия рождение==смерть) и то, что
/// они реально сдвигаются вниз кадр к кадру (не застывшая картинка).
fn particles_demo(render_cluster: &RenderCluster) {
    println!("=== Частицы, Snow (раздел 13 ТЗ) ===");
    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер пуст — демо пропущено, нужен CPU-фоллбек рендерер.");
        return;
    }

    let ctx = &render_cluster.contexts[0];
    let mut renderer = Renderer::new(ctx);
    let camera = Camera::default();

    let mut emitter = pony_core::ParticleEmitter::new(pony_core::ParticleKind::Snow, glam::Vec2::new(0.0, 80.0), 15.0);
    emitter.lifetime = 2.0;
    emitter.spread = 60.0;

    for step in 0..3 {
        emitter.update(0.5); // полсекунды симуляции между кадрами
        let frame = renderer.render_particles(ctx, &emitter, 240, 180, &camera, 0.0);
        let path = format!("snow_{step}.ppm");
        save_ppm(&path, frame.width, frame.height, &frame.rgba);
        println!(
            "t={:.1}с: живых частиц = {} -> {path}",
            (step + 1) as f32 * 0.5,
            emitter.particles.len()
        );
    }
}

/// Освещение (раздел 12 ТЗ): рендерит одного персонажа дважды — с
/// `Lighting::default()` (нейтральный свет, должен давать тот же результат,
/// что и раньше, до появления этого модуля) и с настоящим цветным точечным
/// светом рядом с телом — и печатает цвет пикселя в центре тела в обоих
/// случаях, чтобы доказать: свет реально меняет итоговый цвет, а не просто
/// существует как неиспользуемый параметр.
fn lighting_demo(render_cluster: &RenderCluster) {
    println!("=== Освещение: Ambient/Sun/Point (раздел 12 ТЗ) ===");
    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер пуст — демо пропущено, нужен CPU-фоллбек рендерер.");
        return;
    }

    let ctx = &render_cluster.contexts[0];
    let mut renderer = Renderer::new(ctx);
    let camera = Camera::default();
    let character = build_demo_character("LitPony", 0.0);

    let center_color = |frame: &pony_render::FrameOutput| -> [u8; 4] {
        let idx = (((frame.height / 2) * frame.width + frame.width / 2) * 4) as usize;
        [frame.rgba[idx], frame.rgba[idx + 1], frame.rgba[idx + 2], frame.rgba[idx + 3]]
    };

    let neutral = renderer.render_character(ctx, &character, 240, 180, &camera, 0.0, &pony_core::Lighting::default(), None);
    println!("Нейтральный свет (Lighting::default): центр тела = {:?}", center_color(&neutral));

    let warm_point_light = pony_core::Lighting {
        ambient: pony_core::AmbientLight { color: [0.3, 0.3, 0.35], intensity: 1.0 },
        sun: None,
        points: vec![pony_core::PointLight { position: glam::Vec2::new(0.0, 0.0), color: [1.0, 0.5, 0.1], intensity: 1.5, radius: 120.0 }],
    };
    let lit = renderer.render_character(ctx, &character, 240, 180, &camera, 0.0, &warm_point_light, None);
    println!("Тёплый точечный свет у тела: центр тела = {:?}", center_color(&lit));

    save_ppm("lit_neutral.ppm", neutral.width, neutral.height, &neutral.rgba);
    save_ppm("lit_point.ppm", lit.width, lit.height, &lit.rgba);
}

/// Импорт PSD (раздел 16 ТЗ). Показывает оба пути: настоящий несжатый PSD
/// грузится и рендерится нормально, а PSD с Zip-сжатым слоем (реальный
/// баг крейта `psd` 0.3.5 — паникует внутри `Psd::from_bytes`, см.
/// `TextureLoadError::PsdPanic`) не роняет процесс — `catch_unwind` ловит
/// панику, часть просто рисуется цветной заглушкой, как и для любого
/// другого нечитаемого ассета.
fn psd_import_demo(render_cluster: &RenderCluster) {
    println!("=== Импорт PSD (раздел 16 ТЗ) ===");
    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер пуст — демо пропущено, нужен CPU-фоллбек рендерер.");
        return;
    }

    let ctx = &render_cluster.contexts[0];
    let mut renderer = Renderer::new(ctx);
    let camera = Camera::default();

    for (label, path) in [
        ("несжатый (валидный)", "assets/test_fixtures/valid_uncompressed.psd"),
        ("Zip-слой (паникующий в psd-крейте)", "assets/test_fixtures/unsupported_zip_layer.psd"),
    ] {
        let mut character = Character::new("PsdPony");
        character.skeleton = default_pony_skeleton();
        character.add_part(
            Part::new("body", PartKind::Body, PartSource::Psd { path: path.into(), layer: None }).with_bone("Body").with_layer(0),
        );
        let frame = renderer.render_character(ctx, &character, 120, 90, &camera, 0.0, &pony_core::Lighting::default(), None);
        let idx = (((frame.height / 2) * frame.width + frame.width / 2) * 4) as usize;
        println!("{label}: не упало, центр кадра = {:?}", &frame.rgba[idx..idx + 4]);
    }
}

/// Импорт KRA (раздел 16 ТЗ) — формат Krita, по сути zip-архив. Три пути:
/// сведённый `mergedimage.png` (обычный случай), конкретный слой по имени
/// файла внутри архива, и архив без `mergedimage.png` вообще (проверка,
/// что честная ошибка не роняет рендер — тот же принцип, что и у PSD).
fn kra_import_demo(render_cluster: &RenderCluster) {
    println!("=== Импорт KRA (раздел 16 ТЗ) ===");
    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер пуст — демо пропущено, нужен CPU-фоллбек рендерер.");
        return;
    }

    let ctx = &render_cluster.contexts[0];
    let mut renderer = Renderer::new(ctx);
    let camera = Camera::default();

    for (label, path, layer_file) in [
        ("сведённый mergedimage.png", "assets/test_fixtures/valid_mergedimage.kra", None),
        ("конкретный именованный слой", "assets/test_fixtures/valid_with_named_layer.kra", Some("layers/body.png")),
        ("архив без mergedimage.png (должен упасть на заглушку)", "assets/test_fixtures/missing_mergedimage.kra", None),
    ] {
        let mut character = Character::new("KraPony");
        character.skeleton = default_pony_skeleton();
        character.add_part(
            Part::new("body", PartKind::Body, PartSource::Kra { path: path.into(), layer_file: layer_file.map(String::from) })
                .with_bone("Body")
                .with_layer(0),
        );
        let frame = renderer.render_character(ctx, &character, 120, 90, &camera, 0.0, &pony_core::Lighting::default(), None);
        let idx = (((frame.height / 2) * frame.width + frame.width / 2) * 4) as usize;
        println!("{label}: не упало, центр кадра = {:?}", &frame.rgba[idx..idx + 4]);
    }
}

/// Бюджет памяти на практике (`TextureCache` + `LruBudget`, см. README).
/// Специально маленький бюджет (150КБ) — суммарный размер всех PNG-частей
/// демо-персонажа (~286КБ) его не помещается, значит часть текстур
/// обязательно вытеснится. Проверяем не по коду, а по факту: после рендера
/// занятая память НЕ превышает бюджет (вытеснение реально сработало), а
/// повторный рендер того же персонажа не падает (вытесненные текстуры
/// прозрачно перезагружаются по новой).
fn memory_budget_demo(render_cluster: &RenderCluster) {
    println!("=== Бюджет памяти: LRU-вытеснение текстур ===");
    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер пуст — демо пропущено, нужен CPU-фоллбек рендерер.");
        return;
    }

    const SMALL_BUDGET_BYTES: u64 = 150_000;
    let ctx = &render_cluster.contexts[0];
    let mut renderer = Renderer::new_with_budget(ctx, SMALL_BUDGET_BYTES);
    let camera = Camera::default();
    let character = build_demo_character("BudgetPony", 0.0);

    println!("Бюджет: {SMALL_BUDGET_BYTES} байт (суммарный размер всех PNG-частей персонажа ~286000 байт — не влезает целиком)");

    let _ = renderer.render_character(ctx, &character, 240, 180, &camera, 0.0, &pony_core::Lighting::default(), None);
    println!(
        "После 1-го рендера: занято {} из {} байт (в бюджете: {})",
        renderer.texture_memory_used_bytes(),
        renderer.texture_memory_budget_bytes(),
        renderer.texture_memory_used_bytes() <= renderer.texture_memory_budget_bytes()
    );

    // Повторный рендер того же персонажа: часть текстур уже была вытеснена
    // первым проходом (суммарный размер частей больше бюджета) — они
    // должны прозрачно перезагрузиться, не уронив рендер.
    let frame2 = renderer.render_character(ctx, &character, 240, 180, &camera, 0.0, &pony_core::Lighting::default(), None);
    println!(
        "После 2-го рендера (часть текстур перезагружена после вытеснения): не упало, кадр {}x{}, занято {} байт",
        frame2.width,
        frame2.height,
        renderer.texture_memory_used_bytes()
    );
}

/// Доводка pony.Look() (раздел 7 ТЗ): рендерит персонажа с нейтральным
/// взглядом и с `pony.Look()`, направленным резко вверх, и сравнивает
/// пиксели области глаза — должны реально отличаться (глаз повернулся),
/// а кость Head — не тронута (проверено отдельно unit-тестом в pony-script,
/// здесь только визуальное подтверждение на реальном рендере).
fn look_demo(render_cluster: &RenderCluster) {
    println!("=== Доводка pony.Look(): взгляд через морфинг глаза, не поворот головы ===");
    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер пуст — демо пропущено, нужен CPU-фоллбек рендерер.");
        return;
    }

    let ctx = &render_cluster.contexts[0];
    let mut renderer = Renderer::new(ctx);
    let camera = Camera::default();
    let script_engine = ScriptEngine::new();

    let neutral_character = build_demo_character("LookPony", 0.0);
    let neutral_frame = renderer.render_character(ctx, &neutral_character, 240, 180, &camera, 0.0, &pony_core::Lighting::default(), None);

    let mut looking_character = build_demo_character("LookPony", 0.0);
    let mut dummy_camera = Camera::default();
    let mut dummy_player = pony_core::AnimationPlayer::new();
    let commands = script_engine.run("pony.Look(0.0, 1.0);").expect("script should run");
    pony_script::apply_commands(&mut looking_character, &mut dummy_camera, &mut dummy_player, &commands);
    println!(
        "После pony.Look(0.0, 1.0): eyes.rotation = {:.3} рад (ожидаем π/2 ≈ 1.571), кость Head не тронута",
        looking_character.default_morph.eyes.rotation
    );
    let looking_frame = renderer.render_character(ctx, &looking_character, 240, 180, &camera, 0.0, &pony_core::Lighting::default(), None);

    let diff: u32 = neutral_frame
        .rgba
        .iter()
        .zip(looking_frame.rgba.iter())
        .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs())
        .sum();
    println!("Суммарная разница пикселей между нейтральным взглядом и Look(0,1): {diff} (0 означало бы, что Look ничего не меняет)");

    save_ppm("look_neutral.ppm", neutral_frame.width, neutral_frame.height, &neutral_frame.rgba);
    save_ppm("look_up.ppm", looking_frame.width, looking_frame.height, &looking_frame.rgba);
}

/// Masks/Clipping (раздел 60 ТЗ): `content` — часть-фон (Body-заглушка),
/// `mask_shape` — часть-маска (Custom-заглушка) вдвое ýже content и
/// сдвинутая так, чтобы покрывать только его левую половину. Проверяем
/// РЕАЛЬНЫМИ пикселями (не "на глаз"), что: (1) под маской видна ЛЕВАЯ
/// половина content, (2) правая половина — вне маски — обрезана до фона,
/// (3) без `clip_by` та же правая точка видна как обычно (доказывает,
/// что скрытие в (2) — заслуга именно маски, а не случайной дыры в квадах).
fn mask_demo(render_cluster: &RenderCluster) {
    println!("=== Маски/Clipping (раздел 60 ТЗ): растровая альфа-маска по другой части ===");
    if render_cluster.is_cpu_only() {
        println!("Рендер-кластер пуст — демо пропущено, нужен CPU-фоллбек рендерер.");
        return;
    }

    let ctx = &render_cluster.contexts[0];
    let mut renderer = Renderer::new(ctx);
    let camera = Camera::default();

    let mut character = Character::new("MaskDemoPony");
    character.skeleton.add_bone(pony_core::skeleton::Bone {
        id: "Root".into(),
        parent: None,
        local_transform: pony_core::skeleton::Transform2D { position: glam::Vec2::ZERO, rotation: 0.0, scale: glam::Vec2::ONE },
        length: 1.0,
    });
    character.add_part(Part::new("content", PartKind::Body, PartSource::Png { path: "assets/pony/body.png".into() }).with_bone("Root"));
    let mut mask_part = Part::new("mask_shape", PartKind::Custom, PartSource::Png { path: "assets/mask_demo_shape.png".into() }).with_bone("Root");
    mask_part.size = Some(glam::Vec2::new(25.0, 34.0));
    mask_part.pivot = glam::Vec2::new(-12.5, 0.0);
    character.add_part(mask_part);
    character.parts.get_mut("content").unwrap().clip_by = Some("mask_shape".to_string());

    let width = 200u32;
    let height = 150u32;
    let masked = renderer.render_character(ctx, &character, width, height, &camera, 0.0, &pony_core::Lighting::default(), None);

    let mut unmasked_character = character.clone();
    unmasked_character.parts.get_mut("content").unwrap().clip_by = None;
    unmasked_character.parts.remove("mask_shape");
    let unmasked = renderer.render_character(ctx, &unmasked_character, width, height, &camera, 0.0, &pony_core::Lighting::default(), None);

    let pixel_at = |frame: &pony_render::FrameOutput, x: u32, y: u32| -> [u8; 4] {
        let idx = ((y * frame.width + x) * 4) as usize;
        [frame.rgba[idx], frame.rgba[idx + 1], frame.rgba[idx + 2], frame.rgba[idx + 3]]
    };

    let (cy, left_x, right_x) = (height / 2, 85u32, 115u32);
    let bg = pixel_at(&masked, 2, 2);
    let left_masked = pixel_at(&masked, left_x, cy);
    let right_masked = pixel_at(&masked, right_x, cy);
    let right_unmasked = pixel_at(&unmasked, right_x, cy);

    println!("С маской:  слева(x={left_x})={left_masked:?}  справа(x={right_x})={right_masked:?}  фон={bg:?}");
    println!("Без маски: справа(x={right_x})={right_unmasked:?}");

    let left_visible = left_masked[3] > 200 && left_masked != bg;
    let right_hidden = right_masked[3] < 50 || right_masked == bg;
    let right_visible_without_mask = right_unmasked[3] > 200 && right_unmasked != bg;
    println!(
        "Проверки: слева видно под маской={left_visible}, справа обрезано маской={right_hidden}, справа видно без маски={right_visible_without_mask}"
    );

    save_ppm("mask_demo.ppm", masked.width, masked.height, &masked.rgba);
}

fn run_asset_demo() {
    let mut character = Character::new("SamplePony");
    character.skeleton = default_pony_skeleton();

    character
        .add_part(
            Part::new("body", PartKind::Body, PartSource::Vector { path: "assets/body.svg".into() })
                .with_bone("Body")
                .with_layer(0),
        )
        .add_part(
            Part::new("head", PartKind::Head, PartSource::Vector { path: "assets/head.svg".into() })
                .with_bone("Head")
                .with_layer(1),
        )
        .add_part(
            Part::new("eye_l", PartKind::Eyes, PartSource::Vector { path: "assets/eye.svg".into() })
                .with_bone("Head")
                .with_layer(2),
        );

    // Простая анимация "Blink" — вращение века (тут через морф) плюс
    // покачивание головы, чтобы показать работу дорожек/ключей.
    let blink = Animation {
        name: "Blink".into(),
        duration: 0.4,
        looping: false,
        tracks: vec![Track {
            target: AnimTarget::Morph { name: "Blink".into() },
            keyframes: vec![
                Keyframe { time: 0.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
                Keyframe { time: 0.15, value: AnimValue::Float(1.0), interpolation: Interpolation::Linear },
                Keyframe { time: 0.4, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
            ],
        }],
    };

    let idle_head_bob = Animation {
        name: "Idle".into(),
        duration: 2.0,
        looping: true,
        tracks: vec![Track {
            target: AnimTarget::Bone { id: "Head".into(), channel: BoneChannel::PositionY },
            keyframes: vec![
                Keyframe { time: 0.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
                Keyframe { time: 1.0, value: AnimValue::Float(-2.0), interpolation: Interpolation::Linear },
                Keyframe { time: 2.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
            ],
        }],
    };

    character.add_animation(blink).add_animation(idle_head_bob);

    let out_path = "sample_pony.asset";
    character.save_to_file(out_path).expect("failed to save asset");
    println!("Saved character to {out_path}");

    let loaded = Character::load_from_file(out_path).expect("failed to load asset");
    println!(
        "Loaded '{}' v{}: {} parts, {} bones, {} animations",
        loaded.name,
        loaded.version,
        loaded.parts.len(),
        loaded.skeleton.bones.len(),
        loaded.animations.len()
    );

    // Пример вычисления мировой трансформации кости и сэмплинга анимации.
    if let Some(world_head) = loaded.skeleton.world_transform("Head") {
        println!("Head world position: {:?}", world_head.position);
    }
    if let Some(idle) = loaded.animations.get("Idle") {
        if let Some(track) = idle.tracks.first() {
            println!("Idle head-bob at t=0.5s: {:?}", track.sample(0.5));
        }
    }
}
