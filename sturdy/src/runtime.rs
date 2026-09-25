use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::ExitCode;

use sturdy_sys::{ffi, LogicHandle};

use crate::config::RuntimeConfig;
use crate::engine::{Engine, Frame, FrameDescription};

/// Failure reported from game logic (or by the runtime) as an ordinary Rust error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

/// Your game. The engine drives it; everything runs on the engine's calling thread(s), and the
/// value must be `Send` because the engine may move callbacks between its own threads.
pub trait GameLogic: Send + 'static {
    /// Called once the window and graphics device exist. Returning `Err` aborts startup.
    fn init(&mut self, engine: &mut Engine<'_>) -> Result<(), Error> {
        let _ = engine;
        Ok(())
    }

    /// Called every frame. Return what to render, or `None` to skip presenting this frame.
    fn frame(&mut self, engine: &mut Engine<'_>, frame: &Frame) -> Option<FrameDescription>;

    /// Called once when the engine shuts down.
    fn shutdown(&mut self, engine: &mut Engine<'_>) {
        let _ = engine;
    }
}

/// A panic must never unwind into C++ frames (undefined behavior), so it is reported and the
/// process aborts.
fn guard<R>(what: &str, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("sturdy: game logic panicked in `{what}`; aborting (cannot unwind through C++)");
            std::process::abort();
        }
    }
}

fn into_handle<L: GameLogic>(logic: L) -> Box<LogicHandle> {
    use std::sync::{Arc, Mutex};
    // One logic value serves all three callbacks. The mutex is uncontended (the engine calls
    // logic serially) and only exists so the three closures can share it without unsafe.
    let shared = Arc::new(Mutex::new(logic));
    let (a, b, c) = (shared.clone(), shared.clone(), shared);

    Box::new(LogicHandle {
        on_init: Box::new(move |view| {
            guard("init", || {
                let mut engine = Engine::new(view);
                match a.lock().unwrap().init(&mut engine) {
                    Ok(()) => ffi::Status { ok: true, message: String::new() },
                    Err(e) => ffi::Status { ok: false, message: e.0 },
                }
            })
        }),
        request_frame: Box::new(move |view, info| {
            guard("frame", || {
                let mut engine = Engine::new(view);
                let frame = Frame::from_ffi(info);
                match b.lock().unwrap().frame(&mut engine, &frame) {
                    Some(description) => {
                        // Side channel, not a `FrameRequest` field -- see
                        // `sturdy_sys::render::ffi::set_pending_render_graph`'s doc comment for why.
                        sturdy_sys::render::ffi::set_pending_render_graph(
                            &description.render_graph.to_ffi(),
                        );
                        ffi::FrameRequest {
                            present: true,
                            camera: description.camera.to_ffi(frame.aspect_ratio()),
                            lighting: description.lighting.to_ffi(),
                            debug_label: description.debug_label,
                        }
                    }
                    None => ffi::FrameRequest {
                        present: false,
                        camera: crate::camera::Camera::default().to_ffi(1.0),
                        lighting: crate::engine::SceneLighting::default().to_ffi(),
                        debug_label: String::new(),
                    },
                }
            })
        }),
        on_shutdown: Box::new(move |view| {
            guard("shutdown", || {
                let mut engine = Engine::new(view);
                c.lock().unwrap().shutdown(&mut engine);
            })
        }),
    })
}

/// Runs the engine with `logic` until the application exits, using the process's own arguments.
pub fn run<L: GameLogic>(config: RuntimeConfig, logic: L) -> ExitCode {
    run_with_args(config, logic, std::env::args())
}

/// Like [`run`], with explicit command-line arguments (first item is the program name).
///
/// Only one runtime may run per process, and it must run on the thread that owns the window
/// system (the main thread on most platforms).
pub fn run_with_args<L: GameLogic>(
    config: RuntimeConfig,
    logic: L,
    args: impl IntoIterator<Item = String>,
) -> ExitCode {
    let args: Vec<String> = args.into_iter().collect();
    let code = ffi::run_runtime(&config.to_ffi(), &args, into_handle(logic));
    u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from)
}
