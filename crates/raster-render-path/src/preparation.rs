use super::meshing::{
    RasterArtifact, RasterArtifactBuildError, derive_raster_artifact,
    derive_raster_regions_until_cancelled, metadata_dimensions_error,
};
use super::worker_pool::{RasterTask, RasterWorkerPool};
#[cfg(any(test, feature = "qualification"))]
use super::{RasterPreparationBarrier, RasterPreparationBarrierRelease};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use thiserror::Error;
use voxel_frontend::{
    VoxelCoordinate, VoxelExtent, VoxelSceneRevision, VoxelSceneView, VoxelVolumeId,
};

pub(super) fn raster_region_origin(
    source_revision: VoxelSceneRevision,
    origin_x: u32,
    origin_y: u32,
    origin_z: u32,
) -> Result<VoxelCoordinate, RasterArtifactBuildError> {
    Ok(VoxelCoordinate::new(
        i32::try_from(origin_x).map_err(|_| metadata_dimensions_error(source_revision))?,
        i32::try_from(origin_y).map_err(|_| metadata_dimensions_error(source_revision))?,
        i32::try_from(origin_z).map_err(|_| metadata_dimensions_error(source_revision))?,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterArtifactPreparationEvent {
    #[cfg(any(test, feature = "qualification"))]
    PausedAtBarrier {
        source_revision: VoxelSceneRevision,
    },
    Completed {
        source_revision: VoxelSceneRevision,
    },
}

#[derive(Debug, Error)]
pub enum RasterArtifactPreparationError {
    #[error("background derivation failed for Voxel Scene Revision {source_revision:?}: {source}")]
    Derivation {
        source_revision: VoxelSceneRevision,
        #[source]
        source: RasterArtifactBuildError,
    },
    #[cfg(any(test, feature = "qualification"))]
    #[error(
        "background preparation synchronization failed for Voxel Scene Revision {source_revision:?}"
    )]
    Synchronization { source_revision: VoxelSceneRevision },
    #[error(
        "background preparation worker terminated unexpectedly for Voxel Scene Revision {source_revision:?}"
    )]
    WorkerTerminated { source_revision: VoxelSceneRevision },
    #[error(
        "could not start background preparation for Voxel Scene Revision {source_revision:?}: {source}"
    )]
    WorkerStart {
        source_revision: VoxelSceneRevision,
        #[source]
        source: std::io::Error,
    },
}

impl RasterArtifactPreparationError {
    pub fn source_revision(&self) -> VoxelSceneRevision {
        match self {
            #[cfg(any(test, feature = "qualification"))]
            Self::Synchronization { source_revision } => *source_revision,
            Self::Derivation {
                source_revision, ..
            }
            | Self::WorkerTerminated { source_revision }
            | Self::WorkerStart {
                source_revision, ..
            } => *source_revision,
        }
    }
}

pub struct RasterArtifactPreparation {
    source_revision: VoxelSceneRevision,
    result_receiver: mpsc::Receiver<Result<Option<RasterArtifact>, RasterArtifactPreparationError>>,
    worker: Option<RasterTask<()>>,
    worker_pool: Option<Arc<RasterWorkerPool>>,
    cancellation: Arc<AtomicBool>,
    #[cfg(any(test, feature = "qualification"))]
    cancellation_barrier: Option<RasterPreparationBarrierRelease>,
}

impl RasterArtifactPreparation {
    pub fn start_regions(
        view: VoxelSceneView,
        region_extent: VoxelExtent,
        notify: impl Fn(RasterArtifactPreparationEvent) + Send + 'static,
    ) -> Result<Self, RasterArtifactPreparationError> {
        let source_revision = view.revision();
        Self::start_with_derivation(
            source_revision,
            #[cfg(any(test, feature = "qualification"))]
            None,
            notify,
            move |cancellation, pool| {
                derive_raster_regions_until_cancelled(&view, region_extent, cancellation, pool)
            },
        )
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn start_regions_with_barrier(
        view: VoxelSceneView,
        region_extent: VoxelExtent,
        barrier: Option<RasterPreparationBarrier>,
        notify: impl Fn(RasterArtifactPreparationEvent) + Send + 'static,
    ) -> Result<Self, RasterArtifactPreparationError> {
        let source_revision = view.revision();
        Self::start_with_derivation(
            source_revision,
            barrier,
            notify,
            move |cancellation, pool| {
                derive_raster_regions_until_cancelled(&view, region_extent, cancellation, pool)
            },
        )
    }

    pub fn start(
        view: VoxelSceneView,
        volume_identity: VoxelVolumeId,
        notify: impl Fn(RasterArtifactPreparationEvent) + Send + 'static,
    ) -> Result<Self, RasterArtifactPreparationError> {
        let source_revision = view.revision();
        Self::start_with_derivation(
            source_revision,
            #[cfg(any(test, feature = "qualification"))]
            None,
            notify,
            move |_, _| derive_raster_artifact(&view, &volume_identity).map(Some),
        )
    }

    #[cfg(any(test, feature = "qualification"))]
    pub fn start_with_barrier(
        view: VoxelSceneView,
        volume_identity: VoxelVolumeId,
        barrier: Option<RasterPreparationBarrier>,
        notify: impl Fn(RasterArtifactPreparationEvent) + Send + 'static,
    ) -> Result<Self, RasterArtifactPreparationError> {
        let source_revision = view.revision();
        Self::start_with_derivation(source_revision, barrier, notify, move |_, _| {
            derive_raster_artifact(&view, &volume_identity).map(Some)
        })
    }

    fn start_with_derivation(
        source_revision: VoxelSceneRevision,
        #[cfg(any(test, feature = "qualification"))] barrier: Option<RasterPreparationBarrier>,
        notify: impl Fn(RasterArtifactPreparationEvent) + Send + 'static,
        derive: impl FnOnce(
            Arc<AtomicBool>,
            &RasterWorkerPool,
        ) -> Result<Option<RasterArtifact>, RasterArtifactBuildError>
        + Send
        + 'static,
    ) -> Result<Self, RasterArtifactPreparationError> {
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let cancellation = Arc::new(AtomicBool::new(false));
        #[cfg(any(test, feature = "qualification"))]
        let cancellation_barrier =
            barrier
                .as_ref()
                .map(|barrier| RasterPreparationBarrierRelease {
                    shared: barrier.shared.clone(),
                });
        let worker_pool = Arc::new(RasterWorkerPool::new().map_err(|source| {
            RasterArtifactPreparationError::WorkerStart {
                source_revision,
                source,
            }
        })?);
        let worker_task = {
            let worker_pool = worker_pool.clone();
            let cancellation = cancellation.clone();
            move || {
                let result = (|| {
                    #[cfg(any(test, feature = "qualification"))]
                    if let Some(barrier) = barrier {
                        barrier.reach_and_wait(source_revision, &notify)?;
                    }
                    if cancellation.load(Ordering::Acquire) {
                        return Ok(None);
                    }
                    derive(cancellation, &worker_pool).map_err(|source| {
                        RasterArtifactPreparationError::Derivation {
                            source_revision,
                            source,
                        }
                    })
                })();
                let should_notify = !matches!(&result, Ok(None));
                if result_sender.send(result).is_err() {
                    eprintln!(
                        "background raster preparation result receiver closed for Voxel Scene Revision {source_revision}"
                    );
                    return;
                }
                if should_notify {
                    notify(RasterArtifactPreparationEvent::Completed { source_revision });
                }
            }
        };
        let worker = worker_pool.execute(worker_task).map_err(|source| {
            RasterArtifactPreparationError::WorkerStart {
                source_revision,
                source,
            }
        })?;
        Ok(Self {
            source_revision,
            result_receiver,
            worker: Some(worker),
            worker_pool: Some(worker_pool),
            cancellation,
            #[cfg(any(test, feature = "qualification"))]
            cancellation_barrier,
        })
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }

    pub fn try_complete(
        &mut self,
    ) -> Result<Option<RasterArtifact>, RasterArtifactPreparationError> {
        let result = match self.result_receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(RasterArtifactPreparationError::WorkerTerminated {
                    source_revision: self.source_revision,
                });
            }
        };
        let worker =
            self.worker
                .take()
                .ok_or(RasterArtifactPreparationError::WorkerTerminated {
                    source_revision: self.source_revision,
                })?;
        if worker.join().is_err() {
            return Err(RasterArtifactPreparationError::WorkerTerminated {
                source_revision: self.source_revision,
            });
        }
        self.worker_pool.take();
        result
    }

    pub fn cancel_and_join(&mut self) -> Result<(), RasterArtifactPreparationError> {
        self.cancellation.store(true, Ordering::Release);
        #[cfg(any(test, feature = "qualification"))]
        let barrier_result = self
            .cancellation_barrier
            .take()
            .map(|barrier| barrier.release())
            .transpose()
            .map(|_| ())
            .map_err(|_| RasterArtifactPreparationError::Synchronization {
                source_revision: self.source_revision,
            });
        #[cfg(not(any(test, feature = "qualification")))]
        let barrier_result = Ok(());
        let worker_failed = self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err());
        self.worker_pool.take();
        if worker_failed {
            return Err(RasterArtifactPreparationError::WorkerTerminated {
                source_revision: self.source_revision,
            });
        }
        barrier_result
    }
}

impl Drop for RasterArtifactPreparation {
    fn drop(&mut self) {
        self.cancellation.store(true, Ordering::Release);
        #[cfg(any(test, feature = "qualification"))]
        if let Some(barrier) = self.cancellation_barrier.take()
            && barrier.release().is_err()
        {
            eprintln!(
                "background raster preparation barrier was unavailable during cancellation for Voxel Scene Revision {}",
                self.source_revision
            );
        }
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            eprintln!(
                "background raster preparation worker panicked for Voxel Scene Revision {}",
                self.source_revision
            );
        }
    }
}
