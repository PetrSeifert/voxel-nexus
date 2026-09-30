use super::*;
use std::collections::BTreeSet;
use std::sync::Mutex;

type EditCoordinate = (VoxelVolumeId, VoxelCoordinate);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamedEditStatistics {
    pub current_coordinates: usize,
    /// Includes restore markers still needed by historical views.
    pub retained_entries: usize,
    pub live_versions: usize,
    /// Estimated edit-state bytes, excluding allocator and B-tree node overhead and source recipes.
    pub storage_bytes: usize,
}

impl VoxelFrontend {
    /// Accounts for current and historical streamed edits without materializing any volume.
    /// Unstreamed scenes return `None`.
    pub fn streamed_edit_statistics(
        &self,
    ) -> Result<Option<StreamedEditStatistics>, VoxelFrontendError> {
        let view = self.scene_view()?;
        view.published
            .streamed
            .as_ref()
            .map(|streamed| streamed.overlay.statistics())
            .transpose()
    }
}

#[derive(Clone, Copy)]
struct OverlayValue {
    revision: u64,
    value: Option<MaterialIndex>,
}

#[derive(Default)]
struct EditState {
    live_revisions: BTreeSet<u64>,
    values: HashMap<EditCoordinate, Vec<OverlayValue>>,
}

#[derive(Default)]
struct EditHistory {
    state: Mutex<EditState>,
}

pub(super) struct OverlayVersion {
    history: Arc<EditHistory>,
    revision: u64,
}

impl OverlayVersion {
    pub(super) fn new(revision: VoxelSceneRevision) -> Arc<Self> {
        #[cfg(feature = "qualification")]
        let _allocation_scope =
            QualificationAllocationScope::enter(QualificationAllocationCategory::History);
        let mut state = EditState::default();
        state.live_revisions.insert(revision.0);
        Arc::new(Self {
            history: Arc::new(EditHistory {
                state: Mutex::new(state),
            }),
            revision: revision.0,
        })
    }

    pub(super) fn value(
        &self,
        identity: &VoxelVolumeId,
        coordinate: VoxelCoordinate,
    ) -> Result<Option<MaterialIndex>, VoxelFrontendError> {
        let state = self
            .history
            .state
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        Ok(state
            .values
            .get(&(identity.clone(), coordinate))
            .and_then(|values| {
                values
                    .iter()
                    .rev()
                    .find(|value| value.revision <= self.revision)
            })
            .and_then(|value| value.value))
    }

    fn statistics(&self) -> Result<StreamedEditStatistics, VoxelFrontendError> {
        let state = self
            .history
            .state
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        Ok(StreamedEditStatistics {
            current_coordinates: state
                .values
                .values()
                .filter(|values| values.last().is_some_and(|value| value.value.is_some()))
                .count(),
            retained_entries: state.values.values().map(Vec::len).sum(),
            live_versions: state.live_revisions.len(),
            storage_bytes: size_of::<EditHistory>()
                + 2 * size_of::<usize>()
                + state.live_revisions.len() * (size_of::<Self>() + 2 * size_of::<usize>())
                + state.values.capacity() * (size_of::<(EditCoordinate, Vec<OverlayValue>)>() + 1)
                + state
                    .values
                    .values()
                    .map(|values| values.capacity() * size_of::<OverlayValue>())
                    .sum::<usize>()
                + state.live_revisions.len() * size_of::<u64>(),
        })
    }

    pub(super) fn changes(
        &self,
        identity: &VoxelVolumeId,
    ) -> Result<Vec<(VoxelCoordinate, MaterialIndex)>, VoxelFrontendError> {
        let state = self
            .history
            .state
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        Ok(state
            .values
            .iter()
            .filter_map(|((volume, coordinate), values)| {
                if volume != identity {
                    return None;
                }
                let value = values
                    .iter()
                    .rev()
                    .find(|value| value.revision <= self.revision)?
                    .value?;
                Some((*coordinate, value))
            })
            .collect())
    }

    pub(super) fn successor(
        &self,
        revision: VoxelSceneRevision,
        changes: &HashMap<VoxelVolumeId, Vec<(VoxelCoordinate, MaterialIndex)>>,
        generated: &HashMap<EditCoordinate, MaterialIndex>,
    ) -> Result<Arc<Self>, VoxelFrontendError> {
        #[cfg(feature = "qualification")]
        let _allocation_scope =
            QualificationAllocationScope::enter(QualificationAllocationCategory::History);
        let mut state = self
            .history
            .state
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        for (identity, changes) in changes {
            for &(coordinate, value) in changes {
                let key = (identity.clone(), coordinate);
                let base = generated
                    .get(&key)
                    .expect("each changed streamed coordinate was evaluated before publication");
                let history = state.values.entry(key).or_default();
                history.push(OverlayValue {
                    revision: revision.0,
                    value: (value != *base).then_some(value),
                });
                history.shrink_to_fit();
            }
        }
        state.live_revisions.insert(revision.0);
        Ok(Arc::new(Self {
            history: self.history.clone(),
            revision: revision.0,
        }))
    }
}

impl Drop for OverlayVersion {
    fn drop(&mut self) {
        let mut state = match self.history.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.live_revisions.remove(&self.revision);
        let EditState {
            live_revisions,
            values,
        } = &mut *state;
        values.retain(|_, history| {
            let mut needed = BTreeSet::new();
            for revision in live_revisions.iter() {
                if let Some(value) = history
                    .iter()
                    .rev()
                    .find(|value| value.revision <= *revision)
                {
                    needed.insert(value.revision);
                }
            }
            history.retain(|value| needed.contains(&value.revision));
            // Before the first retained edit, absence already reconstructs generated values.
            let first_edit = history
                .iter()
                .position(|value| value.value.is_some())
                .unwrap_or(history.len());
            drop(history.drain(..first_edit));
            history.shrink_to_fit();
            !history.is_empty()
        });
        values.shrink_to_fit();
    }
}
