use crate::{CameraConfigurationError, CameraState};
use std::sync::Arc;
use voxel_frontend::{
    VoxelFrontendError, VoxelResidencySelection, VoxelSceneView, VoxelVolumeMetadata,
};

/// Coverage retains metadata only, so checking a camera cannot materialize voxel contents.
#[derive(Clone, Debug)]
pub struct RenderPathCoverage {
    volumes: Arc<[VoxelVolumeMetadata]>,
    installed: VoxelResidencySelection,
}

impl RenderPathCoverage {
    pub fn new(
        view: &VoxelSceneView,
        installed: VoxelResidencySelection,
    ) -> Result<Self, VoxelFrontendError> {
        if installed.scene_id() != view.scene_id() {
            return Err(VoxelFrontendError::ResidencySceneMismatch);
        }
        for identity in installed.volumes() {
            view.volume_content_version(identity)?;
        }
        Ok(Self {
            volumes: view.volumes().into(),
            installed,
        })
    }

    pub fn installed_selection(&self) -> &VoxelResidencySelection {
        &self.installed
    }

    pub fn contains_camera(
        &self,
        camera: CameraState,
        dimensions: [u32; 2],
    ) -> Result<bool, CameraConfigurationError> {
        let [width, height] = dimensions;
        if width == 0 || height == 0 {
            return Err(CameraConfigurationError::ZeroDrawableExtent);
        }
        let eye = camera.eye().map(f64::from);
        let forward = normalize(std::array::from_fn(|axis| {
            f64::from(camera.target()[axis]) - eye[axis]
        }));
        let side = normalize(cross(forward, camera.up().map(f64::from)));
        let upward = cross(side, forward);
        let tangent = (f64::from(camera.field_of_view_degrees()).to_radians() * 0.5).tan();
        let aspect = f64::from(width) / f64::from(height);
        let mut minimum = [f64::INFINITY; 3];
        let mut maximum = [f64::NEG_INFINITY; 3];
        for distance in [camera.near_plane(), camera.far_plane()].map(f64::from) {
            for horizontal in [-1.0, 1.0] {
                for vertical in [-1.0, 1.0] {
                    for axis in 0..3 {
                        let corner = eye[axis]
                            + distance
                                * (forward[axis]
                                    + tangent
                                        * (horizontal * aspect * side[axis]
                                            + vertical * upward[axis]));
                        minimum[axis] = minimum[axis].min(corner);
                        maximum[axis] = maximum[axis].max(corner);
                    }
                }
            }
        }
        let mut scene_minimum = [f64::INFINITY; 3];
        let mut scene_maximum = [f64::NEG_INFINITY; 3];
        for volume in self.volumes.iter() {
            let (origin, end) = volume_bounds(volume);
            for axis in 0..3 {
                scene_minimum[axis] = scene_minimum[axis].min(origin[axis]);
                scene_maximum[axis] = scene_maximum[axis].max(end[axis]);
            }
        }
        for axis in 0..3 {
            minimum[axis] = minimum[axis].max(scene_minimum[axis]);
            maximum[axis] = maximum[axis].min(scene_maximum[axis]);
            if minimum[axis] > maximum[axis] {
                return Ok(true);
            }
        }
        // Every intersected volume is needed, including overlapping volumes and
        // volumes in the conservative bounds but outside the frustum itself.
        Ok(self.volumes.iter().all(|volume| {
            let (origin, end) = volume_bounds(volume);
            self.installed.volumes().contains(volume.identity())
                || (0..3).any(|axis| end[axis] < minimum[axis] || origin[axis] > maximum[axis])
        }))
    }
}

fn volume_bounds(volume: &VoxelVolumeMetadata) -> ([f64; 3], [f64; 3]) {
    let origin = volume.scene_origin().map(f64::from);
    let extent = volume.extent().dimensions();
    let end = std::array::from_fn(|axis| {
        origin[axis] + f64::from(extent[axis]) * f64::from(volume.voxel_size())
    });
    (origin, end)
}

fn normalize(vector: [f64; 3]) -> [f64; 3] {
    let length = vector
        .into_iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    vector.map(|value| value / length)
}

fn cross([left_x, left_y, left_z]: [f64; 3], [right_x, right_y, right_z]: [f64; 3]) -> [f64; 3] {
    [
        left_y * right_z - left_z * right_y,
        left_z * right_x - left_x * right_z,
        left_x * right_y - left_y * right_x,
    ]
}
