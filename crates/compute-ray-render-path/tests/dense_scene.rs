use ash::vk;
use canonical_scene::{
    CanonicalSceneScale, canonical_edit_semantic_ray_probes, generate_canonical_scene,
};
use compute_ray_render_path::{ComputeSceneBundle, camera_semantic_ray};
use render_backend::CameraState;
use semantic_ray_oracle::{SemanticRay, SemanticRayDistanceTolerance, observe};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelEditCommand,
    VoxelExtent, VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId,
    VoxelSceneRevision, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn assert_bundle_agrees_with_oracle(
    bundle: &ComputeSceneBundle,
    view: &voxel_frontend::VoxelSceneView,
    ray: &SemanticRay,
) -> Result<(), Box<dyn std::error::Error>> {
    let tolerance = SemanticRayDistanceTolerance::new(1.0e-9)?;
    let compute = bundle.observe(ray);
    let oracle = observe(view, ray)?;
    if !compute.agrees_with(&oracle, tolerance) {
        return Err(
            format!("compute observation {compute:?} disagreed with oracle {oracle:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn bounded_region_reads_build_flattened_words_and_a_local_material_table()
-> Result<(), Box<dyn std::error::Error>> {
    let warm = VoxelMaterialId::new("warm");
    let cool = VoxelMaterialId::new("cool");
    let extent = VoxelExtent::new(33, 1, 1);
    let mut values = vec![VoxelValue::Empty; 33];
    *values.get_mut(0).ok_or("missing first voxel")? = VoxelValue::Occupied(warm.clone());
    *values.get_mut(32).ok_or("missing last voxel")? = VoxelValue::Occupied(cool.clone());
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("flattened"),
        VoxelSceneRevision::new(7),
        vec![
            VoxelMaterial::new(warm.clone(), [0.8, 0.4, 0.2, 1.0]),
            VoxelMaterial::new(cool.clone(), [0.1, 0.3, 0.9, 1.0]),
        ],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("terrain"),
                extent,
                [10.0, 20.0, 30.0],
                0.5,
            ),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    ))?;

    let bundle = ComputeSceneBundle::from_view(&view)?;

    assert_eq!(bundle.scene_identity(), &VoxelSceneId::new("flattened"));
    assert_eq!(bundle.revision(), VoxelSceneRevision::new(7));
    assert_eq!(bundle.material_identities(), &[warm, cool]);
    assert_eq!(bundle.material_words().len(), 8);
    assert_eq!(bundle.voxel_words().len(), 33);
    assert_eq!(bundle.voxel_words()[0], 1);
    assert!(bundle.voxel_words()[1..32].iter().all(|word| *word == 0));
    assert_eq!(bundle.voxel_words()[32], 2);
    let header = bundle
        .volume_headers()
        .first()
        .ok_or("missing volume header")?;
    assert_eq!(header.identity(), &VoxelVolumeId::new("terrain"));
    assert_eq!(header.scene_origin(), [10.0, 20.0, 30.0]);
    assert_eq!(header.voxel_size(), 0.5);
    assert_eq!(header.extent(), extent);
    assert_eq!(header.voxel_word_offset(), 0);
    assert_eq!(&bundle.storage_words()[0..4], &[1, 2, 12, 20]);
    Ok(())
}

#[test]
fn dense_dda_matches_clipping_half_open_and_simultaneous_axis_rules()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 2, 2);
    let material = VoxelMaterialId::new("stone");
    let mut values = vec![VoxelValue::Empty; 8];
    *values.get_mut(7).ok_or("missing diagonal voxel")? = VoxelValue::Occupied(material.clone());
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("dda-rules"),
        VoxelSceneRevision::new(2),
        vec![VoxelMaterial::new(material, [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("volume"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    ))?;
    let bundle = ComputeSceneBundle::from_view(&view)?;
    let rays = [
        SemanticRay::new([-1.0; 3], [1.0; 3], 0.0, 6.0)?,
        SemanticRay::new([2.0, 1.5, 1.5], [-1.0, 0.0, 0.0], 0.0, 2.0)?,
        SemanticRay::new([1.0, 1.5, 1.5], [-1.0, 0.0, 0.0], 0.0, 2.0)?,
        SemanticRay::new([-2.0, 1.5, 1.5], [1.0, 0.0, 0.0], 0.0, 1.5)?,
        SemanticRay::new([-1.0, 2.0, 1.5], [1.0, 0.0, 0.0], 0.0, 4.0)?,
    ];
    for ray in &rays {
        assert_bundle_agrees_with_oracle(&bundle, &view, ray)?;
    }
    Ok(())
}

#[test]
fn equal_distance_contacts_use_stable_volume_identity_order()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(1, 1, 1);
    let material = VoxelMaterialId::new("stone");
    let volumes = ["zeta", "alpha"]
        .into_iter()
        .map(|identity| {
            DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(VoxelVolumeId::new(identity), extent, [0.0; 3], 1.0),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                    vec![VoxelValue::Occupied(material.clone())],
                )],
            )
        })
        .collect();
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("overlap"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material, [1.0; 4])],
        volumes,
    ))?;
    let bundle = ComputeSceneBundle::from_view(&view)?;
    assert_eq!(
        bundle.volume_headers()[0].identity(),
        &VoxelVolumeId::new("alpha")
    );
    let ray = SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?;
    assert_bundle_agrees_with_oracle(&bundle, &view, &ray)?;
    Ok(())
}

#[test]
fn canonical_declared_probes_match_the_oracle_at_revisions_one_and_four()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::new();
    let revision_one =
        frontend.publish(generate_canonical_scene(CanonicalSceneScale::Large)?.into_scene())?;
    let probes = canonical_edit_semantic_ray_probes()?;
    let revision_one_bundle = ComputeSceneBundle::from_view(&revision_one)?;
    for probe in &probes {
        assert_bundle_agrees_with_oracle(&revision_one_bundle, &revision_one, probe.ray())?;
    }
    drop(revision_one_bundle);

    for coordinate in [
        VoxelCoordinate::new(0, 0, 0),
        VoxelCoordinate::new(40, 0, 0),
        VoxelCoordinate::new(80, 0, 0),
    ] {
        frontend.edit(VoxelEditCommand::new(
            VoxelVolumeId::new("canonical-volume"),
            coordinate,
            VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm")),
        ))?;
    }
    let revision_four = frontend.scene_view()?;
    let revision_four_bundle = ComputeSceneBundle::from_view(&revision_four)?;
    for probe in &probes {
        assert_bundle_agrees_with_oracle(&revision_four_bundle, &revision_four, probe.ray())?;
    }
    Ok(())
}

#[test]
fn shared_camera_state_generates_pixel_center_rays_and_view_space_clipping()
-> Result<(), Box<dyn std::error::Error>> {
    let camera = CameraState::new(
        [0.0, 0.0, 5.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        50.0,
        0.1,
        100.0,
    )?;
    let extent = vk::Extent2D {
        width: 101,
        height: 51,
    };

    let centre = camera_semantic_ray(camera, extent, [50, 25])?;

    assert_eq!(centre.origin(), [0.0, 0.0, 5.0]);
    assert_eq!(centre.direction(), [0.0, 0.0, -1.0]);
    assert!((centre.minimum_distance() - 0.1).abs() <= f64::from(f32::EPSILON));
    assert!((centre.maximum_distance() - 100.0).abs() <= f64::from(f32::EPSILON));
    assert!(camera_semantic_ray(camera, extent, [101, 0]).is_err());
    Ok(())
}

#[test]
fn dense_blocks_preserve_rows_across_all_axes_and_volume_offsets()
-> Result<(), Box<dyn std::error::Error>> {
    let material = VoxelMaterialId::new("stone");
    let mut volumes = Vec::new();
    let mut expected_words = Vec::new();
    for (name, extent) in [
        ("first", VoxelExtent::new(35, 34, 33)),
        ("second", VoxelExtent::new(3, 2, 4)),
    ] {
        let [width, height, depth] = extent.dimensions();
        let mut values = Vec::new();
        for z in 0..depth {
            for y in 0..height {
                for x in 0..width {
                    let occupied = (x + 2 * y + 3 * z) % 7 == 0;
                    values.push(if occupied {
                        VoxelValue::Occupied(material.clone())
                    } else {
                        VoxelValue::Empty
                    });
                    expected_words.push(u32::from(occupied));
                }
            }
        }
        volumes.push(DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new(name), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        ));
    }
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("block-order"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material, [0.5, 0.5, 0.5, 1.0])],
        volumes,
    ))?;
    assert_eq!(
        ComputeSceneBundle::from_view(&view)?.voxel_words(),
        expected_words
    );
    Ok(())
}
