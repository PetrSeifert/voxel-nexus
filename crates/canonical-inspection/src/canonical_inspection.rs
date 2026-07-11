use raster_render_path::{CameraConfigurationError, CameraPose, DeterministicCameraMove};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanonicalCameraPose {
    Overview,
    CavityMaterialCloseUp,
    BoundaryCutaway,
}

impl CanonicalCameraPose {
    pub fn pose(self) -> CameraPose {
        match self {
            Self::Overview => CameraPose::new(
                [20.0, 14.0, 22.0],
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                50.0,
                0.1,
                100.0,
            ),
            Self::CavityMaterialCloseUp => CameraPose::new(
                [0.0, 1.0, 17.0],
                [-0.75, -0.5, 0.0],
                [0.0, 1.0, 0.0],
                45.0,
                0.1,
                100.0,
            ),
            Self::BoundaryCutaway => CameraPose::new(
                [-14.0, 2.0, 8.0],
                [-7.5, -1.0, 0.0],
                [0.0, 1.0, 0.0],
                45.0,
                0.1,
                100.0,
            ),
        }
    }
}

pub fn overview_to_cavity_camera_move() -> Result<DeterministicCameraMove, CameraConfigurationError>
{
    DeterministicCameraMove::new(
        CanonicalCameraPose::Overview.pose(),
        CanonicalCameraPose::CavityMaterialCloseUp.pose(),
        120,
    )
}
