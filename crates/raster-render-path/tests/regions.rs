use std::collections::HashSet;

use raster_render_path::{
    AxisNormal, RasterRegionResourceOwnership, RasterRenderPath, SemanticFace,
    derive_raster_regions,
};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn view(
    revision: u64,
    extent: VoxelExtent,
    occupied: &[(VoxelCoordinate, &str)],
) -> Result<voxel_frontend::VoxelSceneView, Box<dyn std::error::Error>> {
    let [width, height, depth] = extent.dimensions();
    let value_count = usize::try_from(width)?
        .checked_mul(usize::try_from(height)?)
        .and_then(|count| count.checked_mul(usize::try_from(depth).ok()?))
        .ok_or("test volume is too large")?;
    let mut values = vec![VoxelValue::Empty; value_count];
    for (coordinate, material) in occupied {
        let [coordinate_x, coordinate_y, coordinate_z] = coordinate.components();
        let x = usize::try_from(coordinate_x)?;
        let y = usize::try_from(coordinate_y)?;
        let z = usize::try_from(coordinate_z)?;
        let width = usize::try_from(width)?;
        let height = usize::try_from(height)?;
        let index = z
            .checked_mul(width.checked_mul(height).ok_or("test index overflow")?)
            .and_then(|index| index.checked_add(y.checked_mul(width)?))
            .and_then(|index| index.checked_add(x))
            .ok_or("test index overflow")?;
        let destination = values
            .get_mut(index)
            .ok_or("test coordinate is outside the volume")?;
        *destination = VoxelValue::Occupied(VoxelMaterialId::new(*material));
    }
    Ok(VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("region-scene"),
        VoxelSceneRevision::new(revision),
        vec![VoxelMaterial::new(
            VoxelMaterialId::new("stone"),
            [0.3, 0.4, 0.5, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("terrain"), extent, [0.0, 0.0, 0.0], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    ))?)
}

#[test]
fn raster_region_grid_is_zero_anchored_and_stable_across_revisions()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(5, 3, 2);
    let region_extent = VoxelExtent::new(2, 2, 2);
    let first = derive_raster_regions(&view(7, extent, &[])?, region_extent)?;
    let second = derive_raster_regions(&view(8, extent, &[])?, region_extent)?;

    let first_regions = first
        .regions()
        .iter()
        .map(|region| (region.identity().clone(), region.core()))
        .collect::<Vec<_>>();
    let second_regions = second
        .regions()
        .iter()
        .map(|region| (region.identity().clone(), region.core()))
        .collect::<Vec<_>>();
    assert_eq!(first_regions, second_regions);
    assert_eq!(first_regions.len(), 6);
    assert!(first_regions.iter().any(|(_, core)| {
        core.origin() == VoxelCoordinate::new(4, 2, 0) && core.extent() == VoxelExtent::new(1, 1, 2)
    }));
    Ok(())
}

#[test]
fn only_core_voxels_own_faces_and_the_face_halo_hides_cross_region_seams()
-> Result<(), Box<dyn std::error::Error>> {
    let artifact = derive_raster_regions(
        &view(
            11,
            VoxelExtent::new(4, 1, 1),
            &[
                (VoxelCoordinate::new(1, 0, 0), "stone"),
                (VoxelCoordinate::new(2, 0, 0), "stone"),
            ],
        )?,
        VoxelExtent::new(2, 1, 1),
    )?;

    assert_eq!(artifact.regions().len(), 2);
    let faces = artifact
        .regions()
        .iter()
        .flat_map(|region| region.semantic_faces().iter().cloned())
        .collect::<HashSet<_>>();
    assert_eq!(faces.len(), 10);
    assert!(!faces.contains(&SemanticFace::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(1, 0, 0),
        AxisNormal::PositiveX,
        VoxelMaterialId::new("stone"),
    )));
    assert!(!faces.contains(&SemanticFace::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(2, 0, 0),
        AxisNormal::NegativeX,
        VoxelMaterialId::new("stone"),
    )));
    Ok(())
}

#[test]
fn complete_installation_records_empty_regions_with_stable_identity_and_ownership()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(4, 1, 1);
    let region_extent = VoxelExtent::new(2, 1, 1);
    let first = derive_raster_regions(
        &view(21, extent, &[(VoxelCoordinate::new(0, 0, 0), "stone")])?,
        region_extent,
    )?;
    assert_eq!(first.regions().len(), 2);
    assert_eq!(
        first.regions()[1].source_revision(),
        VoxelSceneRevision::new(21)
    );
    assert!(first.regions()[1].is_empty());

    let mut render_path = RasterRenderPath::new();
    render_path.install_artifact(first);
    let first_installation = render_path.installed_regions().to_vec();
    assert_eq!(first_installation.len(), 2);
    assert_eq!(
        first_installation[0].resource_ownership(),
        RasterRegionResourceOwnership::VertexAndIndex
    );
    assert_eq!(
        first_installation[1].resource_ownership(),
        RasterRegionResourceOwnership::None
    );

    let second = derive_raster_regions(
        &view(22, extent, &[(VoxelCoordinate::new(3, 0, 0), "stone")])?,
        region_extent,
    )?;
    render_path.install_artifact(second);
    let second_installation = render_path.installed_regions();
    assert_eq!(
        first_installation[0].identity(),
        second_installation[0].identity()
    );
    assert_eq!(
        first_installation[1].identity(),
        second_installation[1].identity()
    );
    assert_eq!(
        second_installation[0].resource_ownership(),
        RasterRegionResourceOwnership::None
    );
    assert_eq!(
        second_installation[1].resource_ownership(),
        RasterRegionResourceOwnership::VertexAndIndex
    );
    assert_eq!(
        render_path.installed_source_revision(),
        Some(VoxelSceneRevision::new(22))
    );
    Ok(())
}

#[test]
fn regional_derivation_covers_every_volume_in_the_complete_scene_view()
-> Result<(), Box<dyn std::error::Error>> {
    let material_identity = VoxelMaterialId::new("stone");
    let extent = VoxelExtent::new(1, 1, 1);
    let volumes = ["terrain", "detail"]
        .into_iter()
        .map(|identity| {
            DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new(identity),
                    extent,
                    [0.0, 0.0, 0.0],
                    1.0,
                ),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    vec![VoxelValue::Occupied(material_identity.clone())],
                )],
            )
        })
        .collect();
    let complete_view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("complete-scene"),
        VoxelSceneRevision::new(31),
        vec![VoxelMaterial::new(material_identity, [0.3, 0.4, 0.5, 1.0])],
        volumes,
    ))?;

    let artifact = derive_raster_regions(&complete_view, extent)?;

    assert_eq!(artifact.volume_identity(), None);
    assert_eq!(artifact.regions().len(), 2);
    assert_eq!(artifact.semantic_faces().len(), 12);
    assert_eq!(
        artifact
            .regions()
            .iter()
            .map(|region| region.identity().volume_identity().clone())
            .collect::<HashSet<_>>(),
        HashSet::from([VoxelVolumeId::new("terrain"), VoxelVolumeId::new("detail")])
    );
    Ok(())
}

#[test]
fn empty_complete_scene_derives_an_empty_revision_tagged_collection()
-> Result<(), Box<dyn std::error::Error>> {
    let complete_view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("empty-scene"),
        VoxelSceneRevision::new(32),
        Vec::new(),
        Vec::new(),
    ))?;

    let artifact = derive_raster_regions(&complete_view, VoxelExtent::new(2, 2, 2))?;

    assert_eq!(artifact.source_revision(), VoxelSceneRevision::new(32));
    assert_eq!(artifact.volume_identity(), None);
    assert!(artifact.regions().is_empty());
    assert!(artifact.vertices().is_empty());
    assert!(artifact.indices().is_empty());
    Ok(())
}
