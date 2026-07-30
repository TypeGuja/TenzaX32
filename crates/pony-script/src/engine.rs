//! Обёртка над `rhai::Engine`: регистрирует объекты `pony` и `camera` с
//! методами ровно тех имён, что в разделе 15 ТЗ (`pony.Move()`,
//! `camera.Zoom()` и т.д.), и даёт им писать в общую очередь команд.

use std::cell::RefCell;
use std::rc::Rc;

use rhai::{Engine, EvalAltResult, Scope};

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

        engine.register_type_with_name::<PonyProxy>("Pony");
        engine.register_fn("Move", |p: &mut PonyProxy, dx: f64, dy: f64| {
            p.push(Command::Move { dx: dx as f32, dy: dy as f32 });
        });
        engine.register_fn("Look", |p: &mut PonyProxy, x: f64, y: f64| {
            p.push(Command::Look { x: x as f32, y: y as f32 });
        });
        engine.register_fn("Blink", |p: &mut PonyProxy| {
            p.push(Command::Blink);
        });
        engine.register_fn("Smile", |p: &mut PonyProxy, amount: f64| {
            p.push(Command::Smile { amount: amount as f32 });
        });
        engine.register_fn("Walk", |p: &mut PonyProxy| {
            p.push(Command::Walk);
        });

        engine.register_type_with_name::<CameraProxy>("Camera");
        engine.register_fn("Move", |c: &mut CameraProxy, dx: f64, dy: f64| {
            c.push(Command::CameraMove { dx: dx as f32, dy: dy as f32 });
        });
        engine.register_fn("Rotate", |c: &mut CameraProxy, radians: f64| {
            c.push(Command::CameraRotate { radians: radians as f32 });
        });
        engine.register_fn("Zoom", |c: &mut CameraProxy, factor: f64| {
            c.push(Command::CameraZoom { factor: factor as f32 });
        });
        engine.register_fn("Shake", |c: &mut CameraProxy, intensity: f64| {
            c.push(Command::CameraShake { intensity: intensity as f32 });
        });
        engine.register_fn("Depth", |c: &mut CameraProxy, value: f64| {
            c.push(Command::CameraDepth { value: value as f32 });
        });
        engine.register_fn("Blur", |c: &mut CameraProxy, value: f64| {
            c.push(Command::CameraBlur { value: value as f32 });
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
