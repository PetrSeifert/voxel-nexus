use semantic_ray_oracle::{
    AxisNormal, SemanticRay, SemanticRayContact, SemanticRayContactClassification,
    SemanticRayDistanceTolerance, SemanticRayError, SemanticRayObservation, SemanticRayProbe,
    SemanticRayResult, observe, observe_probe,
};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

fn one_voxel_view() -> Result<voxel_frontend::VoxelSceneView, Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(1, 1, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let scene = DenseVoxelScene::new(
        VoxelSceneId::new("micro-scene"),
        VoxelSceneRevision::new(7),
        vec![VoxelMaterial::new(
            material_identity.clone(),
            [0.25, 0.5, 0.75, 1.0],
        )],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("single-voxel"),
                extent,
                [0.0, 0.0, 0.0],
                1.0,
            ),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Occupied(material_identity)],
            )],
        )],
    );
    Ok(VoxelFrontend::new().publish(scene)?)
}

fn contact(
    observation: &SemanticRayObservation,
) -> Result<&SemanticRayContact, Box<dyn std::error::Error>> {
    match observation.result() {
        SemanticRayResult::Contact(contact) => Ok(contact),
        SemanticRayResult::Miss => Err("expected a Semantic Ray contact".into()),
    }
}

#[test]
fn entered_contact_identifies_the_pinned_scene_and_occupied_value()
-> Result<(), Box<dyn std::error::Error>> {
    let view = one_voxel_view()?;
    let ray = SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?;

    let observation = observe(&view, &ray)?;

    assert_eq!(
        observation.scene_identity(),
        &VoxelSceneId::new("micro-scene")
    );
    assert_eq!(observation.revision(), VoxelSceneRevision::new(7));
    let SemanticRayResult::Contact(contact) = observation.result() else {
        return Err("the occupied voxel should produce a contact".into());
    };
    assert_eq!(
        contact.volume_identity(),
        &VoxelVolumeId::new("single-voxel")
    );
    assert_eq!(contact.coordinate(), VoxelCoordinate::new(0, 0, 0));
    assert_eq!(contact.material_identity(), &VoxelMaterialId::new("stone"));
    assert_eq!(contact.distance(), 1.0);
    assert_eq!(
        contact.classification(),
        SemanticRayContactClassification::Entered(AxisNormal::NegativeX)
    );

    Ok(())
}

#[test]
fn clipped_start_inside_an_occupied_value_has_no_outward_normal()
-> Result<(), Box<dyn std::error::Error>> {
    let view = one_voxel_view()?;
    let ray = SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 1.25, 3.0)?;

    let observation = observe(&view, &ray)?;

    let SemanticRayResult::Contact(contact) = observation.result() else {
        return Err("the clipped start lies inside the occupied voxel".into());
    };
    assert_eq!(contact.distance(), 1.25);
    assert_eq!(
        contact.classification(),
        SemanticRayContactClassification::StartedInside
    );

    Ok(())
}

#[test]
fn equal_distance_contacts_use_stable_volume_identity_order()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(1, 1, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let volumes = ["zeta", "alpha"]
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
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("overlap"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material_identity, [1.0; 4])],
        volumes,
    ))?;
    let ray = SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?;

    let observation = observe(&view, &ray)?;

    let SemanticRayResult::Contact(contact) = observation.result() else {
        return Err("both overlapping volumes contain a contact".into());
    };
    assert_eq!(contact.volume_identity(), &VoxelVolumeId::new("alpha"));

    Ok(())
}

#[test]
fn finite_direction_magnitude_does_not_change_scene_space_distance()
-> Result<(), Box<dyn std::error::Error>> {
    let view = one_voxel_view()?;
    let ray = SemanticRay::new([-1.0, 0.5, 0.5], [f64::MAX, 0.0, 0.0], 0.0, 3.0)?;

    assert_eq!(ray.origin(), [-1.0, 0.5, 0.5]);
    assert_eq!(ray.direction(), [1.0, 0.0, 0.0]);
    assert_eq!(ray.minimum_distance(), 0.0);
    assert_eq!(ray.maximum_distance(), 3.0);

    let observation = observe(&view, &ray)?;

    let SemanticRayResult::Contact(contact) = observation.result() else {
        return Err("a finite scaled direction should enter the occupied voxel".into());
    };
    assert_eq!(contact.distance(), 1.0);

    Ok(())
}

#[test]
fn occupied_voxel_is_entered_through_each_exposed_face() -> Result<(), Box<dyn std::error::Error>> {
    let view = one_voxel_view()?;
    let cases = [
        ([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], AxisNormal::NegativeX),
        ([2.0, 0.5, 0.5], [-1.0, 0.0, 0.0], AxisNormal::PositiveX),
        ([0.5, -1.0, 0.5], [0.0, 1.0, 0.0], AxisNormal::NegativeY),
        ([0.5, 2.0, 0.5], [0.0, -1.0, 0.0], AxisNormal::PositiveY),
        ([0.5, 0.5, -1.0], [0.0, 0.0, 1.0], AxisNormal::NegativeZ),
        ([0.5, 0.5, 2.0], [0.0, 0.0, -1.0], AxisNormal::PositiveZ),
    ];

    for (origin, direction, expected_normal) in cases {
        let ray = SemanticRay::new(origin, direction, 0.0, 3.0)?;
        let observation = observe(&view, &ray)?;
        let contact = contact(&observation)?;
        assert_eq!(contact.distance(), 1.0);
        assert_eq!(
            contact.classification(),
            SemanticRayContactClassification::Entered(expected_normal)
        );
    }

    Ok(())
}

#[test]
fn half_open_bounds_and_clip_interval_exclude_non_contacts()
-> Result<(), Box<dyn std::error::Error>> {
    let view = one_voxel_view()?;
    let misses = [
        SemanticRay::new([-1.0, 1.0, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?,
        SemanticRay::new([1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?,
        SemanticRay::new([-2.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 1.5)?,
        SemanticRay::new([-2.0, 0.5, 0.5], [1.0, 0.0, 0.0], 3.0, 4.0)?,
    ];

    for ray in misses {
        assert_eq!(observe(&view, &ray)?.result(), &SemanticRayResult::Miss);
    }

    let inward_from_maximum = SemanticRay::new([1.0, 0.5, 0.5], [-1.0, 0.0, 0.0], 0.0, 1.0)?;
    let observation = observe(&view, &inward_from_maximum)?;
    assert_eq!(
        contact(&observation)?.classification(),
        SemanticRayContactClassification::Entered(AxisNormal::PositiveX)
    );

    let far_plane_contact = SemanticRay::new([-2.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 2.0)?;
    assert_eq!(
        contact(&observe(&view, &far_plane_contact)?)?.distance(),
        2.0
    );

    Ok(())
}

#[test]
fn empty_scene_and_parallel_outside_ray_miss() -> Result<(), Box<dyn std::error::Error>> {
    let empty_view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("empty"),
        VoxelSceneRevision::new(3),
        Vec::new(),
        Vec::new(),
    ))?;
    let ray = SemanticRay::new([0.0; 3], [1.0, 0.0, 0.0], 0.0, 1.0)?;
    assert_eq!(
        observe(&empty_view, &ray)?.result(),
        &SemanticRayResult::Miss
    );

    let occupied_view = one_voxel_view()?;
    let parallel_outside = SemanticRay::new([-1.0, 2.0, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?;
    assert_eq!(
        observe(&occupied_view, &parallel_outside)?.result(),
        &SemanticRayResult::Miss
    );

    Ok(())
}

#[test]
fn nonzero_volume_origin_and_non_unit_voxel_size_preserve_material_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 1, 1);
    let warm = VoxelMaterialId::new("warm");
    let cool = VoxelMaterialId::new("cool");
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("scaled"),
        VoxelSceneRevision::new(9),
        vec![
            VoxelMaterial::new(warm.clone(), [1.0, 0.0, 0.0, 1.0]),
            VoxelMaterial::new(cool.clone(), [0.0, 0.0, 1.0, 1.0]),
        ],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("scaled-volume"),
                extent,
                [10.0, 20.0, 30.0],
                0.5,
            ),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![
                    VoxelValue::Occupied(warm),
                    VoxelValue::Occupied(cool.clone()),
                ],
            )],
        )],
    ))?;
    let ray = SemanticRay::new([11.5, 20.25, 30.25], [-1.0, 0.0, 0.0], 0.0, 2.0)?;

    let observation = observe(&view, &ray)?;
    let contact = contact(&observation)?;

    assert_eq!(contact.coordinate(), VoxelCoordinate::new(1, 0, 0));
    assert_eq!(contact.material_identity(), &cool);
    assert_eq!(contact.distance(), 0.5);
    assert_eq!(
        contact.classification(),
        SemanticRayContactClassification::Entered(AxisNormal::PositiveX)
    );

    Ok(())
}

#[test]
fn simultaneous_axis_crossings_advance_all_axes_with_fixed_normal_priority()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 2, 2);
    let material_identity = VoxelMaterialId::new("stone");
    let mut values = vec![VoxelValue::Empty; 8];
    let diagonal_index = 7;
    let destination = values
        .get_mut(diagonal_index)
        .ok_or("micro-scene diagonal coordinate was not addressable")?;
    *destination = VoxelValue::Occupied(material_identity.clone());
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("corner-tie"),
        VoxelSceneRevision::new(2),
        vec![VoxelMaterial::new(material_identity, [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("tie-volume"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    ))?;
    let ray = SemanticRay::new([-1.0; 3], [1.0; 3], 0.0, 6.0)?;

    let observation = observe(&view, &ray)?;
    let contact = contact(&observation)?;

    assert_eq!(contact.coordinate(), VoxelCoordinate::new(1, 1, 1));
    assert!((contact.distance() - 2.0 * 3.0_f64.sqrt()).abs() <= f64::EPSILON * 4.0);
    assert_eq!(
        contact.classification(),
        SemanticRayContactClassification::Entered(AxisNormal::NegativeX)
    );

    Ok(())
}

#[test]
fn two_axis_crossing_uses_x_before_y_for_the_entered_normal()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 2, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("edge-tie"),
        VoxelSceneRevision::new(2),
        vec![VoxelMaterial::new(material_identity.clone(), [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("tie-volume"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![
                    VoxelValue::Empty,
                    VoxelValue::Empty,
                    VoxelValue::Empty,
                    VoxelValue::Occupied(material_identity),
                ],
            )],
        )],
    ))?;
    let ray = SemanticRay::new([-1.0, -1.0, 0.5], [1.0, 1.0, 0.0], 0.0, 6.0)?;

    let observation = observe(&view, &ray)?;
    let contact = contact(&observation)?;

    assert_eq!(contact.coordinate(), VoxelCoordinate::new(1, 1, 0));
    assert_eq!(
        contact.classification(),
        SemanticRayContactClassification::Entered(AxisNormal::NegativeX)
    );

    Ok(())
}

#[test]
fn adjacent_occupied_values_report_only_the_exposed_entry_contact()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 1, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("adjacent"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material_identity.clone(), [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("adjacent-volume"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![
                    VoxelValue::Occupied(material_identity.clone()),
                    VoxelValue::Occupied(material_identity),
                ],
            )],
        )],
    ))?;
    let ray = SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 4.0)?;

    let observation = observe(&view, &ray)?;
    let contact = contact(&observation)?;

    assert_eq!(contact.coordinate(), VoxelCoordinate::new(0, 0, 0));
    assert_eq!(contact.distance(), 1.0);

    Ok(())
}

#[test]
fn ray_starting_in_an_empty_value_reaches_the_next_occupied_value()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 1, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("empty-start"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material_identity.clone(), [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("empty-start-volume"),
                extent,
                [0.0; 3],
                1.0,
            ),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![VoxelValue::Empty, VoxelValue::Occupied(material_identity)],
            )],
        )],
    ))?;
    let ray = SemanticRay::new([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 2.0)?;

    let observation = observe(&view, &ray)?;
    let contact = contact(&observation)?;

    assert_eq!(contact.coordinate(), VoxelCoordinate::new(1, 0, 0));
    assert_eq!(contact.distance(), 0.5);
    assert_eq!(
        contact.classification(),
        SemanticRayContactClassification::Entered(AxisNormal::NegativeX)
    );

    Ok(())
}

#[test]
fn clipped_start_on_an_internal_half_open_boundary_uses_the_containing_value()
-> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 1, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("internal-boundary"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material_identity.clone(), [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("adjacent-volume"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![
                    VoxelValue::Occupied(material_identity.clone()),
                    VoxelValue::Occupied(material_identity),
                ],
            )],
        )],
    ))?;
    let ray = SemanticRay::new([1.0, 0.5, 0.5], [-1.0, 0.0, 0.0], 0.0, 2.0)?;

    let observation = observe(&view, &ray)?;
    let contact = contact(&observation)?;

    assert_eq!(contact.coordinate(), VoxelCoordinate::new(1, 0, 0));
    assert_eq!(
        contact.classification(),
        SemanticRayContactClassification::StartedInside
    );

    Ok(())
}

#[test]
fn nearest_volume_wins_independently_of_publication_order() -> Result<(), Box<dyn std::error::Error>>
{
    let extent = VoxelExtent::new(1, 1, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let ray = SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 6.0)?;

    for volume_order in [
        [("far", [3.0, 0.0, 0.0]), ("near", [0.0, 0.0, 0.0])],
        [("near", [0.0, 0.0, 0.0]), ("far", [3.0, 0.0, 0.0])],
    ] {
        let volumes = volume_order
            .into_iter()
            .map(|(identity, scene_origin)| {
                DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(
                        VoxelVolumeId::new(identity),
                        extent,
                        scene_origin,
                        1.0,
                    ),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                        vec![VoxelValue::Occupied(material_identity.clone())],
                    )],
                )
            })
            .collect();
        let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
            VoxelSceneId::new("nearest"),
            VoxelSceneRevision::new(1),
            vec![VoxelMaterial::new(material_identity.clone(), [1.0; 4])],
            volumes,
        ))?;
        let observation = observe(&view, &ray)?;
        assert_eq!(
            contact(&observation)?.volume_identity(),
            &VoxelVolumeId::new("near")
        );
    }

    Ok(())
}

#[test]
fn edge_only_cell_contact_is_not_a_hit() -> Result<(), Box<dyn std::error::Error>> {
    let extent = VoxelExtent::new(2, 2, 1);
    let material_identity = VoxelMaterialId::new("stone");
    let view = VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("edge-touch"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material_identity.clone(), [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("edge-volume"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                vec![
                    VoxelValue::Empty,
                    VoxelValue::Occupied(material_identity),
                    VoxelValue::Empty,
                    VoxelValue::Empty,
                ],
            )],
        )],
    ))?;
    let ray = SemanticRay::new([-1.0, -1.0, 0.5], [1.0, 1.0, 0.0], 0.0, 6.0)?;

    assert_eq!(observe(&view, &ray)?.result(), &SemanticRayResult::Miss);

    Ok(())
}

#[test]
fn invalid_semantic_rays_are_rejected_before_scene_evaluation() {
    assert!(matches!(
        SemanticRay::new([f64::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], 0.0, 1.0),
        Err(SemanticRayError::NonFiniteInput)
    ));
    assert!(matches!(
        SemanticRay::new([0.0; 3], [0.0; 3], 0.0, 1.0),
        Err(SemanticRayError::ZeroLengthDirection)
    ));
    assert!(matches!(
        SemanticRay::new([0.0; 3], [1.0, 0.0, 0.0], 2.0, 1.0),
        Err(SemanticRayError::InvalidClipInterval)
    ));
    assert!(matches!(
        SemanticRay::new([0.0; 3], [1.0, 0.0, 0.0], -1.0, 1.0),
        Err(SemanticRayError::InvalidClipInterval)
    ));
}

#[test]
fn declared_probe_identity_is_retained_with_its_observation()
-> Result<(), Box<dyn std::error::Error>> {
    let view = one_voxel_view()?;
    let probe = SemanticRayProbe::new(
        "single-voxel-negative-x",
        SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?,
    )?;

    let observed_probe = observe_probe(&view, &probe)?;

    assert_eq!(observed_probe.probe_identity(), "single-voxel-negative-x");
    assert_eq!(
        contact(observed_probe.observation())?.coordinate(),
        VoxelCoordinate::new(0, 0, 0)
    );

    Ok(())
}

#[test]
fn observation_comparison_tolerates_only_contact_distance() -> Result<(), Box<dyn std::error::Error>>
{
    let view = one_voxel_view()?;
    let expected = observe(
        &view,
        &SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?,
    )?;
    let nearby = observe(
        &view,
        &SemanticRay::new([-1.0005, 0.5, 0.5], [1.0, 0.0, 0.0], 0.0, 3.0)?,
    )?;
    let started_inside = observe(
        &view,
        &SemanticRay::new([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0], 1.25, 3.0)?,
    )?;

    assert!(expected.agrees_with(&nearby, SemanticRayDistanceTolerance::new(0.001)?,));
    assert!(!expected.agrees_with(&nearby, SemanticRayDistanceTolerance::new(0.0001)?,));
    assert!(!expected.agrees_with(&started_inside, SemanticRayDistanceTolerance::new(1.0)?,));
    assert!(SemanticRayDistanceTolerance::new(f64::NAN).is_err());
    assert!(SemanticRayDistanceTolerance::new(-0.1).is_err());

    Ok(())
}
