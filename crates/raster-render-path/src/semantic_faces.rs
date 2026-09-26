use super::meshing::RasterArtifact;
use semantic_ray_oracle::{
    AxisNormal as SemanticAxisNormal, SemanticRayContactClassification,
    SemanticRayProbeObservation, SemanticRayResult,
};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use voxel_frontend::{
    VoxelCoordinate, VoxelMaterialId, VoxelSceneId, VoxelSceneRevision, VoxelVolumeId,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AxisNormal {
    NegativeX,
    PositiveX,
    NegativeY,
    PositiveY,
    NegativeZ,
    PositiveZ,
}

impl AxisNormal {
    pub fn vector(self) -> [f32; 3] {
        match self {
            Self::NegativeX => [-1.0, 0.0, 0.0],
            Self::PositiveX => [1.0, 0.0, 0.0],
            Self::NegativeY => [0.0, -1.0, 0.0],
            Self::PositiveY => [0.0, 1.0, 0.0],
            Self::NegativeZ => [0.0, 0.0, -1.0],
            Self::PositiveZ => [0.0, 0.0, 1.0],
        }
    }

    pub(super) fn offset(self) -> [i32; 3] {
        match self {
            Self::NegativeX => [-1, 0, 0],
            Self::PositiveX => [1, 0, 0],
            Self::NegativeY => [0, -1, 0],
            Self::PositiveY => [0, 1, 0],
            Self::NegativeZ => [0, 0, -1],
            Self::PositiveZ => [0, 0, 1],
        }
    }
}

pub(super) const AXIS_NORMALS: [AxisNormal; 6] = [
    AxisNormal::NegativeX,
    AxisNormal::PositiveX,
    AxisNormal::NegativeY,
    AxisNormal::PositiveY,
    AxisNormal::NegativeZ,
    AxisNormal::PositiveZ,
];

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SemanticFace {
    volume_identity: VoxelVolumeId,
    occupied_coordinate: VoxelCoordinate,
    outward_normal: AxisNormal,
    material_identity: VoxelMaterialId,
}

impl SemanticFace {
    pub fn new(
        volume_identity: VoxelVolumeId,
        occupied_coordinate: VoxelCoordinate,
        outward_normal: AxisNormal,
        material_identity: VoxelMaterialId,
    ) -> Self {
        Self {
            volume_identity,
            occupied_coordinate,
            outward_normal,
            material_identity,
        }
    }

    pub fn volume_identity(&self) -> &VoxelVolumeId {
        &self.volume_identity
    }

    pub fn occupied_coordinate(&self) -> VoxelCoordinate {
        self.occupied_coordinate
    }

    pub fn outward_normal(&self) -> AxisNormal {
        self.outward_normal
    }

    pub fn material_identity(&self) -> &VoxelMaterialId {
        &self.material_identity
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RasterSemanticFaceCorrespondence {
    Matched(SemanticFace),
    Missing(SemanticFace),
    NotApplicableMiss,
    NotApplicableStartedInside,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterSemanticFaceObservation {
    probe_identity: String,
    scene_identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    frame_sequence: u64,
    correspondence: RasterSemanticFaceCorrespondence,
}

impl RasterSemanticFaceObservation {
    pub fn probe_identity(&self) -> &str {
        &self.probe_identity
    }

    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn revision(&self) -> VoxelSceneRevision {
        self.revision
    }

    pub fn frame_sequence(&self) -> u64 {
        self.frame_sequence
    }

    pub fn correspondence(&self) -> &RasterSemanticFaceCorrespondence {
        &self.correspondence
    }

    pub fn passed(&self) -> bool {
        matches!(
            self.correspondence,
            RasterSemanticFaceCorrespondence::Matched(_)
                | RasterSemanticFaceCorrespondence::NotApplicableMiss
                | RasterSemanticFaceCorrespondence::NotApplicableStartedInside
        )
    }
}

#[derive(Clone, Debug)]
pub struct RasterSemanticFaceController {
    pub(super) state: Arc<Mutex<RasterSemanticFaceControlState>>,
}

#[derive(Debug, Default)]
pub(super) struct RasterSemanticFaceControlState {
    pending: Option<Vec<SemanticRayProbeObservation>>,
    retained: Vec<RasterSemanticFaceObservation>,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RasterSemanticFaceControlError {
    #[error("a raster Semantic Face observation request is already pending")]
    RequestPending,
    #[error("raster Semantic Face observation control is unavailable")]
    Unavailable,
    #[error(
        "the installed raster artifact is scene {actual:?} revision {actual_revision}, but probe {probe_identity} expects scene {expected:?} revision {expected_revision}"
    )]
    AttributionMismatch {
        probe_identity: String,
        expected: VoxelSceneId,
        expected_revision: VoxelSceneRevision,
        actual: VoxelSceneId,
        actual_revision: VoxelSceneRevision,
    },
}

impl RasterSemanticFaceController {
    pub fn request(
        &self,
        oracle_observations: Vec<SemanticRayProbeObservation>,
    ) -> Result<(), RasterSemanticFaceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RasterSemanticFaceControlError::Unavailable)?;
        if state.pending.is_some() {
            return Err(RasterSemanticFaceControlError::RequestPending);
        }
        state.pending = Some(oracle_observations);
        Ok(())
    }

    pub fn drain(
        &self,
    ) -> Result<Vec<RasterSemanticFaceObservation>, RasterSemanticFaceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RasterSemanticFaceControlError::Unavailable)?;
        Ok(std::mem::take(&mut state.retained))
    }

    pub(super) fn observe_presented_artifact(
        &self,
        artifact: &RasterArtifact,
        frame_sequence: u64,
    ) -> Result<(), RasterSemanticFaceControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RasterSemanticFaceControlError::Unavailable)?;
        let Some(pending) = state.pending.as_ref() else {
            return Ok(());
        };
        let observations = qualify_raster_semantic_faces(artifact, pending, frame_sequence)?;
        state.pending = None;
        state.retained.extend(observations);
        Ok(())
    }
}

pub fn qualify_raster_semantic_faces(
    artifact: &RasterArtifact,
    oracle_observations: &[SemanticRayProbeObservation],
    frame_sequence: u64,
) -> Result<Vec<RasterSemanticFaceObservation>, RasterSemanticFaceControlError> {
    for probe in oracle_observations {
        let observation = probe.observation();
        if observation.scene_identity() != artifact.scene_identity()
            || observation.revision() != artifact.source_revision()
        {
            return Err(RasterSemanticFaceControlError::AttributionMismatch {
                probe_identity: probe.probe_identity().to_owned(),
                expected: observation.scene_identity().clone(),
                expected_revision: observation.revision(),
                actual: artifact.scene_identity().clone(),
                actual_revision: artifact.source_revision(),
            });
        }
    }
    Ok(oracle_observations
        .iter()
        .map(|probe| raster_semantic_face_observation(artifact, probe, frame_sequence))
        .collect())
}

fn raster_semantic_face_observation(
    artifact: &RasterArtifact,
    oracle: &SemanticRayProbeObservation,
    frame_sequence: u64,
) -> RasterSemanticFaceObservation {
    let correspondence = match oracle.observation().result() {
        SemanticRayResult::Miss => RasterSemanticFaceCorrespondence::NotApplicableMiss,
        SemanticRayResult::Contact(contact) => match contact.classification() {
            SemanticRayContactClassification::StartedInside => {
                RasterSemanticFaceCorrespondence::NotApplicableStartedInside
            }
            SemanticRayContactClassification::Entered(normal) => {
                let face = SemanticFace::new(
                    contact.volume_identity().clone(),
                    contact.coordinate(),
                    raster_axis_normal(normal),
                    contact.material_identity().clone(),
                );
                if artifact
                    .semantic_faces()
                    .any(|candidate| candidate == &face)
                {
                    RasterSemanticFaceCorrespondence::Matched(face)
                } else {
                    RasterSemanticFaceCorrespondence::Missing(face)
                }
            }
        },
    };
    RasterSemanticFaceObservation {
        probe_identity: oracle.probe_identity().to_owned(),
        scene_identity: artifact.scene_identity().clone(),
        revision: artifact.source_revision(),
        frame_sequence,
        correspondence,
    }
}

fn raster_axis_normal(normal: SemanticAxisNormal) -> AxisNormal {
    match normal {
        SemanticAxisNormal::NegativeX => AxisNormal::NegativeX,
        SemanticAxisNormal::PositiveX => AxisNormal::PositiveX,
        SemanticAxisNormal::NegativeY => AxisNormal::NegativeY,
        SemanticAxisNormal::PositiveY => AxisNormal::PositiveY,
        SemanticAxisNormal::NegativeZ => AxisNormal::NegativeZ,
        SemanticAxisNormal::PositiveZ => AxisNormal::PositiveZ,
    }
}
