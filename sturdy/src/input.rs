use sturdy_sys::{ffi, EngineView};

use crate::keys::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MouseButton {
    Left = 1,
    Middle = 2,
    Right = 3,
    Extra1 = 4,
    Extra2 = 5,
    Extra3 = 6,
    Extra4 = 7,
}

/// Read-only view of this tick's keyboard and mouse state.
#[derive(Clone, Copy)]
pub struct Input<'a> {
    view: &'a EngineView,
}

impl<'a> Input<'a> {
    pub(crate) fn new(view: &'a EngineView) -> Self {
        Self { view }
    }

    pub fn key_down(&self, key: Key) -> bool {
        ffi::input_key_down(self.view, key.code())
    }
    pub fn key_just_pressed(&self, key: Key) -> bool {
        ffi::input_key_just_pressed(self.view, key.code())
    }
    pub fn key_just_released(&self, key: Key) -> bool {
        ffi::input_key_just_released(self.view, key.code())
    }

    pub fn mouse_down(&self, button: MouseButton) -> bool {
        ffi::input_mouse_down(self.view, button as u8)
    }
    pub fn mouse_just_pressed(&self, button: MouseButton) -> bool {
        ffi::input_mouse_just_pressed(self.view, button as u8)
    }
    pub fn mouse_just_released(&self, button: MouseButton) -> bool {
        ffi::input_mouse_just_released(self.view, button as u8)
    }

    /// Cursor position in window pixels.
    pub fn mouse_position(&self) -> glam::Vec2 {
        ffi::input_mouse_position(self.view).into()
    }
    pub fn mouse_delta(&self) -> glam::Vec2 {
        ffi::input_mouse_delta(self.view).into()
    }
    pub fn wheel_delta(&self) -> glam::Vec2 {
        ffi::input_wheel_delta(self.view).into()
    }

    /// Text typed this tick (after IME composition), as UTF-8.
    pub fn text(&self) -> String {
        ffi::input_text_this_tick(self.view)
    }
}
