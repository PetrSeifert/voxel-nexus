use super::streamed_fixture_recipe as recipe;
use super::streamed_neighbourhood::StreamedNeighbourhood;
use render_backend::{CameraState, PresentationStyle};
use std::sync::Arc;
use voxel_frontend::{
    SparseVoxelVolume, StreamedVoxelScene, StreamedVoxelVolume, VoxelCoordinate, VoxelFrontend,
    VoxelResidencyLimits, VoxelSceneId, VoxelSceneRevision, VoxelSourceError, VoxelValue,
    VoxelVolumeId, VoxelVolumeSource,
};

// Per-volume source bindings all invoke this one immutable, stateless fixture recipe.
#[cfg(feature = "qualification")]
#[allow(dead_code)]
pub(super) const FIXTURE_RECIPE_COUNT: usize = 1;

const GRID_SIDE: u32 = 16;

/// The neighbourhood that `residency_volumes` selects bounds every residency limit.
pub(super) fn frontend(neighbourhood: StreamedNeighbourhood) -> VoxelFrontend {
    VoxelFrontend::with_residency_limits(
        VoxelResidencyLimits::new(neighbourhood.volume_count())
            .expect("an odd neighbourhood side of at least three selects volumes"),
    )
}

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

/// Selects the clipped square around the camera's volume, without hysteresis.
pub(super) fn residency_volumes(
    camera: CameraState,
    neighbourhood: StreamedNeighbourhood,
) -> Vec<VoxelVolumeId> {
    let [x, _, z] = camera.eye();
    let last = GRID_SIDE - 1;
    let center = |value: f32| {
        (value / recipe::EDGE as f32)
            .floor()
            .clamp(0.0, last as f32) as u32
    };
    let (x, z) = (center(x), center(z));
    let reach = neighbourhood.reach();
    let span = move |center: u32| center.saturating_sub(reach)..=(center + reach).min(last);
    span(x)
        .flat_map(|x| span(z).map(move |z| recipe::volume_identity(x, z)))
        .collect()
}

pub(super) fn camera_far_plane(
    camera: CameraState,
    dimensions: [u32; 2],
    neighbourhood: StreamedNeighbourhood,
) -> f32 {
    let [width, height] = dimensions.map(|component| component.max(1) as f32);
    let aspect = width / height;
    let tangent = (camera.field_of_view_degrees().to_radians() * 0.5).tan();
    let corner_length = (1.0 + tangent * tangent * (aspect * aspect + 1.0)).sqrt();
    // From anywhere in its volume, the eye is at least `reach` whole volumes from the
    // neighbourhood's edge, so every orientation's frustum corners stay inside it. One voxel of
    // slack avoids demanding the next volume at an inclusive coverage edge.
    let guaranteed_reach = (neighbourhood.reach() * recipe::EDGE) as f32 - 1.0;
    guaranteed_reach / corner_length
}

pub(super) fn camera_state(
    camera: CameraState,
    dimensions: [u32; 2],
    neighbourhood: StreamedNeighbourhood,
) -> Result<CameraState, render_backend::CameraConfigurationError> {
    let far = camera_far_plane(camera, dimensions, neighbourhood);
    Ok(camera
        .with_clip_planes(0.1_f32.min(far * 0.25), far)?
        .with_radial_far_clip()
        .with_presentation_style(PresentationStyle::SCENIC))
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_frontend::{VoxelExtent, VoxelRegion, VoxelResidencySelectionId};

    const DEFAULT: StreamedNeighbourhood = StreamedNeighbourhood::DEFAULT;

    fn looking_along_x(x: f32, z: f32) -> CameraState {
        CameraState::new(
            [x, 42.0, z],
            [x + 1.0, 42.0, z],
            [0.0, 1.0, 0.0],
            60.0,
            0.1,
            32.0,
        )
        .expect("the test camera is valid")
    }

    fn square(
        x: std::ops::RangeInclusive<u32>,
        z: std::ops::RangeInclusive<u32>,
    ) -> Vec<VoxelVolumeId> {
        x.flat_map(|x| z.clone().map(move |z| recipe::volume_identity(x, z)))
            .collect()
    }

    #[test]
    fn rotating_in_place_keeps_the_same_voxel_visibility_at_the_edge_and_center()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(DEFAULT);
        frontend.publish_streamed(scene())?;
        let extent = ash::vk::Extent2D {
            width: 1601,
            height: 901,
        };
        let eye = [160.5, 42.5, 160.5];
        let edge_camera = camera_state(
            CameraState::new(eye, [161.5, 42.5, 160.5], [0.0, 1.0, 0.0], 60.0, 0.1, 32.0)?,
            [extent.width, extent.height],
            DEFAULT,
        )?;
        let edge_ray = compute_ray_render_path::camera_semantic_ray(edge_camera, extent, [0, 450])?;
        let target = std::array::from_fn(|axis| eye[axis] + edge_ray.direction()[axis] as f32);
        let center_camera = camera_state(
            CameraState::new(eye, target, [0.0, 1.0, 0.0], 60.0, 0.1, 32.0)?,
            [extent.width, extent.height],
            DEFAULT,
        )?;
        let center_ray =
            compute_ray_render_path::camera_semantic_ray(center_camera, extent, [800, 450])?;
        assert_eq!(
            residency_volumes(edge_camera, DEFAULT),
            residency_volumes(center_camera, DEFAULT)
        );
        let far = f64::from(edge_camera.far_plane());
        for distance in [far - 4.0, far + 4.0] {
            let [x, _, z]: [i32; 3] = std::array::from_fn(|axis| {
                (f64::from(eye[axis]) + edge_ray.direction()[axis] * distance).floor() as i32
            });
            let volume_edge = recipe::EDGE as i32;
            let volume =
                recipe::volume_identity((x / volume_edge) as u32, (z / volume_edge) as u32);
            let coordinate = VoxelCoordinate::new(x % volume_edge, 42, z % volume_edge);
            let view = frontend
                .edit(voxel_frontend::VoxelEditCommand::new(
                    volume.clone(),
                    coordinate,
                    VoxelValue::Occupied(voxel_frontend::VoxelMaterialId::new("stone")),
                ))?
                .view()
                .clone();
            let edge = semantic_ray_oracle::observe_along_ray(&view, &edge_ray)?;
            let center = semantic_ray_oracle::observe_along_ray(&view, &center_ray)?;
            let expected_visible = distance < far;
            assert_eq!(
                matches!(
                    center.result(),
                    semantic_ray_oracle::SemanticRayResult::Contact(_)
                ),
                expected_visible
            );
            assert_eq!(
                matches!(
                    edge.result(),
                    semantic_ray_oracle::SemanticRayResult::Contact(_)
                ),
                matches!(
                    center.result(),
                    semantic_ray_oracle::SemanticRayResult::Contact(_)
                ),
                "a voxel {distance} units away changes visibility when rotating: edge={edge:?}, center={center:?}"
            );
            frontend.edit(voxel_frontend::VoxelEditCommand::new(
                volume,
                coordinate,
                VoxelValue::Empty,
            ))?;
        }
        Ok(())
    }

    #[test]
    fn far_plane_reaches_the_neighbourhood_edge_from_the_frustum_corner()
    -> Result<(), Box<dyn std::error::Error>> {
        let camera = looking_along_x(160.0, 160.0);
        let tangent = 30.0_f32.to_radians().tan();
        let corner_length =
            |aspect: f32| (1.0 + tangent * tangent * (aspect * aspect + 1.0)).sqrt();
        for (neighbourhood, reach) in [
            (StreamedNeighbourhood::QUALIFICATION, 63.0),
            (DEFAULT, 191.0),
            (StreamedNeighbourhood::new(15)?, 447.0),
        ] {
            for dimensions in [[1920, 1080], [800, 600], [1080, 1920]] {
                let far = camera_far_plane(camera, dimensions, neighbourhood);
                let aspect = dimensions[0] as f32 / dimensions[1] as f32;
                assert!(
                    (far * corner_length(aspect) - reach).abs() < 1e-3,
                    "{neighbourhood} {dimensions:?}: {far}"
                );
            }
        }
        let far = camera_far_plane(camera, [1920, 1080], DEFAULT);
        assert!((123.0..125.0).contains(&far), "7x7 at 16:9 reaches {far}");
        let state = camera_state(camera, [1920, 1080], DEFAULT)?;
        assert_eq!(state.far_plane(), far);
        assert!(state.radial_far_clip());
        assert_eq!(state.presentation_style(), PresentationStyle::SCENIC);
        Ok(())
    }

    #[test]
    fn camera_frustum_fits_residency_at_boundaries_in_wide_and_tall_windows()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = frontend(StreamedNeighbourhood::new(15)?).publish_streamed(scene())?;
        for neighbourhood in [
            StreamedNeighbourhood::QUALIFICATION,
            DEFAULT,
            StreamedNeighbourhood::new(15)?,
        ] {
            for dimensions in [[800, 600], [1920, 400], [1920, 300], [3840, 1], [1, 3840]] {
                for eye in [
                    [130.0, 42.0, 160.0],
                    [128.0, 42.0, 128.0],
                    [191.99, 42.0, 191.99],
                    [448.0, 42.0, 511.99],
                    [0.0, 42.0, 0.0],
                    [0.0, 42.0, 512.0],
                    [1023.99, 42.0, 1023.99],
                ] {
                    for direction in [
                        [0.0, 0.0, 1.0],
                        [1.0, 0.0, 0.0],
                        [-1.0, 0.0, -1.0],
                        [1.0, -0.3, 1.0],
                        [0.001, 1.0, 0.0],
                    ] {
                        let target = std::array::from_fn(|axis| eye[axis] + direction[axis]);
                        let camera =
                            CameraState::new(eye, target, [0.0, 1.0, 0.0], 60.0, 0.1, 32.0)?;
                        let camera = camera_state(camera, dimensions, neighbourhood)?;
                        let selection = view.residency_selection(
                            VoxelResidencySelectionId::new(1),
                            residency_volumes(camera, neighbourhood),
                        )?;
                        assert!(
                            render_backend::RenderPathCoverage::new(&view, selection)?
                                .contains_camera(camera, dimensions)?,
                            "{neighbourhood} eye={eye:?} direction={direction:?} dimensions={dimensions:?}"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn production_fixture_keeps_frozen_contents_and_edits_after_eviction_and_restoration()
    -> Result<(), Box<dyn std::error::Error>> {
        let frontend = frontend(DEFAULT);
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
        for (identity, x) in [(1, 160.0), (2, 800.0), (3, 160.0)] {
            frontend.require_residency(frontend.scene_view()?.residency_selection(
                VoxelResidencySelectionId::new(identity),
                residency_volumes(looking_along_x(x, x), DEFAULT),
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
        for (eye, expected) in [
            ([0.0, 0.0], square(0..=3, 0..=3)),
            ([-20.0, -20.0], square(0..=3, 0..=3)),
            ([1023.0, 1023.0], square(12..=15, 12..=15)),
            ([2048.0, 2048.0], square(12..=15, 12..=15)),
            ([0.0, 512.0], square(0..=3, 5..=11)),
            ([1023.0, 512.0], square(12..=15, 5..=11)),
            ([512.0, 0.0], square(5..=11, 0..=3)),
            ([160.0, 160.0], square(0..=5, 0..=5)),
            ([447.99, 447.99], square(3..=9, 3..=9)),
            ([448.0, 448.0], square(4..=10, 4..=10)),
        ] {
            assert_eq!(
                residency_volumes(looking_along_x(eye[0], eye[1]), DEFAULT),
                expected,
                "eye={eye:?}"
            );
        }
        assert_eq!(
            residency_volumes(looking_along_x(448.0, 448.0), DEFAULT).len(),
            DEFAULT.volume_count()
        );
        assert_eq!(
            residency_volumes(
                looking_along_x(128.0, 128.0),
                StreamedNeighbourhood::QUALIFICATION
            ),
            square(1..=3, 1..=3)
        );
        let before = residency_volumes(looking_along_x(447.99, 447.99), DEFAULT);
        let after = residency_volumes(looking_along_x(448.0, 448.0), DEFAULT);
        assert_ne!(before, after);
        assert_eq!(
            residency_volumes(looking_along_x(447.99, 447.99), DEFAULT),
            before
        );
    }
}
