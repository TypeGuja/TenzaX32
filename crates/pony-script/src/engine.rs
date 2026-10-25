//! Обёртка над `rhai::Engine`: регистрирует объекты `pony` и `camera` с
//! методами ровно тех имён, что в разделе 15 ТЗ (`pony.Move()`,
//! `camera.Zoom()` и т.д.), и даёт им писать в общую очередь команд.

use std::cell::RefCell;
use std::rc::Rc;

use rhai::{Dynamic, Engine, EvalAltResult, Scope};

use crate::commands::Command;

type Queue = Rc<RefCell<Vec<Command>>>;

/// Прокси-объект `pony` внутри скрипта. Клонируемый (нужно для rhai),
/// но клон — это клон Rc, т.е. все клоны пишут в одну и ту же очередь.
#[derive(Clone)]
struct PonyProxy {
    queue: Queue,
}

impl PonyProxy {
    fn push(&mut self, cmd: Command) {
        self.queue.borrow_mut().push(cmd);
    }
}

#[derive(Clone)]
struct CameraProxy {
    queue: Queue,
}

impl CameraProxy {
    fn push(&mut self, cmd: Command) {
        self.queue.borrow_mut().push(cmd);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("script error: {0}")]
    Eval(#[from] Box<EvalAltResult>),
}

/// Движок с уже зарегистрированными типами `pony`/`camera`. Дорого создавать
/// один раз (регистрация функций не бесплатна) и дёшево переиспользовать —
/// `run()` не мутирует сам движок, только создаёт новый Scope на вызов.
pub struct ScriptEngine {
    engine: Engine,
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptEngine {
    pub fn new() -> Self {
        let mut engine = Engine::new();

        // Скрипт выполняется в потоке интерфейса: `loop {}` или очень
        // длинный цикл вешал редактор насовсем. Предел операций превращает
        // это в обычную ошибку скрипта.
        engine.set_max_operations(5_000_000);

        // Числа в rhai бывают целыми (`5`) и дробными (`5.0`), а функции
        // регистрируются под точные типы: раньше `pony.Move(5, 0)` падал с
        // "function not found" — работало только `pony.Move(5.0, 0.0)`.
        // Поэтому аргументы принимаются как Dynamic и приводятся к числу.
        type R = Result<(), Box<EvalAltResult>>;
        fn num(v: &Dynamic) -> Result<f32, Box<EvalAltResult>> {
            v.as_float()
                .map(|f| f as f32)
                .or_else(|_| v.as_int().map(|i| i as f32))
                .map_err(|t| format!("ожидалось число, получено {t}").into())
        }

        engine.register_type_with_name::<PonyProxy>("Pony");
        engine.register_fn("Move", |p: &mut PonyProxy, dx: Dynamic, dy: Dynamic| -> R {
            p.push(Command::Move { dx: num(&dx)?, dy: num(&dy)? });
            Ok(())
        });
        engine.register_fn("Look", |p: &mut PonyProxy, x: Dynamic, y: Dynamic| -> R {
            p.push(Command::Look { x: num(&x)?, y: num(&y)? });
            Ok(())
        });
        engine.register_fn("Blink", |p: &mut PonyProxy| {
            p.push(Command::Blink);
        });
        engine.register_fn("Smile", |p: &mut PonyProxy, amount: Dynamic| -> R {
            p.push(Command::Smile { amount: num(&amount)? });
            Ok(())
        });
        engine.register_fn("Walk", |p: &mut PonyProxy| {
            p.push(Command::Walk);
        });

        engine.register_type_with_name::<CameraProxy>("Camera");
        engine.register_fn("Move", |c: &mut CameraProxy, dx: Dynamic, dy: Dynamic| -> R {
            c.push(Command::CameraMove { dx: num(&dx)?, dy: num(&dy)? });
            Ok(())
        });
        engine.register_fn("Rotate", |c: &mut CameraProxy, radians: Dynamic| -> R {
            c.push(Command::CameraRotate { radians: num(&radians)? });
            Ok(())
        });
        engine.register_fn("Zoom", |c: &mut CameraProxy, factor: Dynamic| -> R {
            c.push(Command::CameraZoom { factor: num(&factor)? });
            Ok(())
        });
        engine.register_fn("Shake", |c: &mut CameraProxy, intensity: Dynamic| -> R {
            c.push(Command::CameraShake { intensity: num(&intensity)? });
            Ok(())
        });
        engine.register_fn("Depth", |c: &mut CameraProxy, value: Dynamic| -> R {
            c.push(Command::CameraDepth { value: num(&value)? });
            Ok(())
        });
        engine.register_fn("Blur", |c: &mut CameraProxy, value: Dynamic| -> R {
            c.push(Command::CameraBlur { value: num(&value)? });
            Ok(())
        });

        Self { engine }
    }

    /// Выполнить скрипт и вернуть команды, которые он испустил через
    /// `pony.*`/`camera.*` — в порядке вызова. Ничего не применяется к
    /// реальному состоянию здесь — см. `apply_commands`.
    pub fn run(&self, script: &str) -> Result<Vec<Command>, ScriptError> {
        let queue: Queue = Rc::new(RefCell::new(Vec::new()));

        let mut scope = Scope::new();
        scope.push("pony", PonyProxy { queue: queue.clone() });
        scope.push("camera", CameraProxy { queue: queue.clone() });

        self.engine.run_with_scope(&mut scope, script)?;

        // В этот момент все клоны прокси в scope уронены вместе со scope,
        // но исходный `queue` (Rc) всё ещё жив здесь — забираем содержимое.
        Ok(Rc::try_unwrap(queue)
            .map(|cell| cell.into_inner())
            .unwrap_or_else(|rc| rc.borrow().clone()))
    }
}
