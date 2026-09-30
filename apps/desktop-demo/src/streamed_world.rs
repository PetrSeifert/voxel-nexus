use super::streamed_fixture_recipe as recipe;
use render_backend::CameraState;
use std::sync::Arc;
use voxel_frontend::{
    SparseVoxelVolume, StreamedVoxelScene, StreamedVoxelVolume, VoxelCoordinate, VoxelSceneId,
    VoxelSceneRevision, VoxelSourceError, VoxelValue, VoxelVolumeId, VoxelVolumeSource,
};

// Per-volume source bindings all invoke this one immutable, stateless fixture recipe.
#[cfg(feature = "qualification")]
#[allow(dead_code)]
pub(super) const FIXTURE_RECIPE_COUNT: usize = 1;

pub(super) fn scene() -> StreamedVoxelScene {
    scene_with_side(16)
}

pub(super) fn scene_with_side(side: u32) -> StreamedVoxelScene {
    let identity = VoxelSceneId::new("streamed-qualification-v1");
    let volumes = recipe::catalog(side)
        .into_iter()
        .map(|metadata| {
            let [x, _, z] = metadata.scene_origin();
            StreamedVoxelVolume::new(
                metadata,
                Arc::new(FixtureSource {
                    scene: identity.clone(),
                    x: x as u32 / recipe::EDGE,
                    z: z as u32 / recipe::EDGE,
                }),
            )
        })
        .collect();
    StreamedVoxelScene::new(
        identity,
        VoxelSceneRevision::new(1),
        recipe::materials(),
        volumes,
    )
}

struct FixtureSource {
    scene: VoxelSceneId,
    x: u32,
    z: u32,
}

impl VoxelVolumeSource for FixtureSource {
    fn scene_id(&self) -> &VoxelSceneId {
        &self.scene
    }
    fn requires_materials(&self) -> bool {
        true
    }
    fn value(&self, coordinate: VoxelCoordinate) -> Result<VoxelValue, VoxelSourceError> {
        let [x, y, z] = coordinate.components();
        Ok(recipe::material(recipe::generated_code(x, y, z)))
    }
    fn materialize(&self) -> Result<SparseVoxelVolume, VoxelSourceError> {
        Ok(recipe::volume_contents(self.x, self.z, false))
    }
}

pub(super) fn residency_volumes(camera: CameraState) -> Vec<VoxelVolumeId> {
    let [x, _, z] = camera.eye();
    let center = |value: f32| (value / recipe::EDGE as f32).floor().clamp(0.0, 15.0) as u32;
    let (x, z) = (center(x), center(z));
    (x.saturating_sub(1)..=(x + 1).min(15))
        .flat_map(|x| {
            (z.saturating_sub(1)..=(z + 1).min(15)).map(move |z| recipe::volume_identity(x, z))
        })
        .collect()
}

pub(super) fn camera_far_plane(camera: CameraState, dimensions: [u32; 2]) -> f32 {
    let [width, height] = dimensions.map(|component| component.max(1) as f32);
    let aspect = width / height;
    let tangent = (camera.field_of_view_degrees().to_radians() * 0.5).tan();
    let corner_length = (1.0 + tangent * tangent * (aspect * aspect + 1.0)).sqrt();
    // Every orientation fits within one neighbouring volume, including at a cell boundary.
    // One voxel of slack avoids demanding the next volume at an inclusive coverage edge.
    (63.0 / corner_length).min(32.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_frontend::{VoxelExtent, VoxelFrontend, VoxelRegion, VoxelResidencySelectionId};

    #[test]
    fn camera_frustum_fits_residency_at_boundaries_in_wide_and_tall_windows()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = VoxelFrontend::new().publish_streamed(scene())?;
        for dimensions in [[800, 600], [1920, 400], [1920, 300], [3840, 1], [1, 3840]] {
            for eye in [
                [130.0, 42.0, 160.0],
                [128.0, 42.0, 128.0],
                [191.99, 42.0, 191.99],
                [0.0, 42.0, 0.0],
            ] {
                for direction in [
                    [0.0, 0.0, 1.0],
                    [1.0, 0.0, 0.0],
                    [-1.0, 0.0, -1.0],
                    [0.001, 1.0, 0.0],
                ] {
                    let target = std::array::from_fn(|axis| eye[axis] + direction[axis]);
                    let camera = CameraState::new(eye, target, [0.0, 1.0, 0.0], 60.0, 0.1, 32.0)?;
                    let far = camera_far_plane(camera, dimensions);
                    let camera = CameraState::new(
                        eye,
                        target,
                        [0.0, 1.0, 0.0],
                        60.0,
                        0.1_f32.min(far * 0.25),
                        far,
                    )?;
                    let selection = view.residency_selection(
                        VoxelResidencySelectionId::new(1),
                        residency_volumes(camera),
                    )?;
                    assert!(
                        render_backend::RenderPathCoverage::new(&view, selection)?
                            .contains_camera(camera, dimensions)?,
                        "eye={eye:?} direction={direction:?} dimensions={dimensions:?}"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn production_fixture_keeps_frozen_contents_and_edits_after_eviction_and_restoration()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = VoxelFrontend::new();
        let original = frontend.publish_streamed(scene())?;
        assert!(original.is_streamed());
        assert_eq!(original.volumes().len(), 256);
        assert_eq!(frontend.materialization_cache_stats()?.copies, 0);
        let volume = recipe::volume_identity(2, 2);
        let fingerprint = |view: &voxel_frontend::VoxelSceneView| {
            let samples = view.read_region(
                &volume,
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(64, 64, 64)),
            )?;
            Ok::<_, voxel_frontend::VoxelFrontendError>(samples.iter().fold(
                0xcbf2_9ce4_8422_2325_u64,
                |hash, sample| {
                    let code = match sample.value() {
                        VoxelValue::Empty => 0,
                        VoxelValue::Occupied(identity)
                            if identity == &voxel_frontend::VoxelMaterialId::new("stone") =>
                        {
                            1
                        }
                        VoxelValue::Occupied(_) => 2,
                    };
                    (hash ^ code).wrapping_mul(0x100_0000_01b3)
                },
            ))
        };
        assert_eq!(fingerprint(&original)?, 0x9660_d930_8d69_ede5);
        let camera = |x| {
            CameraState::new(
                [x, 42.0, x],
                [x + 1.0, 42.0, x],
                [0.0, 1.0, 0.0],
                60.0,
                0.1,
                32.0,
            )
            .unwrap()
        };
        for (identity, x) in [(1, 160.0), (2, 800.0), (3, 160.0)] {
            frontend.require_residency(frontend.scene_view()?.residency_selection(
                VoxelResidencySelectionId::new(identity),
                residency_volumes(camera(x)),
            )?)?;
            assert!(frontend.establish_residency()?);
            if identity == 1 {
                frontend.edit(recipe::edit(2, 2, false))?;
            }
        }
        assert_eq!(fingerprint(&frontend.scene_view()?)?, 0x478e_e4a9_737e_1ad7);
        assert_eq!(fingerprint(&original)?, 0x9660_d930_8d69_ede5);
        frontend.edit(recipe::edit(2, 2, true))?;
        assert_eq!(fingerprint(&frontend.scene_view()?)?, 0x9660_d930_8d69_ede5);
        Ok(())
    }

    #[test]
    fn residency_clips_at_corners_edges_and_diagonal_crossings_without_hysteresis() {
        let camera = |x, z| {
            CameraState::new(
                [x, 42.0, z],
                [x + 1.0, 42.0, z],
                [0.0, 1.0, 0.0],
                60.0,
                0.1,
                32.0,
            )
            .unwrap()
        };
        for (eye, expected) in [
            (
                [0.0, 0.0],
                vec![
                    "volume-00-00",
                    "volume-00-01",
                    "volume-01-00",
                    "volume-01-01",
                ],
            ),
            (
                [1023.0, 1023.0],
                vec![
                    "volume-14-14",
                    "volume-14-15",
                    "volume-15-14",
                    "volume-15-15",
                ],
            ),
            (
                [0.0, 96.0],
                vec![
                    "volume-00-00",
                    "volume-00-01",
                    "volume-00-02",
                    "volume-01-00",
                    "volume-01-01",
                    "volume-01-02",
                ],
            ),
            (
                [127.99, 127.99],
                vec![
                    "volume-00-00",
                    "volume-00-01",
                    "volume-00-02",
                    "volume-01-00",
                    "volume-01-01",
                    "volume-01-02",
                    "volume-02-00",
                    "volume-02-01",
                    "volume-02-02",
                ],
            ),
            (
                [128.0, 128.0],
                vec![
                    "volume-01-01",
                    "volume-01-02",
                    "volume-01-03",
                    "volume-02-01",
                    "volume-02-02",
                    "volume-02-03",
                    "volume-03-01",
                    "volume-03-02",
                    "volume-03-03",
                ],
            ),
            (
                [-20.0, -20.0],
                vec![
                    "volume-00-00",
                    "volume-00-01",
                    "volume-01-00",
                    "volume-01-01",
                ],
            ),
            (
                [2048.0, 2048.0],
                vec![
                    "volume-14-14",
                    "volume-14-15",
                    "volume-15-14",
                    "volume-15-15",
                ],
            ),
        ] {
            assert_eq!(
                residency_volumes(camera(eye[0], eye[1])),
                expected
                    .into_iter()
                    .map(VoxelVolumeId::new)
                    .collect::<Vec<_>>()
            );
        }
        let before = residency_volumes(camera(127.99, 127.99));
        let after = residency_volumes(camera(128.0, 128.0));
        assert_ne!(before, after);
        assert_eq!(residency_volumes(camera(127.99, 127.99)), before);
    }
}
