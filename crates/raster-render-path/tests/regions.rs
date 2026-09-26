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
fn explicit_flattening_rebases_indices_and_preserves_region_face_order()
-> Result<(), Box<dyn std::error::Error>> {
    let artifact = derive_raster_regions(
        &view(
            7,
            VoxelExtent::new(6, 1, 1),
            &[
                (VoxelCoordinate::new(0, 0, 0), "stone"),
                (VoxelCoordinate::new(4, 0, 0), "stone"),
            ],
        )?,
        VoxelExtent::new(2, 1, 1),
    )?;
    let geometry = artifact.flatten_geometry()?;
    assert_eq!(geometry.vertices().len(), artifact.vertex_count());
    assert_eq!(geometry.indices().len(), artifact.index_count());
    assert_eq!(
        geometry.semantic_faces().len(),
        artifact.semantic_face_count()
    );
    assert!(
        geometry
            .semantic_faces()
            .iter()
            .eq(artifact.semantic_faces())
    );
    let mut vertex_offset = 0;
    let mut index_offset = 0;
    for region in artifact.regions() {
        assert_eq!(
            geometry
                .vertices()
                .get(vertex_offset..vertex_offset + region.vertices().len()),
            Some(
                region
                    .vertices()
                    .iter()
                    .map(|vertex| region.decode_vertex(vertex).ok_or("invalid packed vertex"))
                    .collect::<Result<Vec<_>, _>>()?
                    .as_slice()
            )
        );
        for (index, local_index) in region.indices().iter().enumerate() {
            assert_eq!(
                geometry.indices().get(index_offset + index),
                Some(&(u32::try_from(vertex_offset)? + local_index))
            );
        }
        for face in region.semantic_faces() {
            let face_index = geometry
                .semantic_faces()
                .iter()
                .position(|candidate| candidate == face)
                .ok_or("missing flattened face")?;
            assert_eq!(
                artifact.quad_vertices(face).as_deref(),
                geometry.vertices().get(face_index * 4..face_index * 4 + 4)
            );
        }
        vertex_offset += region.vertices().len();
        index_offset += region.indices().len();
    }
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
    assert_eq!(artifact.semantic_face_count(), 12);
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
    assert!(artifact.vertex_count() == 0);
    assert!(artifact.index_count() == 0);
    Ok(())
}

#[test]
fn greedy_faces_stop_at_region_boundaries_and_decode_nonzero_origins()
-> Result<(), Box<dyn std::error::Error>> {
    let occupied = (0..4)
        .map(|coordinate| (VoxelCoordinate::new(coordinate, 0, 0), "stone"))
        .collect::<Vec<_>>();
    let artifact = derive_raster_regions(
        &view(1, VoxelExtent::new(4, 1, 1), &occupied)?,
        VoxelExtent::new(2, 1, 1),
    )?;
    assert_eq!(artifact.vertex_count(), 40);
    assert_eq!(artifact.semantic_face_count(), 18);
    for region in artifact.regions() {
        let origin = region.core().origin().components()[0];
        assert_eq!(region.vertices().len(), 20);
        for vertex in region.vertices() {
            assert!(vertex.local_position()[0] <= 2);
            let position = region
                .decode_vertex(vertex)
                .ok_or("invalid packed vertex")?
                .position();
            assert!((origin as f32..=(origin + 2) as f32).contains(&position[0]));
        }
    }
    Ok(())
}
