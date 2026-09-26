use super::meshing::{
    RasterArtifact, RasterArtifactBuildError, RasterRegionIdentity, RasterRegionResult,
};
use std::fmt;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use voxel_frontend::{VoxelSceneId, VoxelSceneRevision};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterRegionResourceOwnership {
    None,
    VertexAndIndex,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterRegionInstallation {
    identity: RasterRegionIdentity,
    pub(super) resource_ownership: RasterRegionResourceOwnership,
    pub(super) installation_generation: RasterRegionInstallationGeneration,
    pub(super) gpu_resource_identity: Option<RasterRegionGpuResourceIdentity>,
    pub(super) activity: RasterRegionActivity,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RasterRegionGpuResourceIdentity {
    pub(super) region_identity: RasterRegionIdentity,
    pub(super) installation_generation: RasterRegionInstallationGeneration,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RasterRegionInstallationGeneration(u64);

impl RasterRegionInstallationGeneration {
    pub fn new(generation: u64) -> Self {
        Self(generation)
    }

    pub fn checked_successor(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RasterRegionActivity {
    pub(super) scheduling_events: u64,
    pub(super) derivation_events: u64,
    pub(super) upload_events: u64,
    pub(super) replacement_events: u64,
}

impl RasterRegionInstallation {
    pub fn identity(&self) -> &RasterRegionIdentity {
        &self.identity
    }

    pub fn resource_ownership(&self) -> RasterRegionResourceOwnership {
        self.resource_ownership
    }

    pub fn installation_generation(&self) -> RasterRegionInstallationGeneration {
        self.installation_generation
    }

    pub fn gpu_resource_identity(&self) -> Option<&RasterRegionGpuResourceIdentity> {
        self.gpu_resource_identity.as_ref()
    }

    pub fn activity(&self) -> RasterRegionActivity {
        self.activity
    }

    pub(super) fn new(
        region: &RasterRegionResult,
        has_gpu_resources: bool,
        installation_generation: RasterRegionInstallationGeneration,
    ) -> Self {
        Self {
            identity: region.identity().clone(),
            resource_ownership: if has_gpu_resources {
                RasterRegionResourceOwnership::VertexAndIndex
            } else {
                RasterRegionResourceOwnership::None
            },
            gpu_resource_identity: has_gpu_resources.then(|| RasterRegionGpuResourceIdentity {
                region_identity: region.identity().clone(),
                installation_generation,
            }),
            installation_generation,
            activity: RasterRegionActivity::default(),
        }
    }
}

impl RasterRegionActivity {
    pub fn scheduling_events(self) -> u64 {
        self.scheduling_events
    }

    pub fn derivation_events(self) -> u64 {
        self.derivation_events
    }

    pub fn upload_events(self) -> u64 {
        self.upload_events
    }

    pub fn replacement_events(self) -> u64 {
        self.replacement_events
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RasterAdjacentChangeMismatch {
    SceneIdentity {
        installed: Option<VoxelSceneId>,
        change_set: VoxelSceneId,
        successor_view: VoxelSceneId,
    },
    SuccessorRevision {
        change_set: VoxelSceneRevision,
        successor_view: VoxelSceneRevision,
    },
    PredecessorRevision {
        installed: Option<VoxelSceneRevision>,
        change_set: VoxelSceneRevision,
    },
    Adjacency {
        installed: Option<VoxelSceneRevision>,
        successor: VoxelSceneRevision,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RasterAdjacentChangeOutcome {
    Applied {
        scene_identity: VoxelSceneId,
        predecessor_revision: VoxelSceneRevision,
        successor_revision: VoxelSceneRevision,
        affected_regions: Vec<RasterRegionIdentity>,
    },
    Inapplicable {
        mismatches: Vec<RasterAdjacentChangeMismatch>,
    },
}

#[derive(Debug, Error)]
pub enum RasterAdjacentChangeError {
    #[error(transparent)]
    Derivation(#[from] RasterArtifactBuildError),
    #[error("Raster Region installation generation overflow for {identity:?}")]
    InstallationGenerationOverflow { identity: RasterRegionIdentity },
    #[error("no complete raster artifact is installed")]
    MissingInstallation,
    #[error("the successor artifact is missing installed Raster Region {identity:?}")]
    MissingSuccessorRegion { identity: RasterRegionIdentity },
    #[error("the installed artifact does not define a Raster Region grid")]
    MissingRegionGrid,
    #[error("configured Raster Region resources require a device context for replacement")]
    ConfiguredResourcesRequireDevice,
    #[error("configured GPU resources are missing Raster Region {identity:?}")]
    MissingConfiguredRegion { identity: RasterRegionIdentity },
    #[error("Raster Region resource bookkeeping could not be allocated")]
    ResourceBookkeepingAllocation,
    #[error("GPU upload failed for Raster Region {identity:?}: {source}")]
    Upload {
        identity: RasterRegionIdentity,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterArtifactInstallationPhase {
    Upload,
    PresentationConfiguration,
    Record,
}

impl fmt::Display for RasterArtifactInstallationPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Upload => "upload",
            Self::PresentationConfiguration => "presentation configuration",
            Self::Record => "record",
        })
    }
}

#[derive(Debug, Error)]
#[error("raster artifact {phase} failed for Voxel Scene Revision {source_revision}: {source}")]
pub struct RasterArtifactInstallationError {
    phase: RasterArtifactInstallationPhase,
    source_revision: VoxelSceneRevision,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

pub(super) struct RasterArtifactInstallationState {
    pub(super) expected_scene_identity: Option<VoxelSceneId>,
    pub(super) expected_revision: VoxelSceneRevision,
    pub(super) staged_artifact: Option<RasterArtifact>,
    pub(super) artifact_was_published: bool,
    pub(super) installed_revision: Option<VoxelSceneRevision>,
    #[cfg(any(test, feature = "qualification"))]
    pub(super) inject_upload_failure: bool,
}

#[derive(Clone)]
pub struct RasterArtifactInstaller {
    pub(super) state: Arc<Mutex<RasterArtifactInstallationState>>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RasterArtifactInstallerError {
    #[error(
        "complete raster artifact Voxel Scene identity mismatch: expected {expected:?}, received {actual:?}"
    )]
    SceneIdentityMismatch {
        expected: VoxelSceneId,
        actual: VoxelSceneId,
    },
    #[error(
        "complete raster artifact revision mismatch: expected Voxel Scene Revision {expected}, received {actual}"
    )]
    RevisionMismatch {
        expected: VoxelSceneRevision,
        actual: VoxelSceneRevision,
    },
    #[error(
        "a complete raster artifact was already published for Voxel Scene Revision {source_revision}"
    )]
    AlreadyPublished { source_revision: VoxelSceneRevision },
    #[error("the raster artifact installation state is unavailable")]
    StateUnavailable,
}

impl RasterArtifactInstaller {
    #[cfg(any(test, feature = "qualification"))]
    pub fn inject_next_upload_failure(&self) -> Result<(), RasterArtifactInstallerError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        state.inject_upload_failure = true;
        Ok(())
    }

    pub fn publish_complete(
        &self,
        artifact: RasterArtifact,
    ) -> Result<(), RasterArtifactInstallerError> {
        let actual = artifact.source_revision();
        let actual_scene_identity = artifact.scene_identity().clone();
        let mut state = self
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        if let Some(expected) = &state.expected_scene_identity
            && expected != &actual_scene_identity
        {
            return Err(RasterArtifactInstallerError::SceneIdentityMismatch {
                expected: expected.clone(),
                actual: actual_scene_identity,
            });
        }
        if actual != state.expected_revision {
            return Err(RasterArtifactInstallerError::RevisionMismatch {
                expected: state.expected_revision,
                actual,
            });
        }
        if state.artifact_was_published {
            return Err(RasterArtifactInstallerError::AlreadyPublished {
                source_revision: actual,
            });
        }
        state.staged_artifact = Some(artifact);
        state.artifact_was_published = true;
        Ok(())
    }

    pub fn staged_source_revision(
        &self,
    ) -> Result<Option<VoxelSceneRevision>, RasterArtifactInstallerError> {
        let state = self
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        Ok(state
            .staged_artifact
            .as_ref()
            .map(RasterArtifact::source_revision))
    }

    pub fn installed_source_revision(
        &self,
    ) -> Result<Option<VoxelSceneRevision>, RasterArtifactInstallerError> {
        let state = self
            .state
            .lock()
            .map_err(|_| RasterArtifactInstallerError::StateUnavailable)?;
        Ok(state.installed_revision)
    }
}

impl RasterArtifactInstallationError {
    pub fn new(
        phase: RasterArtifactInstallationPhase,
        source_revision: VoxelSceneRevision,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self {
            phase,
            source_revision,
            source,
        }
    }

    pub fn phase(&self) -> RasterArtifactInstallationPhase {
        self.phase
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }
}
