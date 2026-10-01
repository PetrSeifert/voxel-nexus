use super::*;
use std::sync::TryLockError;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VoxelResidencySelectionId(u64);

impl VoxelResidencySelectionId {
    pub fn new(identity: u64) -> Self {
        Self(identity)
    }
}

impl fmt::Display for VoxelResidencySelectionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The caller-chosen maximum Voxel Residency Selection size, N, from which every streamed
/// residency limit derives, so the frontend and Render Paths cannot disagree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoxelResidencyLimits {
    maximum_selection_volumes: usize,
}

impl VoxelResidencyLimits {
    pub fn new(maximum_selection_volumes: usize) -> Result<Self, VoxelFrontendError> {
        if maximum_selection_volumes == 0 {
            return Err(VoxelFrontendError::ZeroResidencyLimit);
        }
        if maximum_selection_volumes
            .checked_mul(2)
            .and_then(|copies| copies.checked_add(1))
            .is_none()
        {
            return Err(VoxelFrontendError::ResidencyLimitTooLarge {
                maximum_selection_volumes,
            });
        }
        Ok(Self {
            maximum_selection_volumes,
        })
    }

    pub fn maximum_selection_volumes(&self) -> usize {
        self.maximum_selection_volumes
    }

    /// 2N: one Render Path owner retains its installed and newest selections.
    pub fn retained_representation_copies(&self) -> usize {
        2 * self.maximum_selection_volumes
    }

    /// 2N + 1: the old and newest selections plus one query-only copy.
    pub fn materialization_copy_cap(&self) -> usize {
        2 * self.maximum_selection_volumes + 1
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoxelResidencySelection {
    identity: VoxelResidencySelectionId,
    scene: VoxelSceneId,
    volumes: Arc<[VoxelVolumeId]>,
}

impl VoxelResidencySelection {
    pub fn identity(&self) -> VoxelResidencySelectionId {
        self.identity
    }

    pub fn scene_id(&self) -> &VoxelSceneId {
        &self.scene
    }

    pub fn volumes(&self) -> &[VoxelVolumeId] {
        &self.volumes
    }

    fn validate(&self, view: &VoxelSceneView) -> Result<(), VoxelFrontendError> {
        if self.scene != *view.scene_id() {
            return Err(VoxelFrontendError::ResidencySceneMismatch);
        }
        for identity in self.volumes.iter() {
            view.volume_content_version(identity)?;
        }
        Ok(())
    }
}

impl VoxelSceneView {
    pub fn residency_limits(&self) -> Result<VoxelResidencyLimits, VoxelFrontendError> {
        self.published
            .residency_limits
            .ok_or(VoxelFrontendError::ResidencyLimitsUndeclared)
    }

    pub fn residency_selection(
        &self,
        identity: VoxelResidencySelectionId,
        volumes: impl IntoIterator<Item = VoxelVolumeId>,
    ) -> Result<VoxelResidencySelection, VoxelFrontendError> {
        let mut volumes: Vec<_> = volumes.into_iter().collect();
        volumes.sort();
        volumes.dedup();
        if volumes.is_empty() {
            return Err(VoxelFrontendError::EmptyResidencySelection);
        }
        let selection = VoxelResidencySelection {
            identity,
            scene: self.scene_id().clone(),
            volumes: volumes.into(),
        };
        selection.validate(self)?;
        Ok(selection)
    }
}

/// Keeps every selected copy alive without pinning payloads through the Scene View itself.
#[derive(Clone)]
pub struct VoxelResidencyCopies {
    selection: VoxelResidencySelection,
    view: VoxelSceneView,
    copies: Vec<ReadStorage>,
}

impl VoxelResidencyCopies {
    pub fn selection(&self) -> &VoxelResidencySelection {
        &self.selection
    }

    pub fn scene_view(&self) -> &VoxelSceneView {
        &self.view
    }

    pub fn storage_bytes(&self) -> usize {
        self.copies.iter().map(|copy| copy.storage_bytes()).sum()
    }
}

fn cached_selection(
    selection: &VoxelResidencySelection,
    view: &VoxelSceneView,
) -> Result<Option<VoxelResidencyCopies>, VoxelFrontendError> {
    let mut copies = Vec::with_capacity(selection.volumes.len());
    for identity in selection.volumes.iter() {
        let Some(copy) = view.published.cached_selection_storage(identity)? else {
            return Ok(None);
        };
        copies.push(copy);
    }
    Ok(Some(VoxelResidencyCopies {
        selection: selection.clone(),
        view: view.clone(),
        copies,
    }))
}

#[derive(Default)]
pub(super) struct ResidencyState {
    required: Option<(VoxelResidencySelection, VoxelSceneView)>,
    installed: Option<VoxelResidencyCopies>,
}

impl VoxelFrontend {
    /// Accepts strictly newer identities. Submission does no generation or scene publication.
    pub fn require_residency(
        &self,
        selection: VoxelResidencySelection,
    ) -> Result<bool, VoxelFrontendError> {
        let view = self.scene_view()?;
        selection.validate(&view)?;
        let mut state = self
            .residency
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        if let Some((required, _)) = &state.required {
            if selection.identity == required.identity && selection != *required {
                return Err(VoxelFrontendError::ResidencyIdentityConflict);
            }
            if selection.identity <= required.identity {
                return Ok(false);
            }
        }
        state.required = Some((selection, view));
        Ok(true)
    }

    pub fn required_residency(
        &self,
    ) -> Result<Option<VoxelResidencySelection>, VoxelFrontendError> {
        let state = self
            .residency
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        Ok(state
            .required
            .as_ref()
            .map(|(selection, _)| selection.clone()))
    }

    pub fn installed_residency(
        &self,
    ) -> Result<Option<VoxelResidencySelection>, VoxelFrontendError> {
        let state = self
            .residency
            .lock()
            .map_err(|_| VoxelFrontendError::StateUnavailable)?;
        Ok(state
            .installed
            .as_ref()
            .map(|copies| copies.selection.clone()))
    }

    /// Returns shared holds at this view, including historical views of this publication.
    /// Dropping the final consumer hold releases payloads no installed state still needs.
    pub fn materialize_residency(
        &self,
        selection: &VoxelResidencySelection,
        view: &VoxelSceneView,
    ) -> Result<VoxelResidencyCopies, VoxelFrontendError> {
        self.materialize_residency_until_cancelled(selection, view, || false)
    }

    /// Cancellation releases partially acquired copies before returning. Generation
    /// already in progress finishes, but no later volume is admitted.
    pub fn materialize_residency_until_cancelled(
        &self,
        selection: &VoxelResidencySelection,
        view: &VoxelSceneView,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<VoxelResidencyCopies, VoxelFrontendError> {
        selection.validate(view)?;
        if cancelled() {
            return Err(VoxelFrontendError::ResidencySuperseded);
        }
        let current = self.scene_view()?;
        // Revisions share the fixed palette map, while separate publications never do.
        if !Arc::ptr_eq(
            &view.published.material_indices,
            &current.published.material_indices,
        ) {
            return Err(VoxelFrontendError::ResidencySceneMismatch);
        }
        if let Some(copies) = cached_selection(selection, view)? {
            view.published.promote_selection(selection.volumes())?;
            return Ok(copies);
        }
        let _worker = match self.residency_worker.try_lock() {
            Ok(worker) => worker,
            Err(TryLockError::WouldBlock) => {
                return Err(VoxelFrontendError::MaterializationInProgress);
            }
            Err(TryLockError::Poisoned(_)) => return Err(VoxelFrontendError::StateUnavailable),
        };
        let required = self.required_residency()?;
        let mut copies = Vec::with_capacity(selection.volumes.len());
        for identity in selection.volumes.iter() {
            let admission = {
                let state = self
                    .residency
                    .lock()
                    .map_err(|_| VoxelFrontendError::StateUnavailable)?;
                if cancelled()
                    || state.required.as_ref().map(|(selection, _)| selection) != required.as_ref()
                {
                    return Err(VoxelFrontendError::ResidencySuperseded);
                }
                view.published.prepare_storage(identity, true)?
            };
            let generated = admission.finish()?;
            let state = self
                .residency
                .lock()
                .map_err(|_| VoxelFrontendError::StateUnavailable)?;
            if cancelled()
                || state.required.as_ref().map(|(selection, _)| selection) != required.as_ref()
            {
                return Err(VoxelFrontendError::ResidencySuperseded);
            }
            copies.push(generated);
        }
        view.published.promote_selection(selection.volumes())?;
        Ok(VoxelResidencyCopies {
            selection: selection.clone(),
            view: view.clone(),
            copies,
        })
    }

    /// Runs one establishment worker on the caller's thread. Other callers return false
    /// while it runs; the owner can call this on its worker thread and retry typed failures.
    pub fn establish_residency(&self) -> Result<bool, VoxelFrontendError> {
        let _worker = match self.residency_worker.try_lock() {
            Ok(worker) => worker,
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Poisoned(_)) => return Err(VoxelFrontendError::StateUnavailable),
        };
        'targets: loop {
            let (selection, view) = {
                let state = self
                    .residency
                    .lock()
                    .map_err(|_| VoxelFrontendError::StateUnavailable)?;
                let Some((selection, view)) = &state.required else {
                    return Ok(false);
                };
                if state
                    .installed
                    .as_ref()
                    .is_some_and(|copies| copies.selection == *selection)
                {
                    return Ok(true);
                }
                (selection.clone(), view.clone())
            };
            let mut copies = Vec::with_capacity(selection.volumes.len());
            for identity in selection.volumes.iter() {
                let admission = {
                    // Submission and admission share this lock, so supersession cannot
                    // slip between the newest-target check and reserving the next volume.
                    let state = self
                        .residency
                        .lock()
                        .map_err(|_| VoxelFrontendError::StateUnavailable)?;
                    if !state
                        .required
                        .as_ref()
                        .is_some_and(|(required, _)| *required == selection)
                    {
                        continue 'targets;
                    }
                    view.published.prepare_storage(identity, true)?
                };
                let generated = admission.finish()?;
                let state = self
                    .residency
                    .lock()
                    .map_err(|_| VoxelFrontendError::StateUnavailable)?;
                if !state
                    .required
                    .as_ref()
                    .is_some_and(|(required, _)| *required == selection)
                {
                    // The current copy and all earlier pending copies drop before the
                    // next target can reserve.
                    drop(generated);
                    continue 'targets;
                }
                copies.push(generated);
            }
            let copies = VoxelResidencyCopies {
                selection: selection.clone(),
                view,
                copies,
            };
            let mut state = self
                .residency
                .lock()
                .map_err(|_| VoxelFrontendError::StateUnavailable)?;
            if state
                .required
                .as_ref()
                .is_some_and(|(required, _)| *required == selection)
            {
                copies
                    .view
                    .published
                    .promote_selection(selection.volumes())?;
                state.installed = Some(copies);
                return Ok(true);
            }
            drop(copies);
        }
    }
}
