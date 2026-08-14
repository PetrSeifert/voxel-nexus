use canonical_scene::{
    CanonicalSceneScale, canonical_edit_semantic_ray_probes, generate_canonical_scene,
};
use semantic_ray_oracle::{
    AxisNormal, SemanticRayContactClassification, SemanticRayResult, observe_probe,
};
use voxel_frontend::{
    VoxelCoordinate, VoxelEditCommand, VoxelFrontend, VoxelMaterialId, VoxelValue, VoxelVolumeId,
};

#[test]
fn canonical_edit_probes_miss_revision_one_and_identify_revision_four_edits()
-> Result<(), Box<dyn std::error::Error>> {
    let frontend = VoxelFrontend::new();
    let revision_one =
        frontend.publish(generate_canonical_scene(CanonicalSceneScale::Large)?.into_scene())?;
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
