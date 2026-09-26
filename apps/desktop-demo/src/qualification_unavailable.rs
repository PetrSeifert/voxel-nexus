use compute_ray_render_path::{ComputeConvergenceController, ComputePreparationBarrierObservation};
use raster_render_path::{
    RasterArtifactInstaller, RasterConvergenceCpuBarrierObservation, RasterLifecycleController,
};

pub(super) trait ComputeQualificationUnavailable {
    fn hold_next_preparation_after_blocks(
        &self,
        completed_block_count: usize,
    ) -> Result<(), String>;
    fn release_preparation_barrier(&self) -> Result<(), String>;
    fn preparation_barrier_observation(
        &self,
    ) -> Result<Option<ComputePreparationBarrierObservation>, String>;
    fn release_post_upload(&self) -> Result<(), String>;
}

impl ComputeQualificationUnavailable for ComputeConvergenceController {
    fn hold_next_preparation_after_blocks(
        &self,
        _completed_block_count: usize,
    ) -> Result<(), String> {
        Err("this scenario requires --features qualification".to_owned())
    }
    fn release_preparation_barrier(&self) -> Result<(), String> {
        Err("this scenario requires --features qualification".to_owned())
    }
    fn preparation_barrier_observation(
        &self,
    ) -> Result<Option<ComputePreparationBarrierObservation>, String> {
        Err("this scenario requires --features qualification".to_owned())
    }
    fn release_post_upload(&self) -> Result<(), String> {
        Err("this scenario requires --features qualification".to_owned())
    }
}

pub(super) trait RasterQualificationUnavailable {
    fn hold_next_cpu_generation_after_regions(&self, region_count: usize) -> Result<(), String>;
    fn release_cpu_barrier(&self) -> Result<(), String>;
    fn cpu_barrier_observation(
        &self,
    ) -> Result<Option<RasterConvergenceCpuBarrierObservation>, String>;
    fn release_post_upload(&self) -> Result<(), String>;
}

impl RasterQualificationUnavailable for RasterLifecycleController {
    fn hold_next_cpu_generation_after_regions(&self, _region_count: usize) -> Result<(), String> {
        Err("this scenario requires --features qualification".to_owned())
    }
    fn release_cpu_barrier(&self) -> Result<(), String> {
        Err("this scenario requires --features qualification".to_owned())
    }
    fn cpu_barrier_observation(
        &self,
    ) -> Result<Option<RasterConvergenceCpuBarrierObservation>, String> {
        Err("this scenario requires --features qualification".to_owned())
    }
    fn release_post_upload(&self) -> Result<(), String> {
        Err("this scenario requires --features qualification".to_owned())
    }
}

pub(super) trait RasterUploadQualificationUnavailable {
    fn inject_next_upload_failure(&self) -> Result<(), String>;
}

impl RasterUploadQualificationUnavailable for RasterArtifactInstaller {
    fn inject_next_upload_failure(&self) -> Result<(), String> {
        Err("this scenario requires --features qualification".to_owned())
    }
}

pub(super) trait RasterHoldUnavailable {
    fn enable_lifecycle_control_with_hold(&mut self, hold: bool) -> RasterLifecycleController;
}
impl RasterHoldUnavailable for raster_render_path::RasterRenderPathAdapter {
    fn enable_lifecycle_control_with_hold(&mut self, _hold: bool) -> RasterLifecycleController {
        self.enable_lifecycle_control()
    }
}
pub(super) trait ComputeHoldUnavailable {
    fn enable_convergence_control_with_hold(&mut self, hold: bool) -> ComputeConvergenceController;
}
impl ComputeHoldUnavailable for compute_ray_render_path::ComputeRayRenderPathAdapter {
    fn enable_convergence_control_with_hold(
        &mut self,
        _hold: bool,
    ) -> ComputeConvergenceController {
        self.enable_convergence_control()
    }
}

pub(super) struct RasterPreparationBarrierRelease;
impl RasterPreparationBarrierRelease {
    pub(super) fn release(&self) -> Result<(), String> {
        Err("this scenario requires --features qualification".to_owned())
    }
}
