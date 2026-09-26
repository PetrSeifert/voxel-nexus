use super::*;
use semantic_ray_oracle::SemanticRay;
use semantic_ray_oracle::SemanticRayProbe;
use voxel_frontend::VoxelCoordinate;
use voxel_frontend::VoxelSceneId;
use voxel_frontend::VoxelSceneRevision;

#[test]
fn path_neutral_edit_submission_starts_convergence_without_external_control()
-> Result<(), Box<dyn std::error::Error>> {
    use voxel_frontend::{
        DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelEditCommand, VoxelExtent,
        VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelValue, VoxelVolumeId,
        VoxelVolumeMetadata,
    };
    let frontend = VoxelFrontend::new();
    let extent = VoxelExtent::new(1, 1, 1);
    let view = frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("compute-convergence"),
        VoxelSceneRevision::new(7),
        vec![VoxelMaterial::new(
            VoxelMaterialId::new("stone"),
            [0.2, 0.3, 0.4, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("terrain"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Empty],
            )],
        )],
    ))?;
    let camera = CameraState::new(
        [2.0, 2.0, 2.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        50.0,
        0.1,
        100.0,
    )?;
    let mut adapter = ComputeRayRenderPathAdapter::new(view, camera, CameraStateRevision::new(3))?;
    let outcome = frontend.edit(VoxelEditCommand::new(
        VoxelVolumeId::new("terrain"),
        VoxelCoordinate::new(0, 0, 0),
        VoxelValue::Occupied(VoxelMaterialId::new("stone")),
    ))?;

    let path: &mut dyn RenderPath = &mut adapter;
    path.submit_edit_outcome(outcome)
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    adapter
        .render_path
        .convergence
        .apply_controlled_request_at_frame_boundary()?;
    assert_eq!(
        adapter.stamp().required_revision(),
        VoxelSceneRevision::new(8)
    );
    assert_eq!(
        adapter.stamp().visible_revision(),
        VoxelSceneRevision::new(7)
    );
    assert_eq!(adapter.convergence_status().worker_count(), 1);
    Ok(())
}

#[test]
fn hidden_gpu_candidate_release_clears_control_and_owned_resources()
-> Result<(), Box<dyn std::error::Error>> {
    use voxel_frontend::{DenseVoxelScene, VoxelFrontend, VoxelSceneId};

    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("hidden-release"),
        VoxelSceneRevision::new(1),
        Vec::new(),
        Vec::new(),
    ))?;
    let bundle = ComputeSceneBundle::from_view(&view)?;
    let mut render_path = ComputeRayRenderPath::new(
        bundle,
        CameraState::new(
            [2.0, 2.0, 2.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            50.0,
            0.1,
            100.0,
        )?,
        None,
    );
    let controller = render_path.convergence.enable_control(true);
    render_path.convergence_control = Some(controller.clone());
    let installed = render_path.convergence.status().installed();
    render_path.hidden_scene_gpu_resources = Some(ComputeHiddenSceneGpuResources {
        stamp: installed,
        resources: Some(ComputeSceneGpuResources {
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            allocation_bytes: 0,
        }),
        range: 0,
    });
    assert!(controller.hold_post_upload(installed.revision())?);
    let mut release_count = 0;

    render_path.release_hidden_scene_gpu_resources_with(|resources| {
        assert_eq!(resources.buffer, vk::Buffer::null());
        assert_eq!(resources.memory, vk::DeviceMemory::null());
        release_count += 1;
    })?;

    assert_eq!(release_count, 1);
    assert!(render_path.hidden_scene_gpu_resources.is_none());
    assert_eq!(controller.post_upload_revision()?, None);
    Ok(())
}

#[test]
fn first_dispatch_discards_undefined_contents_before_compute_writes() {
    assert_eq!(
        output_barrier_plan(false),
        OutputBarrierPlan {
            source_stage: vk::PipelineStageFlags::TOP_OF_PIPE,
            source_access: vk::AccessFlags::empty(),
            old_layout: vk::ImageLayout::UNDEFINED,
        }
    );
}

#[test]
fn later_dispatch_waits_for_the_previous_composite_read() {
    assert_eq!(
        output_barrier_plan(true),
        OutputBarrierPlan {
            source_stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            source_access: vk::AccessFlags::SHADER_READ,
            old_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }
    );
}

#[test]
fn compute_writes_become_visible_to_fragment_sampling() {
    assert_eq!(
        sampling_barrier_plan(),
        SamplingBarrierPlan {
            source_stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            destination_stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            source_access: vk::AccessFlags::SHADER_WRITE,
            destination_access: vk::AccessFlags::SHADER_READ,
            old_layout: vk::ImageLayout::GENERAL,
            new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }
    );
}

#[test]
fn compute_shader_guards_only_out_of_range_invocations_and_writes_every_valid_pixel() {
    let shader = include_str!("../shaders/dense_dda.comp");
    assert!(shader.contains("writeonly image2D output_image"));
    assert!(shader.contains("greaterThanEqual(pixel, dimensions)"));
    assert!(shader.contains("readonly buffer SceneData"));
    assert!(shader.contains("Hit trace_volume"));
    assert!(shader.contains("bvec3 tied"));
    assert!(shader.contains("imageStore(output_image, pixel"));
    assert!(shader.contains("binding = 4) buffer SemanticRayData"));
    assert!(shader.contains("Hit nearest = trace_scene"));
    assert!(shader.contains("observe_semantic_ray(probe_index)"));
}

#[test]
fn semantic_ray_readback_decodes_installed_scene_identities_and_gpu_contact_data()
-> Result<(), Box<dyn std::error::Error>> {
    use semantic_ray_oracle::{SemanticRayDistanceTolerance, observe};
    use voxel_frontend::{
        DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelExtent, VoxelFrontend,
        VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelValue, VoxelVolumeId,
        VoxelVolumeMetadata,
    };

    let material_identity = VoxelMaterialId::new("stone");
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("gpu-observation"),
        VoxelSceneRevision::new(9),
        vec![VoxelMaterial::new(material_identity.clone(), [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("volume"),
                VoxelExtent::new(1, 1, 1),
                [0.0; 3],
                1.0,
            ),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(1, 1, 1)),
                vec![VoxelValue::Occupied(material_identity)],
            )],
        )],
    ))?;
    let bundle = ComputeSceneBundle::from_view(&view)?;
    let probe = SemanticRayProbe::new(
        "entered",
        SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?,
    )?;
    let mut words = semantic_ray_input_words(std::slice::from_ref(&probe));
    let output = &mut words
        [SEMANTIC_RAY_OUTPUT_START..SEMANTIC_RAY_OUTPUT_START + SEMANTIC_RAY_OUTPUT_WORD_COUNT];
    output.copy_from_slice(&[1, 0, 0, 0, 0, 1, 1.0_f32.to_bits(), 1]);

    let decoded = decode_semantic_ray_output(
        std::slice::from_ref(&probe),
        &words,
        &bundle,
        view.revision(),
        37,
    )?;
    let observation = decoded.first().ok_or("missing decoded observation")?;
    let oracle = observe(&view, probe.ray())?;
    let tolerance = SemanticRayDistanceTolerance::new(1.0e-6)?;

    assert_eq!(observation.probe_identity(), "entered");
    assert_eq!(observation.frame_sequence(), 37);
    assert!(observation.observation().agrees_with(&oracle, tolerance));
    Ok(())
}

#[test]
fn composite_samples_the_compute_output_over_a_full_screen_triangle() {
    let vertex_shader = include_str!("../shaders/composite.vert");
    let fragment_shader = include_str!("../shaders/composite.frag");
    assert!(vertex_shader.contains("vec2( 3.0, -1.0)"));
    assert!(vertex_shader.contains("vec2(-1.0,  3.0)"));
    assert!(fragment_shader.contains("sampler2D computed_image"));
    assert!(fragment_shader.contains("texture(computed_image, texture_coordinate)"));
}
