use raster_render_path::{AxisNormal, RasterRenderPath, SemanticFace, derive_raster_artifact};
use std::collections::HashSet;
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn published_view(
    extent: VoxelExtent,
    values: Vec<VoxelValue>,
) -> Result<voxel_frontend::VoxelSceneView, voxel_frontend::VoxelFrontendError> {
    let volume_identity = VoxelVolumeId::new("diagnostic");
    VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("diagnostic-scene"),
        VoxelSceneRevision::new(41),
        vec![VoxelMaterial::new(
            VoxelMaterialId::new("stone"),
            [0.25, 0.5, 0.75, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(volume_identity, extent, [10.0, 20.0, 30.0], 0.5),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    ))
}

fn diagnostic_artifacts(
    extent: VoxelExtent,
    materials: Vec<VoxelMaterial>,
    values: Vec<VoxelValue>,
) -> Result<Vec<raster_render_path::RasterArtifact>, Box<dyn std::error::Error>> {
    let volume_identity = VoxelVolumeId::new("diagnostic");
    let metadata =
        || VoxelVolumeMetadata::new(volume_identity.clone(), extent, [10.0, 20.0, 30.0], 0.5);
    let one_batch = vec![DenseVoxelBatch::new(
        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
        values.clone(),
    )];
    let [width, height, depth] = extent.dimensions();
    let width = usize::try_from(width)?;
    let height = usize::try_from(height)?;
    let mut reversed_single_voxel_batches = Vec::new();
    for coordinate_z in (0..usize::try_from(depth)?).rev() {
        for coordinate_y in (0..height).rev() {
            for coordinate_x in (0..width).rev() {
                let index = coordinate_z
                    .checked_mul(height)
                    .and_then(|value| value.checked_add(coordinate_y))
                    .and_then(|value| value.checked_mul(width))
                    .and_then(|value| value.checked_add(coordinate_x))
                    .ok_or("diagnostic index overflow")?;
                let value = values
                    .get(index)
                    .ok_or("diagnostic value was missing")?
                    .clone();
                reversed_single_voxel_batches.push(DenseVoxelBatch::new(
                    VoxelRegion::new(
                        VoxelCoordinate::new(
                            i32::try_from(coordinate_x)?,
                            i32::try_from(coordinate_y)?,
                            i32::try_from(coordinate_z)?,
                        ),
                        VoxelExtent::new(1, 1, 1),
                    ),
                    vec![value],
                ));
            }
        }
    }

    let mut artifacts = Vec::new();
    for (revision, batches) in [one_batch, reversed_single_voxel_batches]
        .into_iter()
        .enumerate()
    {
        let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
            VoxelSceneId::new("diagnostic-scene"),
            VoxelSceneRevision::new(u64::try_from(revision)? + 41),
            materials.clone(),
            vec![DenseVoxelVolume::new(metadata(), batches)],
        ))?;
        artifacts.push(derive_raster_artifact(&view, &volume_identity)?);
    }
    Ok(artifacts)
}

fn face_set(artifact: &raster_render_path::RasterArtifact) -> HashSet<SemanticFace> {
    artifact.semantic_faces().iter().cloned().collect()
}

fn semantic_face(coordinate: [i32; 3], normal: AxisNormal, material: &str) -> SemanticFace {
    let [coordinate_x, coordinate_y, coordinate_z] = coordinate;
    SemanticFace::new(
        VoxelVolumeId::new("diagnostic"),
        VoxelCoordinate::new(coordinate_x, coordinate_y, coordinate_z),
        normal,
        VoxelMaterialId::new(material),
    )
}

#[test]
fn one_voxel_produces_six_scene_space_material_colored_faces()
-> Result<(), Box<dyn std::error::Error>> {
    let material_identity = VoxelMaterialId::new("stone");
    let view = published_view(
        VoxelExtent::new(1, 1, 1),
        vec![VoxelValue::Occupied(material_identity.clone())],
    )?;

    let artifact = derive_raster_artifact(&view, &VoxelVolumeId::new("diagnostic"))?;

    assert_eq!(artifact.source_revision(), VoxelSceneRevision::new(41));
    assert_eq!(
        artifact.volume_identity(),
        &VoxelVolumeId::new("diagnostic")
    );
    assert_eq!(artifact.vertices().len(), 24);
    assert_eq!(artifact.indices().len(), 36);
    assert_eq!(artifact.vertex_byte_size(), 24 * 10 * size_of::<f32>());
    assert_eq!(artifact.index_byte_size(), 36 * size_of::<u32>());
    let expected_faces = [
        AxisNormal::NegativeX,
        AxisNormal::PositiveX,
        AxisNormal::NegativeY,
        AxisNormal::PositiveY,
        AxisNormal::NegativeZ,
        AxisNormal::PositiveZ,
    ]
    .into_iter()
    .map(|normal| {
        SemanticFace::new(
            VoxelVolumeId::new("diagnostic"),
            VoxelCoordinate::new(0, 0, 0),
            normal,
            material_identity.clone(),
        )
    })
    .collect::<HashSet<_>>();
    assert_eq!(
        artifact
            .semantic_faces()
            .iter()
            .cloned()
            .collect::<HashSet<_>>(),
        expected_faces
    );
    assert!(artifact.vertices().iter().all(|vertex| {
        vertex.linear_base_color() == [0.25, 0.5, 0.75, 1.0]
            && vertex
                .position()
                .iter()
                .zip([10.0, 20.0, 30.0])
                .all(|(value, minimum)| *value == minimum || *value == minimum + 0.5)
    }));
    assert!(artifact.indices().iter().all(|index| *index < 24));
    let (index_quads, remaining_indices) = artifact.indices().as_chunks::<6>();
    assert!(remaining_indices.is_empty());
    assert!(index_quads.iter().enumerate().all(|(quad_index, indices)| {
        let Some(first_vertex) = quad_index
            .checked_mul(4)
            .and_then(|value| u32::try_from(value).ok())
        else {
            return false;
        };
        indices
            .iter()
            .zip([0_u32, 1, 2, 0, 2, 3])
            .all(|(index, offset)| first_vertex.checked_add(offset) == Some(*index))
    }));

    Ok(())
}

#[test]
fn empty_volume_produces_an_empty_complete_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let artifacts = diagnostic_artifacts(
        VoxelExtent::new(2, 1, 1),
        vec![VoxelMaterial::new(
            VoxelMaterialId::new("stone"),
            [0.25, 0.5, 0.75, 1.0],
        )],
        vec![VoxelValue::Empty, VoxelValue::Empty],
    )?;

    assert!(artifacts.iter().all(|artifact| {
        artifact.vertices().is_empty()
            && artifact.indices().is_empty()
            && artifact.semantic_faces().is_empty()
            && artifact.vertex_byte_size() == 0
            && artifact.index_byte_size() == 0
    }));

    Ok(())
}

#[test]
fn adjacent_same_material_voxels_suppress_the_shared_faces_for_every_batch_permutation()
-> Result<(), Box<dyn std::error::Error>> {
    let stone_identity = VoxelMaterialId::new("stone");
    let artifacts = diagnostic_artifacts(
        VoxelExtent::new(2, 1, 1),
        vec![VoxelMaterial::new(
            stone_identity.clone(),
            [0.25, 0.5, 0.75, 1.0],
        )],
        vec![
            VoxelValue::Occupied(stone_identity.clone()),
            VoxelValue::Occupied(stone_identity),
        ],
    )?;
    let mut expected = HashSet::new();
    for coordinate_x in [0, 1] {
        for normal in [
            AxisNormal::NegativeY,
            AxisNormal::PositiveY,
            AxisNormal::NegativeZ,
            AxisNormal::PositiveZ,
        ] {
            expected.insert(semantic_face([coordinate_x, 0, 0], normal, "stone"));
        }
    }
    expected.insert(semantic_face([0, 0, 0], AxisNormal::NegativeX, "stone"));
    expected.insert(semantic_face([1, 0, 0], AxisNormal::PositiveX, "stone"));

    assert!(
        artifacts
            .iter()
            .all(|artifact| face_set(artifact) == expected)
    );

    Ok(())
}

#[test]
fn adjacent_different_material_voxels_suppress_shared_faces_and_resolve_each_color()
-> Result<(), Box<dyn std::error::Error>> {
    let stone_identity = VoxelMaterialId::new("stone");
    let moss_identity = VoxelMaterialId::new("moss");
    let artifacts = diagnostic_artifacts(
        VoxelExtent::new(2, 1, 1),
        vec![
            VoxelMaterial::new(stone_identity.clone(), [0.25, 0.5, 0.75, 1.0]),
            VoxelMaterial::new(moss_identity.clone(), [0.1, 0.8, 0.2, 1.0]),
        ],
        vec![
            VoxelValue::Occupied(stone_identity),
            VoxelValue::Occupied(moss_identity),
        ],
    )?;
    let mut expected = HashSet::new();
    for (coordinate_x, material) in [(0, "stone"), (1, "moss")] {
        for normal in [
            AxisNormal::NegativeY,
            AxisNormal::PositiveY,
            AxisNormal::NegativeZ,
            AxisNormal::PositiveZ,
        ] {
            expected.insert(semantic_face([coordinate_x, 0, 0], normal, material));
        }
    }
    expected.insert(semantic_face([0, 0, 0], AxisNormal::NegativeX, "stone"));
    expected.insert(semantic_face([1, 0, 0], AxisNormal::PositiveX, "moss"));

    for artifact in artifacts {
        assert_eq!(face_set(&artifact), expected);
        for face in artifact.semantic_faces() {
            let expected_color = if face.material_identity() == &VoxelMaterialId::new("stone") {
                [0.25, 0.5, 0.75, 1.0]
            } else {
                [0.1, 0.8, 0.2, 1.0]
            };
            let vertices = artifact
                .quad_vertices(face)
                .ok_or("semantic face had no quad vertices")?;
            assert!(vertices.iter().all(|vertex| {
                vertex.linear_base_color() == expected_color
                    && vertex.normal() == face.outward_normal().vector()
            }));
        }
    }

    Ok(())
}

#[test]
fn hollow_three_cube_includes_outer_boundary_and_inner_cavity_faces_for_every_batch_permutation()
-> Result<(), Box<dyn std::error::Error>> {
    let material_identity = VoxelMaterialId::new("stone");
    let mut values = vec![VoxelValue::Occupied(material_identity.clone()); 27];
    let center = values
        .get_mut(13)
        .ok_or("hollow diagnostic center missing")?;
    *center = VoxelValue::Empty;
    let artifacts = diagnostic_artifacts(
        VoxelExtent::new(3, 3, 3),
        vec![VoxelMaterial::new(
            material_identity,
            [0.25, 0.5, 0.75, 1.0],
        )],
        values,
    )?;
    let mut expected = HashSet::new();
    for coordinate_z in 0..3 {
        for coordinate_y in 0..3 {
            for coordinate_x in 0..3 {
                let coordinate = [coordinate_x, coordinate_y, coordinate_z];
                if coordinate == [1, 1, 1] {
                    continue;
                }
                for (normal, exposed) in [
                    (
                        AxisNormal::NegativeX,
                        coordinate_x == 0 || coordinate == [2, 1, 1],
                    ),
                    (
                        AxisNormal::PositiveX,
                        coordinate_x == 2 || coordinate == [0, 1, 1],
                    ),
                    (
                        AxisNormal::NegativeY,
                        coordinate_y == 0 || coordinate == [1, 2, 1],
                    ),
                    (
                        AxisNormal::PositiveY,
                        coordinate_y == 2 || coordinate == [1, 0, 1],
                    ),
                    (
                        AxisNormal::NegativeZ,
                        coordinate_z == 0 || coordinate == [1, 1, 2],
                    ),
                    (
                        AxisNormal::PositiveZ,
                        coordinate_z == 2 || coordinate == [1, 1, 0],
                    ),
                ] {
                    if exposed {
                        expected.insert(semantic_face(coordinate, normal, "stone"));
                    }
                }
            }
        }
    }
    assert_eq!(expected.len(), 60);

    assert!(
        artifacts
            .iter()
            .all(|artifact| face_set(artifact) == expected)
    );

    Ok(())
}

#[test]
fn occupied_finite_corner_emits_all_outward_boundary_faces_for_every_batch_permutation()
-> Result<(), Box<dyn std::error::Error>> {
    let material_identity = VoxelMaterialId::new("stone");
    let artifacts = diagnostic_artifacts(
        VoxelExtent::new(2, 2, 2),
        vec![VoxelMaterial::new(
            material_identity.clone(),
            [0.25, 0.5, 0.75, 1.0],
        )],
        vec![
            VoxelValue::Occupied(material_identity),
            VoxelValue::Empty,
            VoxelValue::Empty,
            VoxelValue::Empty,
            VoxelValue::Empty,
            VoxelValue::Empty,
            VoxelValue::Empty,
            VoxelValue::Empty,
        ],
    )?;
    let expected = [
        AxisNormal::NegativeX,
        AxisNormal::PositiveX,
        AxisNormal::NegativeY,
        AxisNormal::PositiveY,
        AxisNormal::NegativeZ,
        AxisNormal::PositiveZ,
    ]
    .into_iter()
    .map(|normal| semantic_face([0, 0, 0], normal, "stone"))
    .collect::<HashSet<_>>();

    assert!(
        artifacts
            .iter()
            .all(|artifact| face_set(artifact) == expected)
    );

    Ok(())
}

#[test]
fn render_path_installs_one_complete_revision_tagged_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let view = published_view(
        VoxelExtent::new(1, 1, 1),
        vec![VoxelValue::Occupied(VoxelMaterialId::new("stone"))],
    )?;
    let artifact = derive_raster_artifact(&view, &VoxelVolumeId::new("diagnostic"))?;
    let mut render_path = RasterRenderPath::new();

    render_path.install_artifact(artifact)?;

    assert_eq!(
        render_path.installed_source_revision(),
        Some(VoxelSceneRevision::new(41))
    );
    Ok(())
}
