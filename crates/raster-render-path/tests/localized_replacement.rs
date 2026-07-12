use std::collections::{HashMap, HashSet};

use raster_render_path::{
    AxisNormal, RasterAdjacentChangeMismatch, RasterAdjacentChangeOutcome, RasterRegionIdentity,
    RasterRenderPath, SemanticFace, derive_raster_regions,
};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelEditCommand,
    VoxelEditOutcome, VoxelExtent, VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion,
    VoxelSceneId, VoxelSceneRevision, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

const NORMALS: [(AxisNormal, [i32; 3]); 6] = [
    (AxisNormal::NegativeX, [-1, 0, 0]),
    (AxisNormal::PositiveX, [1, 0, 0]),
    (AxisNormal::NegativeY, [0, -1, 0]),
    (AxisNormal::PositiveY, [0, 1, 0]),
    (AxisNormal::NegativeZ, [0, 0, -1]),
    (AxisNormal::PositiveZ, [0, 0, 1]),
];

fn frontend(
    scene: &str,
    revision: u64,
    extent: VoxelExtent,
    occupied: &HashSet<VoxelCoordinate>,
) -> Result<VoxelFrontend, Box<dyn std::error::Error>> {
    let [width, height, depth] = extent.dimensions();
    let mut values = Vec::new();
    for z in 0..depth {
        for y in 0..height {
            for x in 0..width {
                let coordinate =
                    VoxelCoordinate::new(i32::try_from(x)?, i32::try_from(y)?, i32::try_from(z)?);
                values.push(if occupied.contains(&coordinate) {
                    VoxelValue::Occupied(VoxelMaterialId::new("stone"))
                } else {
                    VoxelValue::Empty
                });
            }
        }
    }
    let frontend = VoxelFrontend::new();
    frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new(scene),
        VoxelSceneRevision::new(revision),
        vec![VoxelMaterial::new(
            VoxelMaterialId::new("stone"),
            [0.2, 0.3, 0.4, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("terrain"), extent, [0.0, 0.0, 0.0], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    ))?;
    Ok(frontend)
}

fn changed_view_and_change_set(
    outcome: &VoxelEditOutcome,
) -> Result<
    (
        &voxel_frontend::VoxelSceneView,
        &voxel_frontend::VoxelChangeSet,
    ),
    &'static str,
> {
    match outcome {
        VoxelEditOutcome::Changed { view, change_set } => Ok((view, change_set)),
        VoxelEditOutcome::Unchanged(_) => Err("test edit must change the Voxel Scene"),
    }
}

fn semantic_face_oracle(occupied: &HashSet<VoxelCoordinate>) -> HashSet<SemanticFace> {
    occupied
        .iter()
        .flat_map(|coordinate| {
            NORMALS.into_iter().filter_map(move |(normal, offset)| {
                let [x, y, z] = coordinate.components();
                let neighbor = VoxelCoordinate::new(x + offset[0], y + offset[1], z + offset[2]);
                (!occupied.contains(&neighbor)).then(|| {
                    SemanticFace::new(
                        VoxelVolumeId::new("terrain"),
                        *coordinate,
                        normal,
                        VoxelMaterialId::new("stone"),
                    )
                })
            })
        })
        .collect()
}

#[test]
fn one_adjacent_edit_replaces_only_face_neighbor_regions_with_complete_results()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(4, 4, 4);
    let region_extent = VoxelExtent::new(2, 2, 2);
    let mut occupied = HashSet::from([
        VoxelCoordinate::new(0, 0, 0),
        VoxelCoordinate::new(2, 0, 0),
        VoxelCoordinate::new(0, 2, 0),
        VoxelCoordinate::new(2, 2, 0),
        VoxelCoordinate::new(0, 0, 2),
        VoxelCoordinate::new(2, 0, 2),
        VoxelCoordinate::new(0, 2, 2),
        VoxelCoordinate::new(2, 2, 2),
    ]);
    let frontend = frontend("localized", 7, extent, &occupied)?;
    let initial_view = frontend.scene_view()?;
    let mut render_path = RasterRenderPath::new();
    render_path.install_artifact(derive_raster_regions(&initial_view, region_extent)?);
    let before = render_path
        .installed_regions()
        .iter()
        .map(|installation| (installation.identity().clone(), installation.clone()))
        .collect::<HashMap<_, _>>();

    let changed_coordinate = VoxelCoordinate::new(1, 1, 1);
    let edit = frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        changed_coordinate,
        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
    ))?;
    let (successor_view, change_set) = changed_view_and_change_set(&edit)?;
    let outcome = render_path.apply_adjacent_change(successor_view, change_set)?;

    let expected_affected = HashSet::from([
        VoxelCoordinate::new(0, 0, 0),
        VoxelCoordinate::new(2, 0, 0),
        VoxelCoordinate::new(0, 2, 0),
        VoxelCoordinate::new(0, 0, 2),
    ]);
    let RasterAdjacentChangeOutcome::Applied {
        affected_regions, ..
    } = outcome
    else {
        return Err("the adjacent edit must apply".into());
    };
    assert_eq!(
        affected_regions
            .iter()
            .map(RasterRegionIdentity::core_origin)
            .collect::<HashSet<_>>(),
        expected_affected
    );
    assert_eq!(
        render_path.installed_source_revision(),
        Some(VoxelSceneRevision::new(8))
    );

    for installation in render_path.installed_regions() {
        let prior = before
            .get(installation.identity())
            .ok_or("missing prior installation")?;
        if expected_affected.contains(&installation.identity().core_origin()) {
            assert_eq!(
                installation.installation_generation(),
                prior
                    .installation_generation()
                    .checked_successor()
                    .ok_or("test installation generation overflow")?
            );
            assert_ne!(
                installation.gpu_resource_identity(),
                prior.gpu_resource_identity()
            );
            assert_eq!(installation.activity().scheduling_events(), 1);
            assert_eq!(installation.activity().derivation_events(), 1);
            assert_eq!(installation.activity().upload_events(), 1);
            assert_eq!(installation.activity().replacement_events(), 1);
        } else {
            assert_eq!(
                installation.installation_generation(),
                prior.installation_generation()
            );
            assert_eq!(
                installation.gpu_resource_identity(),
                prior.gpu_resource_identity()
            );
            assert_eq!(installation.activity().scheduling_events(), 0);
            assert_eq!(installation.activity().derivation_events(), 0);
            assert_eq!(installation.activity().upload_events(), 0);
            assert_eq!(installation.activity().replacement_events(), 0);
        }
    }

    occupied.insert(changed_coordinate);
    assert_eq!(
        render_path
            .installed_artifact()
            .ok_or("missing installed artifact")?
            .semantic_faces()
            .iter()
            .cloned()
            .collect::<HashSet<_>>(),
        semantic_face_oracle(&occupied)
    );
    Ok(())
}

#[test]
fn applicability_mismatches_leave_the_complete_installation_unchanged()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 1, 1);
    let region_extent = VoxelExtent::new(1, 1, 1);
    let occupied = HashSet::from([VoxelCoordinate::new(0, 0, 0)]);
    let installed_frontend = frontend("installed", 10, extent, &occupied)?;
    let installed_view = installed_frontend.scene_view()?;
    let mut render_path = RasterRenderPath::new();
    render_path.install_artifact(derive_raster_regions(&installed_view, region_extent)?);
    let before_revision = render_path.installed_source_revision();
    let before_regions = render_path.installed_regions().to_vec();
    let before_faces = render_path
        .installed_artifact()
        .ok_or("missing artifact")?
        .semantic_faces()
        .to_vec();

    let other_frontend = frontend("other", 8, extent, &occupied)?;
    let other_edit = other_frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(1, 0, 0),
        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
    ))?;
    let (other_view, other_change_set) = changed_view_and_change_set(&other_edit)?;
    let outcome = render_path.apply_adjacent_change(other_view, other_change_set)?;

    let RasterAdjacentChangeOutcome::Inapplicable { mismatches } = outcome else {
        return Err("mismatched submission must be inapplicable".into());
    };
    assert!(matches!(
        mismatches.as_slice(),
        [
            RasterAdjacentChangeMismatch::SceneIdentity { .. },
            RasterAdjacentChangeMismatch::PredecessorRevision { .. },
            RasterAdjacentChangeMismatch::Adjacency { .. },
        ]
    ));
    assert_eq!(render_path.installed_source_revision(), before_revision);
    assert_eq!(render_path.installed_regions(), before_regions);
    assert_eq!(
        render_path
            .installed_artifact()
            .ok_or("missing artifact")?
            .semantic_faces(),
        before_faces
    );

    let installed_edit = installed_frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(1, 0, 0),
        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
    ))?;
    let (_, installed_change_set) = changed_view_and_change_set(&installed_edit)?;
    let outcome = render_path.apply_adjacent_change(other_view, installed_change_set)?;
    let RasterAdjacentChangeOutcome::Inapplicable { mismatches } = outcome else {
        return Err("successor mismatch must be inapplicable".into());
    };
    assert!(mismatches.iter().any(|mismatch| matches!(
        mismatch,
        RasterAdjacentChangeMismatch::SuccessorRevision { .. }
    )));
    assert_eq!(render_path.installed_source_revision(), before_revision);
    assert_eq!(render_path.installed_regions(), before_regions);
    assert_eq!(
        render_path
            .installed_artifact()
            .ok_or("missing artifact")?
            .semantic_faces(),
        before_faces
    );
    Ok(())
}

#[test]
fn an_edit_in_one_volume_retains_every_installation_in_other_volumes()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 1, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let volumes = ["terrain", "detail"]
        .into_iter()
        .map(|volume_identity| {
            DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new(volume_identity),
                    extent,
                    [0.0, 0.0, 0.0],
                    1.0,
                ),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    vec![
                        VoxelValue::Occupied(material_identity.clone()),
                        VoxelValue::Empty,
                    ],
                )],
            )
        })
        .collect();
    let frontend = VoxelFrontend::new();
    let initial_view = frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("multi-volume"),
        VoxelSceneRevision::new(3),
        vec![VoxelMaterial::new(
            material_identity.clone(),
            [0.2, 0.3, 0.4, 1.0],
        )],
        volumes,
    ))?;
    let mut render_path = RasterRenderPath::new();
    render_path.install_artifact(derive_raster_regions(
        &initial_view,
        VoxelExtent::new(1, 1, 1),
    )?);
    let detail_before = render_path
        .installed_regions()
        .iter()
        .filter(|installation| {
            installation.identity().volume_identity() == &VoxelVolumeId::new("detail")
        })
        .cloned()
        .collect::<Vec<_>>();

    let edit = frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(1, 0, 0),
        VoxelValue::Occupied(material_identity),
    ))?;
    let (successor_view, change_set) = changed_view_and_change_set(&edit)?;
    render_path.apply_adjacent_change(successor_view, change_set)?;

    let detail_after = render_path
        .installed_regions()
        .iter()
        .filter(|installation| {
            installation.identity().volume_identity() == &VoxelVolumeId::new("detail")
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(detail_after, detail_before);
    assert_eq!(
        render_path
            .installed_artifact()
            .ok_or("missing artifact")?
            .semantic_faces()
            .iter()
            .filter(|face| face.volume_identity() == &VoxelVolumeId::new("detail"))
            .count(),
        6
    );
    Ok(())
}
