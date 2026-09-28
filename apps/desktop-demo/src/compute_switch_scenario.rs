use super::*;
use compute_ray_render_path::COMPUTE_RAY_STRATEGY;
use raster_render_path::RASTER_STRATEGY;

impl ScenarioExecution<'_> {
    pub(super) fn interactive_compute_switch_demo_active(&self) -> bool {
        self.desktop.render_configuration.compute_switch_demo
            && (!self
                .desktop
                .render_configuration
                .compute_switch_lifecycle_demo
                || self.state.compute.portable_milestone_lifecycle_complete)
    }

    pub(super) fn set_render_path_overlay(&mut self) -> Result<(), String> {
        if self.state.interactive.is_some() {
            return self.set_interactive_overlay();
        }
        let diagnostics = self
            .desktop
            .backend
            .as_ref()
            .and_then(RenderBackend::render_path_switch_diagnostics)
            .ok_or_else(|| "Render Path switching diagnostics are unavailable".to_owned())?;
        let burst_stage = self
            .state
            .compute
            .compute_edit_burst_stage
            .as_ref()
            .map(ComputeEditBurstStage::overlay_label)
            .or_else(|| {
                self.state
                    .raster
                    .edit_burst_stage
                    .as_ref()
                    .map(EditBurstStage::overlay_label)
            })
            .unwrap_or("inactive");
        let camera = self
            .state
            .camera
            .pending_camera_report
            .as_deref()
            .or(self.state.camera.last_presented_camera.as_deref())
            .map(str::to_owned)
            .unwrap_or_else(|| self.desktop.render_configuration.camera_identity());
        let report = format_render_path_overlay(
            &diagnostics,
            burst_stage,
            &camera,
            &self.state.compute.render_path_control_feedback,
        );
        if self.state.raster.last_overlay_report.as_deref() != Some(&report) {
            println!("Render Path overlay: {report}");
            self.state.raster.last_overlay_report = Some(report.clone());
        }
        self.desktop
            .text_overlay
            .as_ref()
            .ok_or_else(|| "the in-client Render Path overlay is unavailable".to_owned())?
            .set_text(&report)?;
        self.desktop.set_status(&report);
        Ok(())
    }

    pub(super) fn request_interactive_render_path_switch(&mut self) -> Result<(), String> {
        if !self.state.evidence.first_matching_frame_presented {
            return Err(
                "the initial raster path has not presented its first matching frame".to_owned(),
            );
        }
        let admitted = self.desktop.admit_render_path_switch()?;
        if admitted.source == COMPUTE_RAY_STRATEGY
            && !matches!(
                self.state.compute.compute_edit_burst_stage,
                Some(ComputeEditBurstStage::Complete)
            )
        {
            return Err("the fixed compute edit burst has not completed".to_owned());
        }
        let source = admitted.source;
        let replacement = admitted.replacement;
        let revision = admitted.view().revision();
        let mut burst_plan = None;
        if replacement == COMPUTE_RAY_STRATEGY {
            let prepare_compute_burst = self.state.compute.compute_edit_burst_stage.is_none()
                && self.state.compute.completed_interactive_switches == 0;
            if prepare_compute_burst {
                let plan = fixed_edit_burst(
                    admitted.view(),
                    self.desktop.render_configuration.raster_region_extent,
                )?;
                println!(
                    "Compute edit burst qualification: preparation_block_edge=32 hold_after_completed_blocks=1 post_upload_revision=3 expected_final_revision={}",
                    plan.expected_final_revision
                );
                burst_plan = Some(plan);
            }
            self.report_compute_timing_events()?;
        }
        let mut compute_burst_setup = None;
        let semantic_qualification = &mut self.state.evidence.semantic_qualification;
        self.desktop
            .start_render_path_switch(admitted, |replacement_path, view| {
                match replacement_path {
                    ReplacementRenderPath::Compute(path) => {
                        semantic_qualification.register_compute(path, view)?;
                        if let Some(plan) = burst_plan.take() {
                            let controller = path.enable_convergence_control_with_hold(true);
                            compute_burst_setup = Some((controller, plan));
                        }
                    }
                    ReplacementRenderPath::Raster(path) => {
                        semantic_qualification.register_raster(path, view)?;
                    }
                }
                Ok(())
            })?;

        if let Some((controller, plan)) = compute_burst_setup {
            self.desktop.compute_convergence_controller = Some(controller);
            self.state.compute.compute_edit_burst_stage =
                Some(ComputeEditBurstStage::AwaitingSpace(plan));
            self.state.compute.compute_edit_burst_events.clear();
            self.state
                .compute
                .compute_edit_burst_presented_revisions
                .clear();
        }
        self.state.compute.render_path_control_feedback =
            format!("Tab-accepted-{}", replacement.identifier());
        println!(
            "Tab switch accepted: Presenting={source:?} Replacement={replacement:?} revision={revision}"
        );
        Ok(())
    }

    pub(super) fn update_interactive_render_path_switch(&mut self) -> Result<bool, String> {
        let Some(active_switch) = self.desktop.interactive_switch.as_ref() else {
            return Ok(false);
        };
        let diagnostics = self
            .desktop
            .backend
            .as_ref()
            .and_then(RenderBackend::render_path_switch_diagnostics)
            .ok_or_else(|| "Render Path switching diagnostics are unavailable".to_owned())?;
        let roles = diagnostics.roles();
        let handoff_needs_report =
            roles.presenting() == active_switch.replacement && !active_switch.handoff_reported;

        if handoff_needs_report {
            let presenting = diagnostics.presenting();
            let retiring = diagnostics.retiring().ok_or_else(|| {
                "the first replacement frame has no explicitly owned Retiring Render Path"
                    .to_owned()
            })?;
            if retiring.strategy() != active_switch.source
                || presenting.scene_identity() != retiring.scene_identity()
                || presenting.visible_revision() != active_switch.revision
                || presenting.visible_revision() != retiring.visible_revision()
                || presenting.camera_state_revision() != retiring.camera_state_revision()
                || presenting.presentation_configuration() != retiring.presentation_configuration()
            {
                return Err(
                    "the replacement handoff did not preserve the path-neutral frame stamp"
                        .to_owned(),
                );
            }
            if active_switch.replacement == RASTER_STRATEGY {
                self.desktop.artifact_installer = Some(
                    self.desktop
                        .raster_replacement_installer
                        .take()
                        .ok_or_else(|| {
                            "the raster replacement installer is unavailable".to_owned()
                        })?,
                );
                self.desktop.lifecycle_controller = Some(
                    self.desktop
                        .raster_replacement_lifecycle_controller
                        .take()
                        .ok_or_else(|| {
                            "the raster replacement lifecycle diagnostics are unavailable"
                                .to_owned()
                        })?,
                );
            }
            let active_switch = self
                .desktop
                .interactive_switch
                .as_mut()
                .ok_or_else(|| "the active Render Path switch disappeared".to_owned())?;
            active_switch.handoff_reported = true;
            println!(
                "Render Path timing event: phase=Switching source={:?} replacement={:?} revision={} elapsed_ms={:.6}",
                active_switch.source,
                active_switch.replacement,
                presenting.visible_revision(),
                active_switch.requested_at.elapsed().as_secs_f64() * 1_000.0,
            );
            println!(
                "First replacement frame presented: Presenting={:?} Retiring={:?} revision={} CameraStateRevision={:?} PresentationConfiguration={:?}",
                active_switch.replacement,
                active_switch.source,
                presenting.visible_revision(),
                presenting.camera_state_revision(),
                presenting.presentation_configuration()
            );
        }

        let switch_in_progress = roles.replacement().is_some() || roles.retiring().is_some();
        let switch_complete = self
            .desktop
            .interactive_switch
            .as_ref()
            .is_some_and(|active_switch| active_switch.handoff_reported && !switch_in_progress);
        if switch_complete {
            let active_switch = self
                .desktop
                .interactive_switch
                .take()
                .ok_or_else(|| "the completed Render Path switch disappeared".to_owned())?;
            if let Some(controller) = active_switch.retiring_raster {
                match controller
                    .shutdown_owned_resource_count()
                    .map_err(|error| error.to_string())?
                {
                    Some(0) => {}
                    Some(count) => {
                        return Err(format!("retired raster retained {count} owned resources"));
                    }
                    None => {
                        return Err(
                            "retired raster did not report owned resource disposal".to_owned()
                        );
                    }
                }
            }
            self.state.compute.completed_interactive_switches = self
                .state
                .compute
                .completed_interactive_switches
                .checked_add(1)
                .ok_or_else(|| "the completed Render Path switch count overflowed".to_owned())?;
            if active_switch.source == COMPUTE_RAY_STRATEGY {
                self.desktop.compute_convergence_controller = None;
            }
            let feedback = if active_switch.replacement == COMPUTE_RAY_STRATEGY
                && matches!(
                    self.state.compute.compute_edit_burst_stage,
                    Some(ComputeEditBurstStage::AwaitingSpace(_))
                ) {
                "Space-ready".to_owned()
            } else {
                format!(
                    "Tab-ready-completed-{}",
                    self.state.compute.completed_interactive_switches
                )
            };
            self.set_control_feedback(feedback);
            println!(
                "Render Path retirement complete: Retired={:?} owned_resources=0 workers=0 completed_switches={}",
                active_switch.source, self.state.compute.completed_interactive_switches
            );
        }
        Ok(switch_in_progress)
    }

    pub(super) fn handle_render_path_switch_key(&mut self, event_loop: &ActiveEventLoop) {
        match self.request_interactive_render_path_switch() {
            Ok(()) => {}
            Err(error) => {
                println!("Tab switch rejected: {error}");
                self.state.compute.render_path_control_feedback = format!("Tab-rejected-{error}");
            }
        }
        if let Err(error) = self.set_render_path_overlay() {
            self.desktop.fail(event_loop, error);
            return;
        }
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
    }

    pub(super) fn request_compute_switch(&mut self) -> Result<(), String> {
        if self.state.compute.compute_switch_requested {
            return Err("the raster-to-compute switch was requested more than once".to_owned());
        }
        let view = self
            .desktop
            .frontend
            .as_ref()
            .ok_or_else(|| "the Voxel Frontend is unavailable for compute preparation".to_owned())?
            .scene_view()
            .map_err(|error| error.to_string())?;
        if Some(view.revision()) != self.desktop.published_revision {
            return Err(format!(
                "compute preparation revision {} does not match the published Voxel Scene Revision",
                view.revision()
            ));
        }
        println!(
            "Compute replacement cold build started: revision={} CameraStateRevision={:?}",
            view.revision(),
            self.desktop.camera_state_revision
        );
        let milestone_burst_plan = self.desktop.render_configuration
            .portable_milestone_demo
            .then(|| {
                let plan = fixed_edit_burst(&view, self.desktop.render_configuration.raster_region_extent)?;
                println!(
                    "Compute edit burst qualification: preparation_block_edge=32 hold_after_completed_blocks=1 post_upload_revision=3 expected_final_revision={}",
                    plan.expected_final_revision
                );
                Ok::<_, String>(plan)
            })
            .transpose()?;
        let switch_requested_at = Instant::now();
        self.report_compute_timing_events()?;
        let (mut replacement, measurement_controller) =
            ComputeRayRenderPathAdapter::new_with_measurement(
                view.clone(),
                self.desktop.camera_state,
                self.desktop.camera_state_revision,
            )
            .map_err(|error| format!("could not cold-build the compute replacement: {error}"))?;
        self.desktop.compute_measurement_controller = Some(measurement_controller);
        self.desktop.compute_lifecycle_controller = Some(replacement.enable_lifecycle_control());
        let milestone_burst = milestone_burst_plan
            .map(|plan| (replacement.enable_convergence_control_with_hold(true), plan));
        self.state
            .evidence
            .semantic_qualification
            .register_compute(&mut replacement, &view)?;
        self.desktop
            .backend
            .as_mut()
            .ok_or_else(|| {
                "the Render Backend is unavailable for the compute switch request".to_owned()
            })?
            .request_render_path_switch(Box::new(replacement))
            .map_err(|error| error.to_string())?;
        self.state.compute.compute_switch_requested_at = Some(switch_requested_at);
        self.state.compute.compute_switch_requested = true;
        if let Some((controller, plan)) = milestone_burst {
            self.desktop.compute_convergence_controller = Some(controller);
            self.state.compute.compute_edit_burst_stage =
                Some(ComputeEditBurstStage::AwaitingSpace(plan));
            self.state.compute.compute_edit_burst_events.clear();
            self.state
                .compute
                .compute_edit_burst_presented_revisions
                .clear();
        }
        println!("Compute replacement requested while raster remains Presenting");
        if self
            .desktop
            .render_configuration
            .compute_switch_lifecycle_demo
        {
            self.desktop.set_status("compute-replacement-requested");
        }
        Ok(())
    }

    pub(super) fn update_compute_switch_demo(&mut self) -> Result<bool, String> {
        if !self.state.compute.compute_switch_requested {
            return Ok(false);
        }
        let diagnostics = self
            .desktop
            .backend
            .as_ref()
            .and_then(RenderBackend::render_path_switch_diagnostics)
            .ok_or_else(|| "Render Path switching diagnostics are unavailable".to_owned())?;
        let roles = diagnostics.roles();
        if self
            .desktop
            .render_configuration
            .compute_switch_lifecycle_demo
            && self
                .desktop
                .render_path_handoff_control
                .as_ref()
                .is_some_and(RenderPathHandoffControl::is_held)
            && let Some(replacement) = diagnostics.replacement()
        {
            let held_stamp = (
                replacement.camera_state_revision(),
                replacement.presentation_configuration(),
            );
            if self.state.compute.last_held_replacement_stamp != Some(held_stamp) {
                println!(
                    "Compute replacement held: CameraStateRevision={:?} PresentationConfiguration={:?} Readiness={:?}",
                    replacement.camera_state_revision(),
                    replacement.presentation_configuration(),
                    replacement.readiness()
                );
                self.desktop.set_status(&format!(
                    "compute-replacement-held camera={:?} presentation={:?}",
                    replacement.camera_state_revision(),
                    replacement.presentation_configuration()
                ));
                self.state.compute.last_held_replacement_stamp = Some(held_stamp);
            }
        }
        if roles.presenting() == COMPUTE_RAY_STRATEGY
            && !self.state.compute.compute_first_frame_presented
        {
            let compute = diagnostics.presenting();
            let raster = diagnostics.retiring().ok_or_else(|| {
                "the first compute frame has no explicitly owned retiring raster path".to_owned()
            })?;
            if raster.strategy() != RASTER_STRATEGY
                || raster.scene_identity() != compute.scene_identity()
                || raster.visible_revision() != compute.visible_revision()
                || raster.camera_state_revision() != compute.camera_state_revision()
                || raster.presentation_configuration() != compute.presentation_configuration()
            {
                return Err(
                    "the first compute frame does not match the retiring raster frame stamp"
                        .to_owned(),
                );
            }
            self.state.compute.compute_first_frame_presented = true;
            let switching_milliseconds = self
                .state
                .compute
                .compute_switch_requested_at
                .ok_or_else(|| "the compute switch start time is unavailable".to_owned())?
                .elapsed()
                .as_secs_f64()
                * 1_000.0;
            println!(
                "Render Path timing event: phase=Switching source=Raster replacement=ComputeRay revision={} elapsed_ms={switching_milliseconds:.6}",
                compute.visible_revision(),
            );
            self.desktop.set_status("compute-presenting");
            println!(
                "First compute frame presented after atomic handoff: revision={} CameraStateRevision={:?} PresentationConfiguration={:?}",
                compute.visible_revision(),
                compute.camera_state_revision(),
                compute.presentation_configuration()
            );
        }
        let switch_in_progress = roles.replacement().is_some() || roles.retiring().is_some();
        if self.state.compute.compute_first_frame_presented
            && !switch_in_progress
            && !self.state.compute.compute_switch_complete_reported
        {
            match self
                .desktop
                .lifecycle_controller
                .as_ref()
                .ok_or_else(|| {
                    "raster lifecycle diagnostics are unavailable after retirement".to_owned()
                })?
                .shutdown_owned_resource_count()
                .map_err(|error| error.to_string())?
            {
                Some(0) => {}
                Some(count) => {
                    return Err(format!("retired raster retained {count} owned resources"));
                }
                None => {
                    return Err("retired raster did not report owned resource disposal".to_owned());
                }
            }
            self.state.compute.compute_switch_complete_reported = true;
            self.desktop.set_status("compute-switch-complete");
            println!("Raster retirement complete: owned_resources=0 workers=0");
        }
        Ok(switch_in_progress)
    }

    pub(super) fn release_compute_handoff(&mut self) -> Result<(), String> {
        if !self
            .desktop
            .render_configuration
            .compute_switch_lifecycle_demo
        {
            return Err("the compute lifecycle handoff is not enabled".to_owned());
        }
        let diagnostics = self
            .desktop
            .backend
            .as_ref()
            .and_then(RenderBackend::render_path_switch_diagnostics)
            .ok_or_else(|| "Render Path switching diagnostics are unavailable".to_owned())?;
        let replacement = diagnostics
            .replacement()
            .ok_or_else(|| "there is no held compute replacement to release".to_owned())?;
        let presenting = diagnostics.presenting();
        if replacement.strategy() != COMPUTE_RAY_STRATEGY
            || replacement.readiness() != RenderPathReadiness::Recordable
            || replacement.camera_state_revision() != self.desktop.camera_state_revision
            || replacement.camera_state_revision() != presenting.camera_state_revision()
            || replacement.presentation_configuration().is_none()
            || replacement.presentation_configuration() != presenting.presentation_configuration()
        {
            return Err(
                "the compute replacement has not acknowledged the latest Camera State and Presentation Configuration"
                    .to_owned(),
            );
        }
        let handoff_control = self
            .desktop
            .render_path_handoff_control
            .as_ref()
            .ok_or_else(|| "the Render Path handoff control is unavailable".to_owned())?;
        if !handoff_control.is_held() {
            return Err("the compute replacement handoff was already released".to_owned());
        }
        handoff_control.release();
        self.desktop.set_status("compute-handoff-released");
        println!(
            "Compute replacement handoff released: CameraStateRevision={:?} PresentationConfiguration={:?}",
            replacement.camera_state_revision(),
            replacement.presentation_configuration()
        );
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
        Ok(())
    }

    pub(super) fn request_compute_lifecycle_extent(
        &self,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let window = self.desktop.window.as_ref().ok_or_else(|| {
            "the desktop window is unavailable for lifecycle recreation".to_owned()
        })?;
        let _requested_size =
            window.request_inner_size(winit::dpi::PhysicalSize::new(width, height));
        window.request_redraw();
        Ok(())
    }

    pub(super) fn drive_compute_lifecycle_after_presented(&mut self) -> Result<(), String> {
        let Some(stage) = self.state.compute.compute_switch_lifecycle_stage else {
            return Ok(());
        };
        let diagnostics = self
            .desktop
            .backend
            .as_ref()
            .and_then(RenderBackend::render_path_switch_diagnostics)
            .ok_or_else(|| "Render Path switching diagnostics are unavailable".to_owned())?;
        let Some(replacement) = diagnostics.replacement() else {
            return Ok(());
        };
        let Some(configuration) = replacement.presentation_configuration() else {
            return Ok(());
        };
        let presentation_extent = self
            .desktop
            .backend
            .as_ref()
            .and_then(RenderBackend::presentation_extent)
            .ok_or_else(|| "the compute lifecycle presentation extent is unavailable".to_owned())?;

        match stage {
            ComputeSwitchLifecycleStage::Replacement => {
                if replacement.readiness() != RenderPathReadiness::Recordable {
                    return Ok(());
                }
                self.desktop.publish_camera_state(
                    CanonicalCameraPose::CavityMaterialCloseUp
                        .pose()
                        .map_err(|error| error.to_string())?,
                )?;
                self.state.compute.compute_switch_lifecycle_stage =
                    Some(ComputeSwitchLifecycleStage::CameraAcknowledgement);
                println!(
                    "Compute lifecycle qualification published the deterministic Camera State change"
                );
            }
            ComputeSwitchLifecycleStage::CameraAcknowledgement => {
                if replacement.camera_state_revision() != self.desktop.camera_state_revision
                    || diagnostics.presenting().camera_state_revision()
                        != self.desktop.camera_state_revision
                {
                    return Ok(());
                }
                self.request_compute_lifecycle_extent(1100, 700)?;
                self.state.compute.compute_switch_lifecycle_stage =
                    Some(ComputeSwitchLifecycleStage::Landscape {
                        previous_configuration: configuration,
                    });
            }
            ComputeSwitchLifecycleStage::Landscape {
                previous_configuration,
            } => {
                if configuration == previous_configuration
                    || presentation_extent.width <= presentation_extent.height
                {
                    return Ok(());
                }
                self.request_compute_lifecycle_extent(650, 900)?;
                self.state.compute.compute_switch_lifecycle_stage =
                    Some(ComputeSwitchLifecycleStage::Portrait {
                        previous_configuration: configuration,
                    });
            }
            ComputeSwitchLifecycleStage::Portrait {
                previous_configuration,
            } => {
                if configuration == previous_configuration
                    || presentation_extent.width >= presentation_extent.height
                {
                    return Ok(());
                }
                self.state.compute.compute_switch_lifecycle_stage =
                    Some(ComputeSwitchLifecycleStage::Suspension {
                        previous_configuration: configuration,
                    });
                self.desktop
                    .set_drawable_extent(ash::vk::Extent2D::default())?;
                self.report_drawable_extent(ash::vk::Extent2D::default());
                if let Some(window) = &self.desktop.window {
                    window.request_redraw();
                }
            }
            ComputeSwitchLifecycleStage::Restore {
                previous_configuration,
            } => {
                if configuration == previous_configuration {
                    return Ok(());
                }
                self.request_compute_lifecycle_extent(1000, 700)?;
                self.state.compute.compute_switch_lifecycle_stage =
                    Some(ComputeSwitchLifecycleStage::PresentationRecreation {
                        previous_configuration: configuration,
                    });
            }
            ComputeSwitchLifecycleStage::PresentationRecreation {
                previous_configuration,
            } => {
                if configuration == previous_configuration
                    || presentation_extent.width <= presentation_extent.height
                {
                    return Ok(());
                }
                self.release_compute_handoff()?;
                self.state.compute.compute_switch_lifecycle_stage =
                    Some(ComputeSwitchLifecycleStage::Handoff);
            }
            ComputeSwitchLifecycleStage::Suspension { .. }
            | ComputeSwitchLifecycleStage::Handoff => {}
        }
        Ok(())
    }

    pub(super) fn restore_compute_lifecycle_after_suspension(&mut self) -> Result<bool, String> {
        let Some(ComputeSwitchLifecycleStage::Suspension {
            previous_configuration,
        }) = self.state.compute.compute_switch_lifecycle_stage
        else {
            return Ok(false);
        };
        let drawable_size = self
            .desktop
            .window
            .as_ref()
            .map(Window::inner_size)
            .ok_or_else(|| "the desktop window is unavailable for lifecycle restore".to_owned())?;
        if drawable_size.width == 0 || drawable_size.height == 0 {
            return Err("the desktop window reported a zero restore extent".to_owned());
        }
        self.state.compute.compute_switch_lifecycle_stage =
            Some(ComputeSwitchLifecycleStage::Restore {
                previous_configuration,
            });
        let drawable_extent = ash::vk::Extent2D {
            width: drawable_size.width,
            height: drawable_size.height,
        };
        self.desktop.set_drawable_extent(drawable_extent)?;
        self.report_drawable_extent(drawable_extent);
        println!(
            "Compute lifecycle qualification restored presentation: {}x{}",
            drawable_size.width, drawable_size.height
        );
        Ok(true)
    }

    pub(super) fn compute_shutdown_qualification_report(&self) -> Result<String, String> {
        let qualification = self
            .desktop
            .render_configuration
            .compute_shutdown_qualification
            .ok_or_else(|| "the compute shutdown qualification is inactive".to_owned())?;
        let diagnostics = self
            .desktop
            .backend
            .as_ref()
            .and_then(RenderBackend::render_path_switch_diagnostics)
            .ok_or_else(|| {
                "Render Path switching diagnostics are unavailable during compute shutdown"
                    .to_owned()
            })?;
        if qualification == ComputeShutdownQualification::Replacement {
            let replacement = diagnostics.roles().replacement();
            let replacement_stamp = diagnostics.replacement();
            if diagnostics.roles().presenting() != RASTER_STRATEGY
                || replacement != Some(COMPUTE_RAY_STRATEGY)
                || diagnostics.roles().retiring().is_some()
                || self.state.compute.completed_interactive_switches != 0
                || replacement_stamp
                    .is_none_or(|stamp| stamp.readiness() != RenderPathReadiness::Recordable)
                || !self
                    .desktop
                    .render_path_handoff_control
                    .as_ref()
                    .is_some_and(RenderPathHandoffControl::is_held)
            {
                return Err(format!(
                    "compute replacement shutdown requires a recordable replacement held before handoff: Presenting={:?} Replacement={replacement:?} Retiring={:?} completed_switches={} replacement_readiness={:?}",
                    diagnostics.roles().presenting(),
                    diagnostics.roles().retiring(),
                    self.state.compute.completed_interactive_switches,
                    replacement_stamp.map(|stamp| stamp.readiness())
                ));
            }
            return Ok("Closing with compute replacement owned before handoff".to_owned());
        }
        if diagnostics.roles().presenting() != COMPUTE_RAY_STRATEGY
            || diagnostics.roles().replacement().is_some()
            || diagnostics.roles().retiring().is_some()
            || self.state.compute.completed_interactive_switches < 1
        {
            return Err(format!(
                "compute shutdown requires an idle ComputeRay presenter after retirement: Presenting={:?} Replacement={:?} Retiring={:?} completed_switches={}",
                diagnostics.roles().presenting(),
                diagnostics.roles().replacement(),
                diagnostics.roles().retiring(),
                self.state.compute.completed_interactive_switches
            ));
        }
        let controller = self
            .desktop
            .compute_convergence_controller
            .as_ref()
            .ok_or_else(|| "the compute convergence controller is unavailable".to_owned())?;
        let status = controller.status().map_err(|error| error.to_string())?;
        match qualification {
            ComputeShutdownQualification::Presenting => {
                if !matches!(
                    self.state.compute.compute_edit_burst_stage,
                    Some(ComputeEditBurstStage::AwaitingSpace(_))
                ) || status.worker_count() != 0
                {
                    return Err(
                        "the presenting shutdown qualification started transient compute work"
                            .to_owned(),
                    );
                }
                Ok("Closing while compute presents with switching idle".to_owned())
            }
            ComputeShutdownQualification::ActivePreparation => {
                let observation = controller
                    .preparation_barrier_observation()
                    .map_err(|error| error.to_string())?;
                if !matches!(
                    self.state.compute.compute_edit_burst_stage,
                    Some(ComputeEditBurstStage::ShutdownActivePreparationHeld(_))
                ) || status.worker_count() != 1
                    || status.preparing().map(|stamp| stamp.revision())
                        != Some(VoxelSceneRevision::new(2))
                    || !observation.is_some_and(|observation| {
                        observation.reached_revision() == Some(VoxelSceneRevision::new(2))
                            && observation.completed_block_count() == 1
                            && !observation.finished()
                    })
                {
                    return Err(
                        "the active-preparation shutdown qualification is not held at revision 2"
                            .to_owned(),
                    );
                }
                Ok("Closing with active compute preparation: revision=2 workers=1".to_owned())
            }
            ComputeShutdownQualification::HiddenCandidate => {
                if !matches!(
                    self.state.compute.compute_edit_burst_stage,
                    Some(ComputeEditBurstStage::ShutdownHiddenCandidateHeld(_))
                ) || status.worker_count() != 0
                    || status.hidden().map(|stamp| stamp.revision())
                        != Some(VoxelSceneRevision::new(2))
                    || controller
                        .post_upload_revision()
                        .map_err(|error| error.to_string())?
                        != Some(VoxelSceneRevision::new(2))
                {
                    return Err(
                        "the hidden-candidate shutdown qualification is not held at revision 2"
                            .to_owned(),
                    );
                }
                Ok("Closing with hidden uploaded compute candidate: revision=2".to_owned())
            }
            ComputeShutdownQualification::Replacement => Err(
                "the compute replacement shutdown qualification was not handled before convergence checks"
                    .to_owned(),
            ),
        }
    }
}
