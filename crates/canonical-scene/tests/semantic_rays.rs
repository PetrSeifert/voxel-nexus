use canonical_scene::{
    CanonicalSceneScale, canonical_edit_semantic_ray_probes, generate_canonical_scene,
};
use semantic_ray_oracle::{
    AxisNormal, SemanticRay, SemanticRayContactClassification, SemanticRayResult, observe,
    observe_along_ray, observe_probe,
};
use voxel_frontend::{
    VoxelCoordinate, VoxelEditCommand, VoxelFrontend, VoxelMaterialId, VoxelValue, VoxelVolumeId,
};

#[test]
fn canonical_edit_probes_miss_revision_one_and_identify_revision_four_edits()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::new();
    let revision_one = frontend
        .publish_sparse(generate_canonical_scene(CanonicalSceneScale::Large)?.into_scene())?;
    let probes = canonical_edit_semantic_ray_probes()?;

    for probe in &probes {
        assert_eq!(
            observe_probe(&revision_one, probe)?.observation().result(),
            &SemanticRayResult::Miss
        );
    }

    let edit_coordinates = [
        VoxelCoordinate::new(0, 0, 0),
        VoxelCoordinate::new(40, 0, 0),
        VoxelCoordinate::new(80, 0, 0),
    ];
    for coordinate in edit_coordinates {
        frontend.edit(VoxelEditCommand::new(
            VoxelVolumeId::new("canonical-volume"),
            coordinate,
            VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm")),
        ))?;
    }
    let revision_four = frontend.scene_view()?;

    for (probe, coordinate) in probes.iter().zip(edit_coordinates) {
        let observed_probe = observe_probe(&revision_four, probe)?;
        let SemanticRayResult::Contact(contact) = observed_probe.observation().result() else {
            return Err(
                format!("probe {} should contact its edited voxel", probe.identity()).into(),
            );
        };
        assert_eq!(contact.coordinate(), coordinate);
        assert_eq!(
            contact.material_identity(),
            &VoxelMaterialId::new("canonical-warm")
        );
        assert_eq!(
            contact.classification(),
            SemanticRayContactClassification::Entered(AxisNormal::NegativeZ)
        );
    }

    Ok(())
}

#[test]
fn traversal_matches_the_exhaustive_oracle_across_the_canonical_scene()
-> Result<(), Box<dyn std::error::Error>> {
    let view = VoxelFrontend::new()
        .publish_sparse(generate_canonical_scene(CanonicalSceneScale::Small)?.into_scene())?;
    let origins = [
        [20.0, 14.0, 22.0],
        [0.0, 1.0, 17.0],
        [-14.0, 2.0, 8.0],
        [0.0, -1.0, 0.0],
    ];
    let mut contact_count = 0;
    for origin in origins {
        for yaw_step in 0..8 {
            for pitch_step in 0..3 {
                let yaw = f64::from(yaw_step) * std::f64::consts::TAU / 8.0;
                let pitch = (f64::from(pitch_step) - 1.0) * 0.4;
                let direction = [
                    pitch.cos() * yaw.cos(),
                    pitch.sin(),
                    pitch.cos() * yaw.sin(),
                ];
                let ray = SemanticRay::new(origin, direction, 0.0, 100.0)?;
                let exhaustive = observe(&view, &ray)?;
                if matches!(exhaustive.result(), SemanticRayResult::Contact(_)) {
                    contact_count += 1;
                }
                assert_eq!(observe_along_ray(&view, &ray)?, exhaustive, "ray {ray:?}");
            }
        }
    }
    assert!(
        contact_count > 15,
        "only {contact_count} rays reached the canonical scene"
    );
    Ok(())
}

#[test]
fn traversal_matches_all_canonical_scales() -> Result<(), Box<dyn std::error::Error>> {
    for scale in [
        CanonicalSceneScale::Small,
        CanonicalSceneScale::Medium,
        CanonicalSceneScale::Large,
    ] {
        let view =
            VoxelFrontend::new().publish_sparse(generate_canonical_scene(scale)?.into_scene())?;
        for (origin, direction) in [
            ([-10.0, -3.0, 0.0], [1.0, 0.0, 0.0]),
            ([10.0, -3.0, 0.0], [-1.0, 0.0, 0.0]),
            ([0.0, 8.0, 0.0], [0.0, -1.0, 0.0]),
            ([0.0, -8.0, 0.0], [0.0, 1.0, 0.0]),
            ([-4.0, 0.0, -10.0], [0.0, 0.0, 1.0]),
            ([-4.0, 0.0, 10.0], [0.0, 0.0, -1.0]),
            ([-10.0, 8.0, -10.0], [1.0, -1.0, 1.0]),
            ([0.0, -3.0, 0.0], [1.0, 1.0, 1.0]),
            ([20.0, 20.0, 20.0], [1.0, 0.0, 0.0]),
        ] {
            let ray = SemanticRay::new(origin, direction, 0.0, 100.0)?;
            assert_eq!(
                observe_along_ray(&view, &ray)?,
                observe(&view, &ray)?,
                "{scale:?}, {ray:?}"
            );
        }
    }
    Ok(())
}
