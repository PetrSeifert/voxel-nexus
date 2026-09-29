use super::free_fly_camera::FreeFlyCamera;
use render_backend::{RenderPathReadiness, RenderPathStrategy};
use semantic_ray_oracle::{
    AxisNormal, SemanticRay, SemanticRayContact, SemanticRayContactClassification,
    SemanticRayObservation, SemanticRayResult, observe_along_ray,
};
use voxel_frontend::{
    VoxelCoordinate, VoxelEditCommand, VoxelEditOutcome, VoxelFrontend, VoxelMaterialId,
    VoxelSceneRevision, VoxelSceneView, VoxelValue, VoxelVolumeMetadata,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum EditAction {
    Break,
    Place(VoxelMaterialId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EditRejection {
    NoTarget,
    NoMaterial,
    StartedInside,
    OutOfBounds,
    UnknownVolume,
    Switching,
    PresenterPreparing,
}

impl EditRejection {
    pub(super) fn overlay_label(self) -> &'static str {
        match self {
            Self::NoTarget => "no-target",
            Self::NoMaterial => "no-material",
            Self::StartedInside => "started-inside",
            Self::OutOfBounds => "out-of-bounds",
            Self::UnknownVolume => "unknown-volume",
            Self::Switching => "switching",
            Self::PresenterPreparing => "presenter-preparing",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EditPublication {
    Unchanged,
    Changed(VoxelSceneRevision),
}

pub(super) fn pick(
    view: &VoxelSceneView,
    camera: &FreeFlyCamera,
) -> Result<SemanticRayObservation, String> {
    let ray = SemanticRay::new(
        camera.eye().map(f64::from),
        camera.forward().map(f64::from),
        0.0,
        f64::from(camera.far_plane()),
    )
    .map_err(|error| error.to_string())?;
    observe_along_ray(view, &ray).map_err(|error| error.to_string())
}

pub(super) fn format_pick_target(observation: &SemanticRayObservation) -> String {
    let SemanticRayResult::Contact(contact) = observation.result() else {
        return "none".to_owned();
    };
    let [x, y, z] = contact.coordinate().components();
    let face = match contact.classification() {
        SemanticRayContactClassification::Entered(normal) => axis_normal_label(normal),
        SemanticRayContactClassification::StartedInside => "inside",
    };
    format!(
        "{:?}@{x},{y},{z}/{:?}/face={face}",
        contact.volume_identity(),
        contact.material_identity()
    )
}

fn axis_normal_label(normal: AxisNormal) -> &'static str {
    match normal {
        AxisNormal::NegativeX => "-X",
        AxisNormal::PositiveX => "+X",
        AxisNormal::NegativeY => "-Y",
        AxisNormal::PositiveY => "+Y",
        AxisNormal::NegativeZ => "-Z",
        AxisNormal::PositiveZ => "+Z",
    }
}

fn axis_normal_offset(normal: AxisNormal) -> [i32; 3] {
    match normal {
        AxisNormal::NegativeX => [-1, 0, 0],
        AxisNormal::PositiveX => [1, 0, 0],
        AxisNormal::NegativeY => [0, -1, 0],
        AxisNormal::PositiveY => [0, 1, 0],
        AxisNormal::NegativeZ => [0, 0, -1],
        AxisNormal::PositiveZ => [0, 0, 1],
    }
}

pub(super) fn placement_target(
    contact: &SemanticRayContact,
    volumes: &[VoxelVolumeMetadata],
) -> Result<VoxelCoordinate, EditRejection> {
    let SemanticRayContactClassification::Entered(normal) = contact.classification() else {
        return Err(EditRejection::StartedInside);
    };
    let volume = volumes
        .iter()
        .find(|volume| volume.identity() == contact.volume_identity())
        .ok_or(EditRejection::UnknownVolume)?;
    let coordinate = contact.coordinate().components();
    let offset = axis_normal_offset(normal);
    let dimensions = volume.extent().dimensions();
    let mut target = [0_i32; 3];
    for (((target, coordinate), offset), dimension) in target
        .iter_mut()
        .zip(coordinate)
        .zip(offset)
        .zip(dimensions)
    {
        let component = coordinate
            .checked_add(offset)
            .ok_or(EditRejection::OutOfBounds)?;
        let inside = u32::try_from(component).is_ok_and(|component| component < dimension);
        if !inside {
            return Err(EditRejection::OutOfBounds);
        }
        *target = component;
    }
    let [x, y, z] = target;
    Ok(VoxelCoordinate::new(x, y, z))
}

pub(super) fn edit_command(
    action: &EditAction,
    observation: &SemanticRayObservation,
    volumes: &[VoxelVolumeMetadata],
) -> Result<VoxelEditCommand, EditRejection> {
    let SemanticRayResult::Contact(contact) = observation.result() else {
        return Err(EditRejection::NoTarget);
    };
    match action {
        EditAction::Break => Ok(VoxelEditCommand::new(
            contact.volume_identity().clone(),
            contact.coordinate(),
            VoxelValue::Empty,
        )),
        EditAction::Place(material) => Ok(VoxelEditCommand::new(
            contact.volume_identity().clone(),
            placement_target(contact, volumes)?,
            VoxelValue::Occupied(material.clone()),
        )),
    }
}

/// Runs before `VoxelFrontend::edit` so a revision is never published that no Render Path
/// will accept.
pub(super) fn edit_admission(
    replacement: Option<RenderPathStrategy>,
    presenting_readiness: RenderPathReadiness,
) -> Result<(), EditRejection> {
    if replacement.is_some() {
        return Err(EditRejection::Switching);
    }
    if presenting_readiness != RenderPathReadiness::Recordable {
        return Err(EditRejection::PresenterPreparing);
    }
    Ok(())
}

pub(super) fn publish_edit(
    frontend: &VoxelFrontend,
    command: VoxelEditCommand,
    submit: impl FnOnce(VoxelEditOutcome) -> Result<(), String>,
) -> Result<EditPublication, String> {
    let outcome = frontend.edit(command).map_err(|error| error.to_string())?;
    let revision = match &outcome {
        VoxelEditOutcome::Unchanged(_) => return Ok(EditPublication::Unchanged),
        VoxelEditOutcome::Changed { view, .. } => view.revision(),
    };
    submit(outcome)?;
    Ok(EditPublication::Changed(revision))
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_frontend::{
        DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelExtent, VoxelMaterial,
        VoxelRegion, VoxelSceneId, VoxelVolumeId,
    };

    fn volume() -> VoxelVolumeMetadata {
        VoxelVolumeMetadata::new(
            VoxelVolumeId::new("volume"),
            VoxelExtent::new(4, 3, 5),
            [0.0; 3],
            1.0,
        )
    }

    fn contact(
        coordinate: VoxelCoordinate,
        classification: SemanticRayContactClassification,
    ) -> SemanticRayContact {
        SemanticRayContact::new(
            VoxelVolumeId::new("volume"),
            coordinate,
            VoxelMaterialId::new("stone"),
            1.0,
            classification,
        )
    }

    #[test]
    fn placement_target_steps_out_through_each_entry_face() {
        let origin = VoxelCoordinate::new(1, 1, 2);
        for (normal, expected) in [
            (AxisNormal::NegativeX, VoxelCoordinate::new(0, 1, 2)),
            (AxisNormal::PositiveX, VoxelCoordinate::new(2, 1, 2)),
            (AxisNormal::NegativeY, VoxelCoordinate::new(1, 0, 2)),
            (AxisNormal::PositiveY, VoxelCoordinate::new(1, 2, 2)),
            (AxisNormal::NegativeZ, VoxelCoordinate::new(1, 1, 1)),
            (AxisNormal::PositiveZ, VoxelCoordinate::new(1, 1, 3)),
        ] {
            assert_eq!(
                placement_target(
                    &contact(origin, SemanticRayContactClassification::Entered(normal)),
                    &[volume()],
                ),
                Ok(expected),
                "{normal:?}"
            );
        }
    }

    #[test]
    fn placement_outside_the_volume_bounds_is_rejected() {
        for (coordinate, normal) in [
            (VoxelCoordinate::new(0, 1, 1), AxisNormal::NegativeX),
            (VoxelCoordinate::new(3, 1, 1), AxisNormal::PositiveX),
            (VoxelCoordinate::new(1, 0, 1), AxisNormal::NegativeY),
            (VoxelCoordinate::new(1, 2, 1), AxisNormal::PositiveY),
            (VoxelCoordinate::new(1, 1, 0), AxisNormal::NegativeZ),
            (VoxelCoordinate::new(1, 1, 4), AxisNormal::PositiveZ),
            (VoxelCoordinate::new(i32::MAX, 1, 1), AxisNormal::PositiveX),
        ] {
            assert_eq!(
                placement_target(
                    &contact(
                        coordinate,
                        SemanticRayContactClassification::Entered(normal)
                    ),
                    &[volume()],
                ),
                Err(EditRejection::OutOfBounds),
                "{coordinate:?} {normal:?}"
            );
        }
    }

    #[test]
    fn started_inside_contacts_have_no_placement_target() {
        let started_inside = contact(
            VoxelCoordinate::new(1, 1, 1),
            SemanticRayContactClassification::StartedInside,
        );
        assert_eq!(
            placement_target(&started_inside, &[volume()]),
            Err(EditRejection::StartedInside)
        );
        let observation = SemanticRayObservation::new(
            VoxelSceneId::new("scene"),
            VoxelSceneRevision::new(1),
            SemanticRayResult::Contact(started_inside),
        );
        assert_eq!(
            edit_command(
                &EditAction::Place(VoxelMaterialId::new("stone")),
                &observation,
                &[volume()],
            ),
            Err(EditRejection::StartedInside)
        );
        assert!(edit_command(&EditAction::Break, &observation, &[volume()]).is_ok());
    }

    #[test]
    fn a_miss_has_no_edit_target() {
        let observation = SemanticRayObservation::new(
            VoxelSceneId::new("scene"),
            VoxelSceneRevision::new(1),
            SemanticRayResult::Miss,
        );
        assert_eq!(
            edit_command(&EditAction::Break, &observation, &[volume()]),
            Err(EditRejection::NoTarget)
        );
    }

    #[test]
    fn edits_are_admitted_only_without_a_replacement_and_with_a_recordable_presenter() {
        let replacement = RenderPathStrategy::new("test.replacement");
        assert_eq!(
            edit_admission(None, RenderPathReadiness::Recordable),
            Ok(())
        );
        assert_eq!(
            edit_admission(Some(replacement), RenderPathReadiness::Recordable),
            Err(EditRejection::Switching)
        );
        assert_eq!(
            edit_admission(None, RenderPathReadiness::Preparing),
            Err(EditRejection::PresenterPreparing)
        );
    }

    #[test]
    fn crosshair_pick_matches_the_oracle_for_the_latest_revision() -> Result<(), String> {
        let frontend = VoxelFrontend::new();
        frontend
            .publish_sparse(
                canonical_scene::generate_canonical_scene(
                    canonical_scene::CanonicalSceneScale::Small,
                )
                .map_err(|error| error.to_string())?
                .into_scene(),
            )
            .map_err(|error| error.to_string())?;
        let camera = FreeFlyCamera::from_camera_state(
            canonical_inspection::CanonicalCameraPose::Overview
                .pose()
                .map_err(|error| error.to_string())?,
        );
        let before = pick(
            &frontend.scene_view().map_err(|error| error.to_string())?,
            &camera,
        )?;
        let command = edit_command(&EditAction::Break, &before, &[])
            .map_err(|rejection| rejection.overlay_label().to_owned())?;
        assert_eq!(
            publish_edit(&frontend, command, |_| Ok(()))?,
            EditPublication::Changed(VoxelSceneRevision::new(2))
        );

        let latest = frontend.scene_view().map_err(|error| error.to_string())?;
        let picked = pick(&latest, &camera)?;
        let ray = SemanticRay::new(
            camera.eye().map(f64::from),
            camera.forward().map(f64::from),
            0.0,
            f64::from(camera.far_plane()),
        )
        .map_err(|error| error.to_string())?;
        let oracle =
            semantic_ray_oracle::observe(&latest, &ray).map_err(|error| error.to_string())?;
        assert_eq!(picked, oracle);
        assert_eq!(picked.revision(), VoxelSceneRevision::new(2));
        assert_ne!(picked.result(), before.result());
        Ok(())
    }

    #[test]
    fn each_changing_edit_publishes_exactly_one_revision() -> Result<(), String> {
        let extent = VoxelExtent::new(2, 1, 1);
        let stone = VoxelMaterialId::new("stone");
        let frontend = VoxelFrontend::new();
        frontend
            .publish(DenseVoxelScene::new(
                VoxelSceneId::new("scene"),
                VoxelSceneRevision::new(1),
                vec![VoxelMaterial::new(stone.clone(), [1.0; 4])],
                vec![DenseVoxelVolume::new(
                    VoxelVolumeMetadata::new(VoxelVolumeId::new("volume"), extent, [0.0; 3], 1.0),
                    vec![DenseVoxelBatch::new(
                        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                        vec![VoxelValue::Occupied(stone.clone()), VoxelValue::Empty],
                    )],
                )],
            ))
            .map_err(|error| error.to_string())?;
        let mut submitted = Vec::new();
        let mut publish = |value: VoxelValue| {
            publish_edit(
                &frontend,
                VoxelEditCommand::new(
                    VoxelVolumeId::new("volume"),
                    VoxelCoordinate::new(0, 0, 0),
                    value,
                ),
                |outcome| {
                    submitted.push(outcome.view().revision());
                    Ok(())
                },
            )
        };

        assert_eq!(
            publish(VoxelValue::Empty)?,
            EditPublication::Changed(VoxelSceneRevision::new(2))
        );
        assert_eq!(publish(VoxelValue::Empty)?, EditPublication::Unchanged);
        assert_eq!(
            publish(VoxelValue::Occupied(stone))?,
            EditPublication::Changed(VoxelSceneRevision::new(3))
        );
        assert_eq!(
            submitted,
            vec![VoxelSceneRevision::new(2), VoxelSceneRevision::new(3)]
        );
        assert_eq!(
            frontend
                .scene_view()
                .map_err(|error| error.to_string())?
                .revision(),
            VoxelSceneRevision::new(3)
        );
        Ok(())
    }
}
