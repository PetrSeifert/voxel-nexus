use super::lifecycle::RasterConvergenceCpuBarrierShared;
use super::meshing::{
    RasterArtifact, RasterArtifactBuildCause, RasterArtifactBuildError, RasterArtifactBuildPhase,
    RasterRegionResult, assemble_raster_artifact, build_error, derive_raster_region,
    visit_volume_region_cores,
};
use render_backend::RenderPathCoverage;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use voxel_frontend::{
    VoxelExtent, VoxelFrontend, VoxelResidencySelection, VoxelSceneId, VoxelSceneRevision,
    VoxelSceneView, VoxelVolumeId,
};

type VolumeKey = (VoxelSceneId, VoxelVolumeId, VoxelSceneRevision);

#[derive(Debug, Default)]
pub(super) struct RasterVolumeCache {
    entries: HashMap<VolumeKey, Weak<Vec<RasterRegionResult>>>,
    pub(super) derived_volumes: u64,
    constructing: usize,
}

impl RasterVolumeCache {
    pub(super) fn retained_copies(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.strong_count() > 0)
            .count()
            + self.constructing
    }
}

struct RasterVolumeConstruction {
    cache: Arc<Mutex<RasterVolumeCache>>,
    active: bool,
}

impl Drop for RasterVolumeConstruction {
    fn drop(&mut self) {
        if self.active {
            match self.cache.lock() {
                Ok(mut cache) => cache.constructing -= 1,
                Err(_) => eprintln!("Raster volume construction cache unavailable during cleanup"),
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct RasterResidencyArtifact {
    pub(super) view: VoxelSceneView,
    pub(super) frontend: Arc<VoxelFrontend>,
    pub(super) coverage: RenderPathCoverage,
    pub(super) cache: Arc<Mutex<RasterVolumeCache>>,
    pub(super) volumes: Vec<Arc<Vec<RasterRegionResult>>>,
}

impl std::fmt::Debug for RasterResidencyArtifact {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RasterResidencyArtifact")
            .field("coverage", &self.coverage)
            .field("volumes", &self.volumes.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub(super) struct RasterResidencyTarget {
    pub(super) selection: VoxelResidencySelection,
    pub(super) frontend: Arc<VoxelFrontend>,
    pub(super) cache: Arc<Mutex<RasterVolumeCache>>,
}

fn cache_error(revision: VoxelSceneRevision) -> RasterArtifactBuildError {
    build_error(
        revision,
        RasterArtifactBuildPhase::Metadata,
        RasterArtifactBuildCause::ResidencyCacheUnavailable,
    )
}

pub fn derive_raster_residency(
    frontend: Arc<VoxelFrontend>,
    view: &VoxelSceneView,
    selection: VoxelResidencySelection,
    region_extent: VoxelExtent,
) -> Result<RasterArtifact, RasterArtifactBuildError> {
    derive_selection(
        view,
        region_extent,
        &RasterResidencyTarget {
            selection,
            frontend,
            cache: Arc::new(Mutex::new(RasterVolumeCache::default())),
        },
        &AtomicBool::new(false),
        None,
    )?
    .ok_or_else(|| cache_error(view.revision()))
}

pub(super) fn derive_selection(
    view: &VoxelSceneView,
    region_extent: VoxelExtent,
    target: &RasterResidencyTarget,
    cancellation: &AtomicBool,
    barrier: Option<&RasterConvergenceCpuBarrierShared>,
) -> Result<Option<RasterArtifact>, RasterArtifactBuildError> {
    #[cfg(feature = "qualification")]
    let _allocation_scope = voxel_frontend::QualificationAllocationScope::enter(
        voxel_frontend::QualificationAllocationCategory::Raster,
    );
    let revision = view.revision();
    let coverage = RenderPathCoverage::new(view, target.selection.clone()).map_err(|source| {
        build_error(
            revision,
            RasterArtifactBuildPhase::Metadata,
            RasterArtifactBuildCause::VoxelRead(source),
        )
    })?;
    let limits = view.residency_limits().map_err(|source| {
        build_error(
            revision,
            RasterArtifactBuildPhase::Metadata,
            RasterArtifactBuildCause::VoxelRead(source),
        )
    })?;
    if target.selection.volumes().len() > limits.maximum_selection_volumes() {
        return Err(build_error(
            revision,
            RasterArtifactBuildPhase::Metadata,
            RasterArtifactBuildCause::ResidencySelectionTooLarge {
                maximum: limits.maximum_selection_volumes(),
            },
        ));
    }
    let mut volumes = Vec::new();
    for identity in target.selection.volumes() {
        if cancellation.load(Ordering::Acquire) {
            return Ok(None);
        }
        let version = view.volume_content_version(identity).map_err(|source| {
            build_error(
                revision,
                RasterArtifactBuildPhase::Metadata,
                RasterArtifactBuildCause::VoxelRead(source),
            )
        })?;
        let key = (view.scene_id().clone(), identity.clone(), version);
        let cached = {
            let mut cache = target.cache.lock().map_err(|_| cache_error(revision))?;
            cache.entries.retain(|_, entry| entry.strong_count() > 0);
            cache.entries.get(&key).and_then(Weak::upgrade)
        };
        if let Some(cached) = cached {
            volumes.push(cached);
            continue;
        }
        {
            let mut cache = target.cache.lock().map_err(|_| cache_error(revision))?;
            if cache.retained_copies() >= limits.retained_representation_copies() {
                return Err(build_error(
                    revision,
                    RasterArtifactBuildPhase::Metadata,
                    RasterArtifactBuildCause::ResidencyCopyLimit {
                        maximum: limits.retained_representation_copies(),
                    },
                ));
            }
            cache.constructing += 1;
        }
        let mut construction = RasterVolumeConstruction {
            cache: target.cache.clone(),
            active: true,
        };
        // A cancelled generation finishes its current volume, then drops it before admitting another target.
        let mut regions = Vec::new();
        let result = (|| {
            let selected_volume = view
                .residency_selection(target.selection.identity(), [identity.clone()])
                .map_err(|source| {
                    build_error(
                        revision,
                        RasterArtifactBuildPhase::Metadata,
                        RasterArtifactBuildCause::VoxelRead(source),
                    )
                })?;
            let _copy = target
                .frontend
                .materialize_residency(&selected_volume, view)
                .map_err(|source| {
                    build_error(
                        revision,
                        RasterArtifactBuildPhase::VoxelRead,
                        RasterArtifactBuildCause::VoxelRead(source),
                    )
                })?;
            let metadata = view
                .volumes()
                .iter()
                .find(|metadata| metadata.identity() == identity)
                .ok_or_else(|| cache_error(revision))?;
            visit_volume_region_cores(revision, metadata, region_extent, |metadata, core| {
                regions.push(derive_raster_region(view, metadata, core)?);
                #[cfg(any(test, feature = "qualification"))]
                if let Some(barrier) = barrier {
                    barrier
                        .schedule_and_wait(revision)
                        .map_err(|_| cache_error(revision))?;
                }
                Ok(true)
            })
        })();
        {
            let mut cache = target.cache.lock().map_err(|_| cache_error(revision))?;
            cache.constructing -= 1;
            construction.active = false;
            result?;
            cache.derived_volumes = cache.derived_volumes.saturating_add(1);
            if cancellation.load(Ordering::Acquire) {
                return Ok(None);
            }
            let regions = Arc::new(regions);
            cache.entries.insert(key, Arc::downgrade(&regions));
            volumes.push(regions);
        }
        #[cfg(not(any(test, feature = "qualification")))]
        let _barrier = barrier;
    }
    if cancellation.load(Ordering::Acquire) {
        return Ok(None);
    }
    let regions = volumes
        .iter()
        .flat_map(|volume| volume.iter().cloned())
        .collect();
    let mut artifact =
        assemble_raster_artifact(view.scene_id().clone(), revision, region_extent, regions)?;
    artifact.residency = Some(RasterResidencyArtifact {
        view: view.clone(),
        frontend: target.frontend.clone(),
        coverage,
        cache: target.cache.clone(),
        volumes,
    });
    Ok(Some(artifact))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RasterResidencyStatus {
    pub representation_copies: usize,
    pub derived_volumes: u64,
    pub workers: usize,
    pub gpu_allocations: usize,
}
