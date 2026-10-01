use thiserror::Error;

/// Presentation-only shading that changes how a frame looks, never which voxel a pixel shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PresentationStyle {
    /// Fades contacts toward the background before the far plane so the clip edge is hidden.
    pub distance_fog: bool,
    /// Darkens each voxel face near its edges so adjacent same-material voxels stay distinct.
    pub voxel_edge_shading: bool,
}

impl PresentationStyle {
    /// Both Render Paths fog toward, and clear to, this colour so switching does not jump.
    /// The shaders repeat it as `DISTANCE_FOG_COLOR`.
    pub const DISTANCE_FOG_LINEAR_COLOR: [f32; 3] = [0.42, 0.56, 0.74];

    pub const SCENIC: Self = Self {
        distance_fog: true,
        voxel_edge_shading: true,
    };
}

/// Fraction of the far plane at which distance fog begins.
const DISTANCE_FOG_START: f32 = 0.55;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraState {
    eye: [f32; 3],
    target: [f32; 3],
    up: [f32; 3],
    field_of_view_degrees: f32,
    near_plane: f32,
    far_plane: f32,
    radial_far_clip: bool,
    presentation_style: PresentationStyle,
}

impl CameraState {
    /// Validates the camera for every nonzero `u32` drawable extent.
    pub fn new(
        eye: [f32; 3],
        target: [f32; 3],
        up: [f32; 3],
        field_of_view_degrees: f32,
        near_plane: f32,
        far_plane: f32,
    ) -> Result<Self, CameraConfigurationError> {
        for (parameter, values) in [
            ("eye", eye.as_slice()),
            ("target", target.as_slice()),
            ("up", up.as_slice()),
            (
                "field of view",
                std::slice::from_ref(&field_of_view_degrees),
            ),
            ("near plane", std::slice::from_ref(&near_plane)),
            ("far plane", std::slice::from_ref(&far_plane)),
        ] {
            if values.iter().any(|value| !value.is_finite()) {
                return Err(CameraConfigurationError::NonfiniteParameter { parameter });
            }
        }
        if eye == target {
            return Err(CameraConfigurationError::CoincidentEyeAndTarget);
        }
        let direction = subtract(target, eye);
        let direction_length_squared = dot(direction, direction);
        if !direction_length_squared.is_finite() || direction_length_squared <= 0.0 {
            return Err(CameraConfigurationError::UnrepresentableTransform);
        }
        // Test collinearity before normalization can introduce rounding noise.
        let [direction_x, direction_y, direction_z] = direction.map(f64::from);
        let [up_x, up_y, up_z] = up.map(f64::from);
        if direction_y * up_z == direction_z * up_y
            && direction_z * up_x == direction_x * up_z
            && direction_x * up_y == direction_y * up_x
        {
            return Err(CameraConfigurationError::DegenerateUpDirection);
        }
        let side = cross(normalize(direction), up);
        let side_length_squared = dot(side, side);
        if side_length_squared == 0.0 {
            return Err(CameraConfigurationError::DegenerateUpDirection);
        }
        if !side_length_squared.is_finite() {
            return Err(CameraConfigurationError::UnrepresentableTransform);
        }
        if !(0.0 < field_of_view_degrees && field_of_view_degrees < 180.0) {
            return Err(CameraConfigurationError::InvalidFieldOfView);
        }
        if !(0.0 < near_plane && near_plane < far_plane) {
            return Err(CameraConfigurationError::InvalidClipPlanes);
        }
        let camera = Self {
            eye,
            target,
            up,
            field_of_view_degrees,
            near_plane,
            far_plane,
            radial_far_clip: false,
            presentation_style: PresentationStyle::default(),
        };
        // Only the horizontal projection scale depends on extent. Its largest
        // magnitude occurs at the narrowest supported aspect ratio.
        camera.view_projection([1, u32::MAX])?;
        Ok(camera)
    }

    pub fn eye(self) -> [f32; 3] {
        self.eye
    }

    pub fn target(self) -> [f32; 3] {
        self.target
    }

    pub fn up(self) -> [f32; 3] {
        self.up
    }

    pub fn field_of_view_degrees(self) -> f32 {
        self.field_of_view_degrees
    }

    pub fn near_plane(self) -> f32 {
        self.near_plane
    }

    pub fn far_plane(self) -> f32 {
        self.far_plane
    }

    /// Also limits visibility to `far_plane` units from the eye, independent of orientation.
    pub fn with_radial_far_clip(mut self) -> Self {
        self.radial_far_clip = true;
        self
    }

    pub fn radial_far_clip(self) -> bool {
        self.radial_far_clip
    }

    pub fn with_presentation_style(mut self, style: PresentationStyle) -> Self {
        self.presentation_style = style;
        self
    }

    pub fn presentation_style(self) -> PresentationStyle {
        self.presentation_style
    }

    /// The eye distances over which fog rises from none to the full background colour.
    pub fn distance_fog_range(self) -> Option<(f32, f32)> {
        self.presentation_style
            .distance_fog
            .then_some((self.far_plane * DISTANCE_FOG_START, self.far_plane))
    }

    /// Shader constants shared by every Render Path: fog start, fog end (zero when fog is
    /// off), an edge-shading flag and padding to a whole vector.
    pub fn presentation_constants(self) -> [f32; 4] {
        let (fog_start, fog_end) = self.distance_fog_range().unwrap_or((0.0, 0.0));
        let edge_shading = if self.presentation_style.voxel_edge_shading {
            1.0
        } else {
            0.0
        };
        [fog_start, fog_end, edge_shading, 0.0]
    }

    /// Replaces both clip planes while keeping the far clipping mode and presentation style.
    pub fn with_clip_planes(
        self,
        near_plane: f32,
        far_plane: f32,
    ) -> Result<Self, CameraConfigurationError> {
        let camera = Self::new(
            self.eye,
            self.target,
            self.up,
            self.field_of_view_degrees,
            near_plane,
            far_plane,
        )?;
        Ok(Self {
            radial_far_clip: self.radial_far_clip,
            presentation_style: self.presentation_style,
            ..camera
        })
    }

    pub fn view_projection(
        self,
        drawable_dimensions: [u32; 2],
    ) -> Result<[f32; 16], CameraConfigurationError> {
        let [width, height] = drawable_dimensions;
        if width == 0 || height == 0 {
            return Err(CameraConfigurationError::ZeroDrawableExtent);
        }
        let aspect_ratio = width as f32 / height as f32;
        let projection = perspective(
            self.field_of_view_degrees.to_radians(),
            aspect_ratio,
            self.near_plane,
            self.far_plane,
        );
        let view = look_at(self.eye, self.target, self.up);
        let matrix = multiply_matrices(projection, view);
        if matrix.iter().any(|component| !component.is_finite()) {
            return Err(CameraConfigurationError::UnrepresentableTransform);
        }
        Ok(matrix)
    }
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            eye: [5.0, 4.0, 6.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            field_of_view_degrees: 55.0,
            near_plane: 0.1,
            far_plane: 100.0,
            radial_far_clip: false,
            presentation_style: PresentationStyle::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeterministicCameraMove {
    start: CameraState,
    end: CameraState,
    total_steps: u32,
}

impl DeterministicCameraMove {
    pub fn new(
        start: CameraState,
        end: CameraState,
        total_steps: u32,
    ) -> Result<Self, CameraConfigurationError> {
        if total_steps == 0 {
            return Err(CameraConfigurationError::ZeroMoveSteps);
        }
        if start.radial_far_clip() != end.radial_far_clip() {
            return Err(CameraConfigurationError::DifferentFarClipModes);
        }
        if start.presentation_style() != end.presentation_style() {
            return Err(CameraConfigurationError::DifferentPresentationStyles);
        }
        Ok(Self {
            start,
            end,
            total_steps,
        })
    }

    pub fn total_steps(self) -> u32 {
        self.total_steps
    }

    pub fn pose_at_step(self, step: u32) -> Result<CameraState, CameraConfigurationError> {
        if step > self.total_steps {
            return Err(CameraConfigurationError::MoveStepOutOfRange {
                step,
                total_steps: self.total_steps,
            });
        }
        let progress = step as f32 / self.total_steps as f32;
        let camera = CameraState::new(
            interpolate_vector(self.start.eye(), self.end.eye(), progress),
            interpolate_vector(self.start.target(), self.end.target(), progress),
            interpolate_vector(self.start.up(), self.end.up(), progress),
            interpolate_scalar(
                self.start.field_of_view_degrees(),
                self.end.field_of_view_degrees(),
                progress,
            ),
            interpolate_scalar(self.start.near_plane(), self.end.near_plane(), progress),
            interpolate_scalar(self.start.far_plane(), self.end.far_plane(), progress),
        )?;
        let camera = camera.with_presentation_style(self.start.presentation_style());
        Ok(if self.start.radial_far_clip() {
            camera.with_radial_far_clip()
        } else {
            camera
        })
    }
}

fn interpolate_vector(start: [f32; 3], end: [f32; 3], progress: f32) -> [f32; 3] {
    let [start_x, start_y, start_z] = start;
    let [end_x, end_y, end_z] = end;
    [
        interpolate_scalar(start_x, end_x, progress),
        interpolate_scalar(start_y, end_y, progress),
        interpolate_scalar(start_z, end_z, progress),
    ]
}

fn interpolate_scalar(start: f32, end: f32, progress: f32) -> f32 {
    start + (end - start) * progress
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CameraConfigurationError {
    #[error("camera {parameter} must contain only finite values")]
    NonfiniteParameter { parameter: &'static str },
    #[error("camera eye and target must be distinct")]
    CoincidentEyeAndTarget,
    #[error("camera up direction must be nonzero and not parallel to the view direction")]
    DegenerateUpDirection,
    #[error("camera vertical field of view must be greater than 0 and less than 180 degrees")]
    InvalidFieldOfView,
    #[error("camera clip planes must satisfy 0 < near < far")]
    InvalidClipPlanes,
    #[error(
        "camera transform exceeds f32 precision or range; reduce coordinate, up vector, or projection extremes"
    )]
    UnrepresentableTransform,
    #[error("camera projection requires a non-zero drawable extent")]
    ZeroDrawableExtent,
    #[error("a deterministic camera move requires at least one step")]
    ZeroMoveSteps,
    #[error("a deterministic camera move requires the same far clipping mode at both endpoints")]
    DifferentFarClipModes,
    #[error("a deterministic camera move requires the same presentation style at both endpoints")]
    DifferentPresentationStyles,
    #[error("camera move step {step} exceeds the final step {total_steps}")]
    MoveStepOutOfRange { step: u32, total_steps: u32 },
}

fn perspective(field_of_view: f32, aspect_ratio: f32, near: f32, far: f32) -> [f32; 16] {
    let focal_length = 1.0 / (field_of_view * 0.5).tan();
    [
        focal_length / aspect_ratio,
        0.0,
        0.0,
        0.0,
        0.0,
        -focal_length,
        0.0,
        0.0,
        0.0,
        0.0,
        far / (near - far),
        -1.0,
        0.0,
        0.0,
        near * far / (near - far),
        0.0,
    ]
}

fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let forward = normalize(subtract(center, eye));
    let side = normalize(cross(forward, up));
    let upward = cross(side, forward);
    let [side_x, side_y, side_z] = side;
    let [upward_x, upward_y, upward_z] = upward;
    let [forward_x, forward_y, forward_z] = forward;
    [
        side_x,
        upward_x,
        -forward_x,
        0.0,
        side_y,
        upward_y,
        -forward_y,
        0.0,
        side_z,
        upward_z,
        -forward_z,
        0.0,
        -dot(side, eye),
        -dot(upward, eye),
        dot(forward, eye),
        1.0,
    ]
}

fn multiply_matrices(left: [f32; 16], right: [f32; 16]) -> [f32; 16] {
    let mut result = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            let Some(destination) = result.get_mut(column * 4 + row) else {
                continue;
            };
            *destination = (0..4)
                .filter_map(|inner| {
                    let left_value = left.get(inner * 4 + row)?;
                    let right_value = right.get(column * 4 + inner)?;
                    Some(left_value * right_value)
                })
                .sum();
        }
    }
    result
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    [left_x - right_x, left_y - right_y, left_z - right_z]
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    left_x * right_x + left_y * right_y + left_z * right_z
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    [
        left_y * right_z - left_z * right_y,
        left_z * right_x - left_x * right_z,
        left_x * right_y - left_y * right_x,
    ]
}

fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = dot(vector, vector).sqrt();
    let [vector_x, vector_y, vector_z] = vector;
    [vector_x / length, vector_y / length, vector_z / length]
}
