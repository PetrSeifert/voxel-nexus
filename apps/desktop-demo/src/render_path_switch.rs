use super::*;
use compute_ray_render_path::COMPUTE_RAY_STRATEGY;
use raster_render_path::RASTER_STRATEGY;
use render_backend::RenderPathStrategy;

pub(super) struct AdmittedRenderPathSwitch {
    pub(super) source: RenderPathStrategy,
    pub(super) replacement: RenderPathStrategy,
    view: VoxelSceneView,
    retiring_raster: Option<RasterLifecycleController>,
}

impl AdmittedRenderPathSwitch {
    pub(super) fn view(&self) -> &VoxelSceneView {
        &self.view
    }
}

pub(super) enum ReplacementRenderPath<'a> {
    Compute(&'a mut ComputeRayRenderPathAdapter),
    Raster(&'a mut RasterRenderPathAdapter),
}

impl DesktopRuntime {
    pub(super) fn build_streamed_raster(
        &self,
        view: &VoxelSceneView,
    ) -> Result<RasterRenderPathAdapter, String> {
        let frontend = self
            .frontend
            .as_ref()
            .ok_or("the Voxel Frontend is unavailable")?;
        let selection = frontend
            .required_residency()
            .map_err(|error| error.to_string())?
            .ok_or("the Required Voxel Residency Selection is unavailable")?;
        let edge = self.render_configuration.raster_region_extent;
        let artifact = raster_render_path::derive_raster_residency(
            frontend.clone(),
            view,
            selection,
            VoxelExtent::new(edge, edge, edge),
        )
        .map_err(|error| error.to_string())?;
        RasterRenderPathAdapter::from_residency_artifact(
            artifact,
            self.camera_state,
            self.camera_state_revision,
        )
        .map_err(|error| error.to_string())
    }
    pub(super) fn switch_diagnostics(&self) -> Result<RenderPathSwitchDiagnostics, String> {
        self.backend
            .as_ref()
            .and_then(RenderBackend::render_path_switch_diagnostics)
            .ok_or_else(|| "Render Path switching diagnostics are unavailable".to_owned())
    }

    pub(super) fn latest_scene_view(&self) -> Result<VoxelSceneView, String> {
        self.frontend
            .as_ref()
            .ok_or_else(|| "the Voxel Frontend is unavailable".to_owned())?
            .scene_view()
            .map_err(|error| error.to_string())
    }

    pub(super) fn admit_render_path_switch(&self) -> Result<AdmittedRenderPathSwitch, String> {
        self.render_configuration.admit_path_switch()?;
        // The previous switch still owes its retirement bookkeeping even once the roles are idle.
        if self.interactive_switch.is_some() {
            return Err(RenderPathSwitchRequestError::SwitchInProgress.to_string());
        }
        let diagnostics = self.switch_diagnostics()?;
        // Checked here as well as in the switch owner so a rejected request never cold-builds
        // a replacement first.
        render_path_switch_admission(&diagnostics).map_err(|error| error.to_string())?;
        let presenting = diagnostics.presenting();
        let source = presenting.strategy();
        let replacement = match source {
            RASTER_STRATEGY => COMPUTE_RAY_STRATEGY,
            COMPUTE_RAY_STRATEGY => RASTER_STRATEGY,
            _ => return Err("the demo cannot switch this Render Path strategy".to_owned()),
        };
        let view = self.latest_scene_view()?;
        if view.scene_id() != presenting.scene_identity()
            || view.revision() != presenting.required_revision()
        {
            return Err(format!(
                "the current Voxel Scene View does not match the converged presenter: view={} presenter={}",
                view.revision(),
                presenting.required_revision()
            ));
        }
        let retiring_raster = if source == RASTER_STRATEGY {
            Some(
                self.lifecycle_controller
                    .as_ref()
                    .ok_or_else(|| "raster retirement diagnostics are unavailable".to_owned())?
                    .clone(),
            )
        } else {
            None
        };
        Ok(AdmittedRenderPathSwitch {
            source,
            replacement,
            view,
            retiring_raster,
        })
    }

    pub(super) fn start_render_path_switch(
        &mut self,
        admitted: AdmittedRenderPathSwitch,
        prepare_replacement: impl FnOnce(
            ReplacementRenderPath<'_>,
            &VoxelSceneView,
        ) -> Result<(), String>,
    ) -> Result<(), String> {
        let AdmittedRenderPathSwitch {
            source,
            replacement,
            view,
            retiring_raster,
        } = admitted;
        let revision = view.revision();
        let switch_requested_at = Instant::now();
        match replacement {
            COMPUTE_RAY_STRATEGY => {
                let mut replacement_path = if view.is_streamed() {
                    let frontend = self
                        .frontend
                        .as_ref()
                        .ok_or("the Voxel Frontend is unavailable")?;
                    let selection = frontend
                        .required_residency()
                        .map_err(|error| error.to_string())?
                        .ok_or("the Required Voxel Residency Selection is unavailable")?;
                    ComputeRayRenderPathAdapter::new_streamed(
                        frontend.clone(),
                        selection,
                        self.camera_state,
                        self.camera_state_revision,
                        self.render_configuration.compute_representation,
                    )
                    .map_err(|error| error.to_string())?
                } else {
                    let (path, measurement_controller) =
                        ComputeRayRenderPathAdapter::new_with_representation_and_measurement(
                            view.clone(),
                            self.camera_state,
                            self.camera_state_revision,
                            self.render_configuration.compute_representation,
                        )
                        .map_err(|error| {
                            format!("could not cold-build the compute replacement: {error}")
                        })?;
                    self.compute_measurement_controller = Some(measurement_controller);
                    path
                };
                let lifecycle_controller = replacement_path.enable_lifecycle_control();
                prepare_replacement(ReplacementRenderPath::Compute(&mut replacement_path), &view)?;
                self.backend
                    .as_mut()
                    .ok_or_else(|| "the Render Backend is unavailable".to_owned())?
                    .request_render_path_switch(Box::new(replacement_path))
                    .map_err(|error| error.to_string())?;
                // Close-time checks treat these controllers as owning an installed compute path.
                self.compute_lifecycle_controller = Some(lifecycle_controller);
            }
            RASTER_STRATEGY => {
                if view.is_streamed() {
                    let mut path = self.build_streamed_raster(&view)?;
                    let lifecycle = path.enable_lifecycle_control();
                    prepare_replacement(ReplacementRenderPath::Raster(&mut path), &view)?;
                    self.backend
                        .as_mut()
                        .ok_or("the Render Backend is unavailable")?
                        .request_render_path_switch(Box::new(path))
                        .map_err(|error| error.to_string())?;
                    self.raster_replacement_lifecycle_controller = Some(lifecycle);
                } else {
                    let (mut replacement_path, installer, _) =
                        RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
                            self.camera_state,
                            self.camera_state_revision,
                            view.scene_id().clone(),
                            revision,
                        );
                    let lifecycle_controller = replacement_path.enable_lifecycle_control();
                    prepare_replacement(
                        ReplacementRenderPath::Raster(&mut replacement_path),
                        &view,
                    )?;
                    let event_proxy = self.event_proxy.clone();
                    let mut preparation = RasterArtifactPreparation::start_regions(
                    view,
                    VoxelExtent::new(
                        self.render_configuration.raster_region_extent,
                        self.render_configuration.raster_region_extent,
                        self.render_configuration.raster_region_extent,
                    ),
                    move |event| {
                        if event_proxy
                            .send_event(DesktopEvent::Preparation(event))
                            .is_err()
                        {
                            eprintln!(
                                "desktop event loop closed before raster replacement preparation notification"
                            );
                        }
                    },
                )
                .map_err(|error| error.to_string())?;
                    if let Err(error) = self
                        .backend
                        .as_mut()
                        .ok_or_else(|| "the Render Backend is unavailable".to_owned())?
                        .request_render_path_switch(Box::new(replacement_path))
                    {
                        preparation.cancel_and_join().map_err(|cleanup_error| {
                        format!(
                            "{error}; raster replacement preparation cleanup failed: {cleanup_error}"
                        )
                    })?;
                        return Err(error.to_string());
                    }
                    self.preparation = Some(preparation);
                    self.raster_preparation_target = Some(RasterPreparationTarget::Replacement);
                    self.raster_replacement_installer = Some(installer);
                    self.raster_replacement_lifecycle_controller = Some(lifecycle_controller);
                }
            }
            _ => return Err("the demo cannot construct this Render Path strategy".to_owned()),
        }
        self.interactive_switch = Some(InteractiveRenderPathSwitch {
            source,
            replacement,
            revision,
            handoff_reported: false,
            retiring_raster,
            requested_at: switch_requested_at,
        });
        Ok(())
    }
}
