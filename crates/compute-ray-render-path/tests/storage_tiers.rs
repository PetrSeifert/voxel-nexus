use canonical_scene::{
    CanonicalSceneScale, canonical_edit_semantic_ray_probes, generate_canonical_scene,
};
use compute_ray_render_path::ComputeSceneBundle;
use raster_render_path::{derive_raster_artifact, qualify_raster_semantic_faces};
use semantic_ray_oracle::{SemanticRayDistanceTolerance, observe_probe};
use std::{hint::black_box, time::Instant};
use voxel_frontend::{
    StorageTier, VoxelCoordinate, VoxelEditCommand, VoxelFrontend, VoxelMaterialId, VoxelRegion,
    VoxelValue, VoxelVolumeId,
};

#[test]
fn canonical_storage_tiers_have_identical_observations() -> Result<(), Box<dyn std::error::Error>> {
    let probes = canonical_edit_semantic_ray_probes()?;
    let tolerance = SemanticRayDistanceTolerance::new(1.0e-9)?;
    let identity = VoxelVolumeId::new("canonical-volume");
    for scale in [
        CanonicalSceneScale::Small,
        CanonicalSceneScale::Medium,
        CanonicalSceneScale::Large,
    ] {
        let scene = generate_canonical_scene(scale)?.into_scene();
        let dense_frontend = VoxelFrontend::new();
        let sparse_frontend = VoxelFrontend::new();
        dense_frontend.publish_sparse(scene.clone())?;
        sparse_frontend.publish_sparse(scene.with_storage_tier(StorageTier::SparsePages))?;
        for edited in [false, true] {
            if edited {
                for frontend in [&dense_frontend, &sparse_frontend] {
                    frontend.edit(VoxelEditCommand::new(
                        identity.clone(),
                        VoxelCoordinate::new(0, 0, 0),
                        VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm")),
                    ))?;
                }
            }
            let dense = dense_frontend.scene_view()?;
            let sparse = sparse_frontend.scene_view()?;
            let dense_bundle = ComputeSceneBundle::from_view(&dense)?;
            let sparse_bundle = ComputeSceneBundle::from_view(&sparse)?;
            assert_eq!(dense_bundle.voxel_words(), sparse_bundle.voxel_words());
            assert_eq!(
                dense_bundle.material_words(),
                sparse_bundle.material_words()
            );
            let dense_artifact = derive_raster_artifact(&dense, &identity)?;
            let sparse_artifact = derive_raster_artifact(&sparse, &identity)?;
            assert_eq!(
                dense_artifact.semantic_faces().collect::<Vec<_>>(),
                sparse_artifact.semantic_faces().collect::<Vec<_>>()
            );
            for probe in &probes {
                let dense_observation = observe_probe(&dense, probe)?;
                let sparse_observation = observe_probe(&sparse, probe)?;
                assert_eq!(dense_observation, sparse_observation);
                assert!(dense_bundle.observe(probe.ray()).agrees_with(
                    &semantic_ray_oracle::observe(&dense, probe.ray())?,
                    tolerance
                ));
                assert!(sparse_bundle.observe(probe.ray()).agrees_with(
                    &semantic_ray_oracle::observe(&sparse, probe.ray())?,
                    tolerance
                ));
                for artifact in [&dense_artifact, &sparse_artifact] {
                    assert!(
                        qualify_raster_semantic_faces(
                            artifact,
                            std::slice::from_ref(&dense_observation),
                            1
                        )?
                        .iter()
                        .all(|observation| observation.passed())
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "manual release-mode storage measurements"]
fn storage_costs() -> Result<(), Box<dyn std::error::Error>> {
    for scale in [
        CanonicalSceneScale::Small,
        CanonicalSceneScale::Medium,
        CanonicalSceneScale::Large,
    ] {
        for tier in [StorageTier::Dense, StorageTier::SparsePages] {
            let frontend = VoxelFrontend::new();
            let view = frontend.publish_sparse(
                generate_canonical_scene(scale)?
                    .into_scene()
                    .with_storage_tier(tier),
            )?;
            let volume = view.volumes().first().ok_or("missing volume")?;
            let region = VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), volume.extent());
            let count = volume
                .extent()
                .dimensions()
                .into_iter()
                .map(|dimension| dimension as usize)
                .product();
            let mut values = vec![VoxelValue::Empty; count];
            let mut reads = Vec::new();
            for _ in 0..9 {
                let start = Instant::now();
                view.read_region_into(volume.identity(), region, &mut values)?;
                reads.push(start.elapsed());
                black_box(&values);
            }
            let mut edits = Vec::new();
            for index in 0..101 {
                let value = if index % 2 == 0 {
                    VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm"))
                } else {
                    VoxelValue::Empty
                };
                let start = Instant::now();
                black_box(frontend.edit(VoxelEditCommand::new(
                    volume.identity().clone(),
                    VoxelCoordinate::new(0, 0, 0),
                    value,
                ))?);
                edits.push(start.elapsed());
            }
            reads.sort();
            edits.sort();
            println!(
                "{scale:?} {tier:?}: estimated_bytes={}, full_read_median={:?}, edit_median={:?}",
                view.storage_bytes(volume.identity())?,
                reads.get(4).ok_or("missing read")?,
                edits.get(50).ok_or("missing edit")?
            );
        }
    }
    Ok(())
}
