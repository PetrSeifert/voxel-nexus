use ash::vk;
use raster_render_path::{
    AxisNormal, CameraPose, RasterRenderPath, SemanticFace, derive_raster_artifact,
};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

#[test]
fn camera_facing_outward_face_matches_the_configured_framebuffer_winding()
-> Result<(), Box<dyn std::error::Error>> {
    let volume_identity = VoxelVolumeId::new("diagnostic");
    let material_identity = VoxelMaterialId::new("stone");
    let extent = VoxelExtent::new(1, 1, 1);
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("diagnostic-scene"),
        VoxelSceneRevision::new(41),
        vec![VoxelMaterial::new(
            material_identity.clone(),
            [0.25, 0.5, 0.75, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(volume_identity.clone(), extent, [0.0, 0.0, 0.0], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Occupied(material_identity.clone())],
            )],
        )],
    ))?;
    let artifact = derive_raster_artifact(&view, &volume_identity)?;
    let camera_facing_face = SemanticFace::new(
        volume_identity,
        VoxelCoordinate::new(0, 0, 0),
        AxisNormal::PositiveZ,
        material_identity,
    );
    let face_index = artifact
        .semantic_faces()
        .iter()
        .position(|face| face == &camera_facing_face)
        .ok_or("the derived artifact did not contain the camera-facing positive-Z face")?;
    let first_triangle_start = face_index
        .checked_mul(6)
        .ok_or("the face index could not address its triangle")?;
    let first_triangle_end = first_triangle_start
        .checked_add(3)
        .ok_or("the triangle index range overflowed")?;
    let first_triangle_indices = artifact
        .indices()
        .get(first_triangle_start..first_triangle_end)
        .ok_or("the camera-facing face did not contain its first indexed triangle")?;
    let camera = CameraPose::new(
        [0.5, 0.5, 3.0],
        [0.5, 0.5, 0.5],
        [0.0, 1.0, 0.0],
        60.0,
        0.1,
        10.0,
    );
    let drawable_dimensions = [800, 600];
    let view_projection = camera.view_projection(drawable_dimensions)?;
    let framebuffer_vertices = first_triangle_indices
        .iter()
        .map(|index| {
            let index = usize::try_from(*index)?;
            let vertex = artifact
                .vertices()
                .get(index)
                .ok_or("the triangle referenced a missing vertex")?;
            project_to_positive_height_viewport(
                vertex.position(),
                view_projection,
                drawable_dimensions,
            )
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let [first, second, third] = framebuffer_vertices.as_slice() else {
        return Err("the indexed triangle did not contain three vertices".into());
    };
    let signed_area = (second[0] - first[0]) * (third[1] - first[1])
        - (second[1] - first[1]) * (third[0] - first[0]);
    if signed_area.abs() <= f32::EPSILON {
        return Err("the camera-facing triangle projected to zero area".into());
    }
    let projected_front_face = if signed_area < 0.0 {
        vk::FrontFace::COUNTER_CLOCKWISE
    } else {
        vk::FrontFace::CLOCKWISE
    };

    assert_eq!(RasterRenderPath::front_face(), projected_front_face);

    Ok(())
}

fn project_to_positive_height_viewport(
    position: [f32; 3],
    view_projection: [f32; 16],
    drawable_dimensions: [u32; 2],
) -> Result<[f32; 2], Box<dyn std::error::Error>> {
    let [position_x, position_y, position_z] = position;
    let clip_x = view_projection[0] * position_x
        + view_projection[4] * position_y
        + view_projection[8] * position_z
        + view_projection[12];
    let clip_y = view_projection[1] * position_x
        + view_projection[5] * position_y
        + view_projection[9] * position_z
        + view_projection[13];
    let clip_w = view_projection[3] * position_x
        + view_projection[7] * position_y
        + view_projection[11] * position_z
        + view_projection[15];
    if !clip_w.is_finite() || clip_w.abs() <= f32::EPSILON {
        return Err("the projected vertex had an invalid homogeneous coordinate".into());
    }
    let [width, height] = drawable_dimensions.map(|dimension| dimension as f32);

    Ok([
        (clip_x / clip_w + 1.0) * width * 0.5,
        (clip_y / clip_w + 1.0) * height * 0.5,
    ])
}
