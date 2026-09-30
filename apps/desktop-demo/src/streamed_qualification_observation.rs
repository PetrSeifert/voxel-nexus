use super::{
    allocation::{self, Category},
    streamed_residency_probes::Probes,
};
use compute_ray_render_path::ComputeRayRenderPathAdapter;
use raster_render_path::{RasterConvergenceFailurePhase, RasterRenderPathAdapter};
use render_backend::*;
use std::{cell::RefCell, rc::Rc, time::Instant};
use voxel_frontend::{VoxelEditOutcome, VoxelResidencySelection};

pub enum Path {
    Raster(Box<RasterRenderPathAdapter>),
    Brickmap(Box<ComputeRayRenderPathAdapter>),
}

#[derive(Default)]
pub struct PathObservation {
    pub owned: bool,
    pub raster: bool,
    pub fail_upload: bool,
    pub retry: bool,
    pub upload_failed: bool,
    pub workers: usize,
    pub representation_copies: usize,
}

pub struct MeasuredPath {
    pub path: Path,
    pub identity: u64,
    pub observation: Rc<RefCell<PathObservation>>,
    pub raster_control: Option<raster_render_path::RasterLifecycleController>,
}
impl MeasuredPath {
    fn inner(&mut self) -> &mut dyn SwitchableRenderPath {
        match &mut self.path {
            Path::Raster(path) => path.as_mut(),
            Path::Brickmap(path) => path.as_mut(),
        }
    }
    fn category(&self) -> Category {
        match self.path {
            Path::Raster(_) => Category::Raster,
            Path::Brickmap(_) => Category::Brickmap,
        }
    }
    fn operation<T>(&mut self, operation: impl FnOnce(&mut dyn SwitchableRenderPath) -> T) -> T {
        let category = self.category();
        let class = match category {
            Category::Raster => GpuAllocationClass::Raster,
            _ => GpuAllocationClass::Brickmap,
        };
        allocation::within(category, || {
            with_gpu_allocation_owner(self.identity, class, 0, || operation(self.inner()))
        })
    }
    pub fn probes(&mut self) -> Probes {
        match &mut self.path {
            Path::Raster(path) => Probes::Raster(path.enable_semantic_face_observation()),
            Path::Brickmap(path) => Probes::Brickmap(path.enable_semantic_ray_observation()),
        }
    }
}
impl RenderPath for MeasuredPath {
    fn installed_residency_coverage(&self) -> Option<&RenderPathCoverage> {
        match &self.path {
            Path::Raster(path) => path.installed_residency_coverage(),
            Path::Brickmap(path) => path.installed_residency_coverage(),
        }
    }
    fn submit_residency_selection(
        &mut self,
        selection: VoxelResidencySelection,
    ) -> RenderPathResult<()> {
        self.operation(|path| path.submit_residency_selection(selection))
    }
    fn submit_edit_outcome(&mut self, outcome: VoxelEditOutcome) -> RenderPathResult<()> {
        self.operation(|path| path.submit_edit_outcome(outcome))
    }
    fn publish_camera_state(
        &mut self,
        camera: CameraState,
        revision: CameraStateRevision,
    ) -> RenderPathResult<()> {
        self.operation(|path| path.publish_camera_state(camera, revision))
    }
    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.operation(|path| path.release(device))
    }
    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.operation(|path| path.configure(device, target))
    }
    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        if let Path::Raster(path) = &mut self.path {
            let mut observation = self.observation.borrow_mut();
            if std::mem::take(&mut observation.fail_upload) {
                path.qualification_fail_next_convergence(RasterConvergenceFailurePhase::Upload)?;
            }
            if std::mem::take(&mut observation.retry) {
                path.qualification_request_retry()?;
            }
        }
        let result = self.operation(|path| path.advance_frame_boundary(device, target));
        if let Path::Raster(path) = &mut self.path {
            let control = self
                .raster_control
                .as_ref()
                .expect("a measured raster path has lifecycle observation");
            let mut observation = self.observation.borrow_mut();
            observation.upload_failed |= path.qualification_drain_convergence_events()?.iter().any(|event| matches!(event,raster_render_path::RasterConvergenceEvent::Failure { failure } if failure.phase()==RasterConvergenceFailurePhase::Upload));
            if let Some(status) = control.residency_status()? {
                observation.workers = status.workers;
                observation.representation_copies = status.representation_copies;
            }
        } else if let Path::Brickmap(path) = &self.path {
            let status = path.convergence_status();
            let mut observation = self.observation.borrow_mut();
            observation.workers = status.worker_count();
            observation.representation_copies = path.residency_held_copy_count();
        }
        result
    }
    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.operation(|path| path.shutdown(device))?;
        let mut observation = self.observation.borrow_mut();
        match &self.path {
            Path::Raster(_) => {
                let control = self
                    .raster_control
                    .as_ref()
                    .expect("a measured raster path has lifecycle observation");
                let status = control
                    .residency_status()?
                    .ok_or("missing Raster shutdown residency observation")?;
                observation.workers = status.workers;
                observation.representation_copies = status.representation_copies;
                if control.shutdown_owned_resource_count()? != Some(0) {
                    return Err("Raster shutdown retained resources or workers".into());
                }
            }
            Path::Brickmap(path) => {
                observation.workers = path.convergence_status().worker_count();
                observation.representation_copies = path.residency_held_copy_count();
            }
        }
        observation.owned = false;
        Ok(())
    }
    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        self.operation(|path| path.record(frame))
    }
}
impl SwitchableRenderPath for MeasuredPath {
    fn stamp(&self) -> RenderPathStamp {
        match &self.path {
            Path::Raster(path) => path.stamp(),
            Path::Brickmap(path) => path.stamp(),
        }
    }
    fn retire_at_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
    ) -> RenderPathResult<RenderPathRetirement> {
        let retirement = self.operation(|path| path.retire_at_frame_boundary(device))?;
        if retirement == RenderPathRetirement::Complete {
            self.observation.borrow_mut().owned = false;
        }
        Ok(retirement)
    }
}

#[derive(Default)]
pub struct BoundaryObservation {
    pub at: Option<Instant>,
}

pub struct ObservedOwner {
    pub owner: RenderPathSwitchOwner,
    pub observation: Rc<RefCell<BoundaryObservation>>,
}
impl RenderPath for ObservedOwner {
    fn installed_residency_coverage(&self) -> Option<&RenderPathCoverage> {
        self.owner.installed_residency_coverage()
    }
    fn submit_residency_selection(
        &mut self,
        selection: VoxelResidencySelection,
    ) -> RenderPathResult<()> {
        self.owner.submit_residency_selection(selection)
    }
    fn submit_edit_outcome(&mut self, outcome: VoxelEditOutcome) -> RenderPathResult<()> {
        self.owner.submit_edit_outcome(outcome)
    }
    fn publish_camera_state(
        &mut self,
        camera: CameraState,
        revision: CameraStateRevision,
    ) -> RenderPathResult<()> {
        self.owner.publish_camera_state(camera, revision)
    }
    fn switch_diagnostics(&self) -> Option<RenderPathSwitchDiagnostics> {
        Some(self.owner.diagnostics())
    }
    fn request_switch(
        &mut self,
        replacement: Box<dyn SwitchableRenderPath>,
    ) -> Result<(), RenderPathSwitchRequestError> {
        self.owner.request_switch(replacement)
    }
    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.owner.release(device)
    }
    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.owner.configure(device, target)
    }
    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.owner.advance_frame_boundary(device, target)?;
        self.observation.borrow_mut().at = Some(Instant::now());
        Ok(())
    }
    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.owner.shutdown(device)
    }
    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        self.owner.record(frame)
    }
}
