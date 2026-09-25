use sturdy_sys::ffi;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    Perspective { vertical_fov_degrees: f32 },
    Orthographic { vertical_size: f32 },
}

/// A camera described by value; the engine builds its own camera from it each frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub projection: Projection,
    pub position: glam::Vec3,
    /// Pitch, yaw, roll in degrees.
    pub euler_degrees: glam::Vec3,
    /// `None` uses the framebuffer's aspect ratio.
    pub aspect_ratio: Option<f32>,
    pub near_clip: f32,
    pub far_clip: f32,
}

impl Camera {
    pub const fn perspective() -> Self {
        Self::with(Projection::Perspective { vertical_fov_degrees: 60.0 })
    }

    pub const fn orthographic() -> Self {
        Self::with(Projection::Orthographic { vertical_size: 10.0 })
    }

    const fn with(projection: Projection) -> Self {
        Self {
            projection,
            position: glam::Vec3::ZERO,
            euler_degrees: glam::Vec3::ZERO,
            aspect_ratio: None,
            near_clip: 0.05,
            far_clip: 1000.0,
        }
    }

    #[must_use]
    pub fn at(mut self, position: impl Into<glam::Vec3>) -> Self {
        self.position = position.into();
        self
    }

    #[must_use]
    pub fn euler_degrees(mut self, pitch_yaw_roll: impl Into<glam::Vec3>) -> Self {
        self.euler_degrees = pitch_yaw_roll.into();
        self
    }

    #[must_use]
    pub const fn fov_degrees(mut self, degrees: f32) -> Self {
        self.projection = Projection::Perspective { vertical_fov_degrees: degrees };
        self
    }

    #[must_use]
    pub const fn clip(mut self, near: f32, far: f32) -> Self {
        self.near_clip = near;
        self.far_clip = far;
        self
    }

    pub(crate) fn to_ffi(self, framebuffer_aspect: f32) -> ffi::CameraDesc {
        let (kind, fov, size) = match self.projection {
            Projection::Perspective { vertical_fov_degrees } => {
                (ffi::CameraProjection::Perspective, vertical_fov_degrees, 10.0)
            }
            Projection::Orthographic { vertical_size } => {
                (ffi::CameraProjection::Orthographic, 60.0, vertical_size)
            }
        };
        ffi::CameraDesc {
            projection: kind,
            position: self.position.into(),
            euler_degrees: self.euler_degrees.into(),
            vertical_fov_degrees: fov,
            orthographic_size: size,
            aspect_ratio: self.aspect_ratio.unwrap_or(framebuffer_aspect),
            near_clip: self.near_clip,
            far_clip: self.far_clip,
        }
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::perspective()
    }
}
