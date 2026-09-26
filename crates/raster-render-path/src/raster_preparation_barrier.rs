use super::{RasterArtifactPreparationError, RasterArtifactPreparationEvent};
use std::sync::{Arc, Condvar, Mutex};
use thiserror::Error;
use voxel_frontend::VoxelSceneRevision;

#[derive(Default)]
struct RasterPreparationBarrierState {
    reached: bool,
    released: bool,
}

pub(super) struct RasterPreparationBarrierShared {
    state: Mutex<RasterPreparationBarrierState>,
    released: Condvar,
}

pub struct RasterPreparationBarrier {
    pub(super) shared: Arc<RasterPreparationBarrierShared>,
}

pub struct RasterPreparationBarrierRelease {
    pub(super) shared: Arc<RasterPreparationBarrierShared>,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("the raster preparation barrier state is unavailable")]
pub struct RasterPreparationBarrierError;

impl RasterPreparationBarrier {
    #[cfg(any(test, feature = "qualification"))]
    pub fn held() -> (Self, RasterPreparationBarrierRelease) {
        let shared = Arc::new(RasterPreparationBarrierShared {
            state: Mutex::new(RasterPreparationBarrierState::default()),
            released: Condvar::new(),
        });
        (
            Self {
                shared: shared.clone(),
            },
            RasterPreparationBarrierRelease { shared },
        )
    }

    pub(super) fn reach_and_wait(
        &self,
        source_revision: VoxelSceneRevision,
        notify: &impl Fn(RasterArtifactPreparationEvent),
    ) -> Result<(), RasterArtifactPreparationError> {
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| RasterArtifactPreparationError::Synchronization { source_revision })?;
        state.reached = true;
        notify(RasterArtifactPreparationEvent::PausedAtBarrier { source_revision });
        while !state.released {
            state =
                self.shared.released.wait(state).map_err(|_| {
                    RasterArtifactPreparationError::Synchronization { source_revision }
                })?;
        }
        Ok(())
    }
}

impl RasterPreparationBarrierRelease {
    pub fn release(&self) -> Result<(), RasterPreparationBarrierError> {
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| RasterPreparationBarrierError)?;
        state.released = true;
        self.shared.released.notify_one();
        Ok(())
    }

    pub fn was_reached(&self) -> Result<bool, RasterPreparationBarrierError> {
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| RasterPreparationBarrierError)?;
        Ok(state.reached)
    }
}

impl Drop for RasterPreparationBarrierRelease {
    fn drop(&mut self) {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                eprintln!("raster preparation barrier was poisoned during implicit release");
                poisoned.into_inner()
            }
        };
        state.released = true;
        self.shared.released.notify_one();
    }
}
