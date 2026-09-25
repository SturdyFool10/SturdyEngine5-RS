use sturdy_sys::ffi;

macro_rules! mirrored_enum {
    ($(#[$m:meta])* $name:ident => $ffi:ident { $($variant:ident),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl From<$name> for ffi::$ffi {
            fn from(value: $name) -> Self {
                match value { $($name::$variant => ffi::$ffi::$variant),+ }
            }
        }

        // The reverse direction: other subsystem bridges (e.g. `render::ffi::PresentationSettings`)
        // reuse these same root-bridge enums for their own *outputs*, so a round trip needs a way
        // back. `ffi::$ffi` isn't necessarily `sturdy_sys::ffi::$ffi` here — cxx's shared-enum reuse
        // (`#[namespace = "sturdy_rs"] type X = crate::ffi::X;`) makes it the identical type under a
        // different bridge's path, and this impl covers all of those paths at once.
        impl From<ffi::$ffi> for $name {
            fn from(value: ffi::$ffi) -> Self {
                // cxx shared enums are a repr-integer newtype with associated consts, not a closed
                // Rust enum, so the match can't be proven exhaustive — the wildcard only fires for
                // a discriminant the engine itself never produces for this type.
                match value {
                    $(ffi::$ffi::$variant => $name::$variant,)+
                    _ => unreachable!("unknown {} discriminant {:?}", stringify!($ffi), value),
                }
            }
        }
    };
}

mirrored_enum!(
    VSync => VSync { Off, On, Adaptive }
);
mirrored_enum!(
    VariableRefresh => VariableRefresh { Disabled, Automatic, Preferred }
);
mirrored_enum!(
    LatencyMode => LatencyMode { Normal, Low, Ultra }
);
mirrored_enum!(
    PresentationPreference => PresentationPreference {
        Automatic, LowestLatency, Smoothest, PowerEfficient
    }
);
mirrored_enum!(
    WindowMode => WindowMode { Windowed, BorderlessFullscreen, ExclusiveFullscreen }
);

/// Everything the runtime needs before the first frame. Built fluently:
///
/// ```
/// # use sturdy::*;
/// let config = RuntimeConfig::new("Demo").size(1600, 900).vsync(VSync::Adaptive).raytracing(true);
/// ```
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub(crate) window_title: String,
    pub(crate) app_name: String,
    pub(crate) shaders_directory: String,
    pub(crate) size: (u32, u32),
    pub(crate) resizable: bool,
    pub(crate) decorated: bool,
    pub(crate) high_dpi: bool,
    pub(crate) window_mode: WindowMode,
    pub(crate) raytracing: bool,
    pub(crate) vsync: VSync,
    pub(crate) variable_refresh: VariableRefresh,
    pub(crate) latency: LatencyMode,
    pub(crate) preference: PresentationPreference,
    pub(crate) title_update_interval_seconds: f64,
    pub(crate) runtime_window_management: bool,
}

impl RuntimeConfig {
    pub fn new(title: impl Into<String>) -> Self {
        let title = title.into();
        Self {
            app_name: title.clone(),
            window_title: title,
            shaders_directory: "Shaders".into(),
            size: (1280, 720),
            resizable: true,
            decorated: true,
            high_dpi: true,
            window_mode: WindowMode::Windowed,
            raytracing: false,
            vsync: VSync::On,
            variable_refresh: VariableRefresh::Automatic,
            latency: LatencyMode::Normal,
            preference: PresentationPreference::Automatic,
            title_update_interval_seconds: 0.0,
            runtime_window_management: false,
        }
    }

    #[must_use]
    pub fn app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = name.into();
        self
    }
    #[must_use]
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = (width, height);
        self
    }
    #[must_use]
    pub fn resizable(mut self, on: bool) -> Self {
        self.resizable = on;
        self
    }
    #[must_use]
    pub fn decorated(mut self, on: bool) -> Self {
        self.decorated = on;
        self
    }
    #[must_use]
    pub fn high_dpi(mut self, on: bool) -> Self {
        self.high_dpi = on;
        self
    }
    #[must_use]
    pub fn window_mode(mut self, mode: WindowMode) -> Self {
        self.window_mode = mode;
        self
    }
    #[must_use]
    pub fn raytracing(mut self, on: bool) -> Self {
        self.raytracing = on;
        self
    }
    #[must_use]
    pub fn vsync(mut self, mode: VSync) -> Self {
        self.vsync = mode;
        self
    }
    #[must_use]
    pub fn variable_refresh(mut self, mode: VariableRefresh) -> Self {
        self.variable_refresh = mode;
        self
    }
    #[must_use]
    pub fn latency(mut self, mode: LatencyMode) -> Self {
        self.latency = mode;
        self
    }
    #[must_use]
    pub fn presentation_preference(mut self, preference: PresentationPreference) -> Self {
        self.preference = preference;
        self
    }
    /// Directory the engine looks in for shader sources at runtime.
    #[must_use]
    pub fn shaders_directory(mut self, dir: impl Into<String>) -> Self {
        self.shaders_directory = dir.into();
        self
    }
    /// Refresh the window title (e.g. with frame stats) every `seconds`.
    #[must_use]
    pub fn title_update_interval(mut self, seconds: f64) -> Self {
        self.title_update_interval_seconds = seconds;
        self
    }
    #[must_use]
    pub fn runtime_window_management(mut self, on: bool) -> Self {
        self.runtime_window_management = on;
        self
    }

    pub(crate) fn to_ffi(&self) -> ffi::RuntimeOptions {
        ffi::RuntimeOptions {
            window_title: self.window_title.clone(),
            app_name: self.app_name.clone(),
            shaders_directory: self.shaders_directory.clone(),
            width: self.size.0,
            height: self.size.1,
            resizable: self.resizable,
            decorated: self.decorated,
            high_dpi: self.high_dpi,
            window_mode: self.window_mode.into(),
            raytracing: self.raytracing,
            vsync: self.vsync.into(),
            variable_refresh: self.variable_refresh.into(),
            latency: self.latency.into(),
            preference: self.preference.into(),
            title_update_interval_seconds: self.title_update_interval_seconds,
            runtime_window_management: self.runtime_window_management,
        }
    }
}
