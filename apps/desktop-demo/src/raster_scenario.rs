use super::*;

impl ScenarioExecution<'_> {
    pub(super) fn set_convergence_overlay(
        &mut self,
        status: RasterConvergenceStatus,
    ) -> Result<(), String> {
        let stage = self
            .state
            .raster
            .edit_burst_stage
            .as_ref()
            .map(EditBurstStage::overlay_label)
            .unwrap_or("inactive");
        let camera = self
            .state
            .camera
            .last_presented_camera
            .as_deref()
            .unwrap_or("initial");
        let report = format_convergence_overlay(stage, status, camera);
        if self.state.raster.last_overlay_report.as_deref() != Some(&report) {
            println!("Edit burst overlay: {report}");
            self.state.raster.last_overlay_report = Some(report.clone());
        }
        self.desktop
            .text_overlay
            .as_ref()
            .ok_or_else(|| "the in-client convergence overlay is unavailable".to_owned())?
            .set_text(&report)?;
        self.desktop.set_status(&report);
        Ok(())
    }

    pub(super) fn publish_next_burst_command(
        &self,
        plan: &mut EditBurstPlan,
    ) -> Result<(), String> {
        let command = plan.take_next_owned_command()?;
        let outcome = self
            .desktop
            .frontend
            .as_ref()
            .ok_or_else(|| "the edit burst Voxel Frontend is unavailable".to_owned())?
            .edit(command)
            .map_err(|error| error.to_string())?;
        let revision = match &outcome {
            VoxelEditOutcome::Changed { view, .. } => view.revision(),
            VoxelEditOutcome::Unchanged(_) => {
                return Err("a fixed edit burst command did not change its Voxel Value".to_owned());
            }
        };
        self.desktop
            .lifecycle_controller
            .as_ref()
            .ok_or_else(|| "the edit burst lifecycle controller is unavailable".to_owned())?
            .submit(outcome)
            .map_err(|error| error.to_string())?;
        println!("Edit burst command published: revision={revision}");
        Ok(())
    }

    pub(super) fn start_edit_burst(&mut self, event_loop: &ActiveEventLoop) {
        let Some(EditBurstStage::AwaitingKey(mut plan)) = self.state.raster.edit_burst_stage.take()
        else {
            self.desktop.fail(
                event_loop,
                "the fixed edit burst is not awaiting its keypress",
            );
            return;
        };
        if let Err(error) = plan.claim_space_keypress() {
            self.desktop.fail(event_loop, error);
            return;
        }
        if let Err(error) = self
            .desktop
            .lifecycle_controller
            .as_ref()
            .ok_or_else(|| "the edit burst lifecycle controller is unavailable".to_owned())
            .and_then(|controller| {
                controller
                    .begin_characterization()
                    .map_err(|error| error.to_string())
            })
        {
            self.desktop.fail(event_loop, error);
            return;
        }
        self.state.raster.edit_burst_started_at = Some(Instant::now());
        if let Err(error) = self.publish_next_burst_command(&mut plan) {
            self.desktop.fail(event_loop, error);
            return;
        }
        println!("Edit burst started by one keypress");
        self.state.raster.edit_burst_stage = Some(EditBurstStage::WaitingForCpuBarrier(plan));
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
    }

    pub(super) fn advance_edit_burst(&mut self, event_loop: &ActiveEventLoop) {
        let Some(stage) = self.state.raster.edit_burst_stage.take() else {
            return;
        };
        let Some(controller) = self.desktop.lifecycle_controller.clone() else {
            self.state.raster.edit_burst_stage = Some(stage);
            return;
        };
        let next_stage = match stage {
            EditBurstStage::AwaitingKey(plan) => EditBurstStage::AwaitingKey(plan),
            EditBurstStage::WaitingForCpuBarrier(mut plan) => {
                let observation = match controller.cpu_barrier_observation() {
                    Ok(observation) => observation,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                if observation.is_some_and(|observation| observation.reached_revision.is_some()) {
                    println!("Edit burst CPU barrier reached: scheduled_regions=1");
                    if let Err(error) = self.publish_next_burst_command(&mut plan) {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                    EditBurstStage::WaitingForSecondRequirement(plan)
                } else {
                    EditBurstStage::WaitingForCpuBarrier(plan)
                }
            }
            EditBurstStage::WaitingForSecondRequirement(plan) => {
                let status = match controller.convergence_status() {
                    Ok(status) => status,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                if status.is_some_and(|status| {
                    status.required_revision.checked_successor()
                        == Some(plan.expected_final_revision)
                }) {
                    println!("Edit burst CPU barrier held with newer requirement installed");
                    EditBurstStage::CpuBarrierHeld(plan)
                } else {
                    EditBurstStage::WaitingForSecondRequirement(plan)
                }
            }
            EditBurstStage::CpuBarrierHeld(plan) => EditBurstStage::CpuBarrierHeld(plan),
            EditBurstStage::WaitingForCpuCancellation(plan) => {
                let observation = match controller.cpu_barrier_observation() {
                    Ok(observation) => observation,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                if observation.is_some_and(|observation| {
                    observation.finished
                        && observation.cancelled
                        && observation.scheduled_region_count == 1
                }) {
                    println!(
                        "Obsolete CPU generation cancelled: scheduled_regions_before_hold=1 scheduled_regions_total=1"
                    );
                    EditBurstStage::WaitingForPostUploadCandidate(plan)
                } else {
                    EditBurstStage::WaitingForCpuCancellation(plan)
                }
            }
            EditBurstStage::WaitingForPostUploadCandidate(plan) => {
                let revision = match controller.post_upload_revision() {
                    Ok(revision) => revision,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                if revision.is_some_and(|revision| {
                    revision.checked_successor() == Some(plan.expected_final_revision)
                }) {
                    println!("Superseded candidate held after upload: revision={revision:?}");
                    EditBurstStage::PostUploadCandidateHeld(plan)
                } else {
                    EditBurstStage::WaitingForPostUploadCandidate(plan)
                }
            }
            EditBurstStage::PostUploadCandidateHeld(plan) => {
                EditBurstStage::PostUploadCandidateHeld(plan)
            }
            EditBurstStage::WaitingForPostUploadCandidateAfterLifecycle(mut plan) => {
                let revision = match controller.post_upload_revision() {
                    Ok(revision) => revision,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                if revision.is_some_and(|revision| {
                    revision.checked_successor() == Some(plan.expected_final_revision)
                }) {
                    println!(
                        "Post-upload candidate restored after lifecycle: revision={revision:?}"
                    );
                    if let Err(error) = self.publish_next_burst_command(&mut plan) {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                    EditBurstStage::WaitingForFinalRequirement(plan)
                } else {
                    EditBurstStage::WaitingForPostUploadCandidateAfterLifecycle(plan)
                }
            }
            EditBurstStage::WaitingForFinalRequirement(plan) => {
                let status = match controller.convergence_status() {
                    Ok(status) => status,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                if status
                    .is_some_and(|status| status.required_revision == plan.expected_final_revision)
                {
                    println!("Post-upload barrier held with newest requirement installed");
                    EditBurstStage::PostUploadBarrierHeld(plan)
                } else {
                    EditBurstStage::WaitingForFinalRequirement(plan)
                }
            }
            EditBurstStage::PostUploadBarrierHeld(plan) => {
                EditBurstStage::PostUploadBarrierHeld(plan)
            }
            EditBurstStage::WaitingForCandidateRejection(plan) => {
                let rejection = match controller.rejected_candidate() {
                    Ok(rejection) => rejection,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                if let Some(rejection) = rejection {
                    if rejection.revision.checked_successor() != Some(plan.expected_final_revision)
                    {
                        self.desktop.fail(
                            event_loop,
                            format!(
                                "unexpected rejected candidate revision {}; expected the predecessor of {}",
                                rejection.revision, plan.expected_final_revision
                            ),
                        );
                        return;
                    }
                    println!(
                        "Superseded candidate rejected at commit: revision={} retired_resources={}",
                        rejection.revision, rejection.retired_resource_count
                    );
                    EditBurstStage::WaitingForFinalVisibility(plan)
                } else {
                    EditBurstStage::WaitingForCandidateRejection(plan)
                }
            }
            EditBurstStage::WaitingForFinalVisibility(plan) => {
                let status = match controller.convergence_status() {
                    Ok(status) => status,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                if status.is_some_and(|status| {
                    status.required_revision == plan.expected_final_revision
                        && status.visible_revision == plan.expected_final_revision
                }) {
                    let latency_milliseconds = match self.state.raster.edit_burst_started_at.take()
                    {
                        Some(started_at) => started_at.elapsed().as_secs_f64() * 1_000.0,
                        None => {
                            self.desktop
                                .fail(event_loop, "the edit burst keypress timestamp is missing");
                            return;
                        }
                    };
                    let resource_peak = match controller.gpu_resource_peak() {
                        Ok(resource_peak) => resource_peak,
                        Err(error) => {
                            self.desktop.fail(event_loop, error);
                            return;
                        }
                    };
                    let characterization = match controller.characterization() {
                        Ok(Some(characterization)) => characterization,
                        Ok(None) => {
                            self.desktop.fail(
                                event_loop,
                                "Raster Convergence characterization did not start",
                            );
                            return;
                        }
                        Err(error) => {
                            self.desktop.fail(event_loop, error);
                            return;
                        }
                    };
                    println!(
                        "Edit burst converged atomically: visible_revision={} expected_final_revision={}",
                        plan.expected_final_revision, plan.expected_final_revision
                    );
                    println!(
                        "Edit burst final-visible measurement: elapsed_ms={latency_milliseconds:.6} peak_live_gpu_bytes={} peak_live_gpu_resources={}",
                        resource_peak.bytes, resource_peak.resources
                    );
                    println!(
                        "Raster Convergence characterization: {}",
                        format_convergence_characterization(&characterization)
                    );
                    EditBurstStage::Complete
                } else {
                    EditBurstStage::WaitingForFinalVisibility(plan)
                }
            }
            EditBurstStage::Complete => EditBurstStage::Complete,
        };
        self.state.raster.edit_burst_stage = Some(next_stage);
    }

    pub(super) fn release_edit_cpu_barrier(&mut self, event_loop: &ActiveEventLoop) {
        let Some(EditBurstStage::CpuBarrierHeld(plan)) = self.state.raster.edit_burst_stage.take()
        else {
            self.desktop
                .fail(event_loop, "the edit burst CPU barrier is not held");
            return;
        };
        let result = self
            .desktop
            .lifecycle_controller
            .as_ref()
            .ok_or_else(|| "the edit burst lifecycle controller is unavailable".to_owned())
            .and_then(|controller| {
                controller
                    .release_cpu_barrier()
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            self.desktop.fail(event_loop, error);
            return;
        }
        println!("Edit burst CPU barrier released after newer requirement");
        self.state.raster.edit_burst_stage = Some(EditBurstStage::WaitingForCpuCancellation(plan));
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
    }

    pub(super) fn release_edit_post_upload_barrier(&mut self, event_loop: &ActiveEventLoop) {
        let Some(EditBurstStage::PostUploadBarrierHeld(plan)) =
            self.state.raster.edit_burst_stage.take()
        else {
            self.desktop
                .fail(event_loop, "the edit burst post-upload barrier is not held");
            return;
        };
        let result = self
            .desktop
            .lifecycle_controller
            .as_ref()
            .ok_or_else(|| "the edit burst lifecycle controller is unavailable".to_owned())
            .and_then(|controller| {
                controller
                    .release_post_upload()
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            self.desktop.fail(event_loop, error);
            return;
        }
        println!("Post-upload barrier released for newest requirement");
        self.state.raster.edit_burst_stage =
            Some(EditBurstStage::WaitingForCandidateRejection(plan));
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
    }

    pub(super) fn release_edit_post_upload_lifecycle_barrier(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) {
        let Some(EditBurstStage::PostUploadCandidateHeld(plan)) =
            self.state.raster.edit_burst_stage.take()
        else {
            self.desktop.fail(
                event_loop,
                "the edit burst post-upload lifecycle barrier is not held",
            );
            return;
        };
        println!("Post-upload lifecycle barrier released; waiting for restored candidate");
        self.state.raster.edit_burst_stage = Some(
            EditBurstStage::WaitingForPostUploadCandidateAfterLifecycle(plan),
        );
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
    }

    pub(super) fn complete_preparation(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut preparation) = self.desktop.preparation.take() else {
            self.desktop.fail(
                event_loop,
                "background preparation completed more than once",
            );
            return;
        };
        let Some(preparation_target) = self.desktop.raster_preparation_target.take() else {
            self.desktop
                .fail(event_loop, "the raster preparation target is unavailable");
            return;
        };
        let artifact = match preparation.try_complete() {
            Ok(Some(artifact)) => artifact,
            Ok(None) => {
                self.desktop.preparation = Some(preparation);
                self.desktop.fail(
                    event_loop,
                    "background preparation signaled completion without an artifact",
                );
                return;
            }
            Err(error) => {
                self.desktop.fail(event_loop, error);
                return;
            }
        };
        self.state.raster.raster_region_count = artifact.regions().len();
        if let Some(measurement) = &mut self.state.evidence.measurement {
            let derived_at = Instant::now();
            let source_revision = match measurement_revision(artifact.source_revision()) {
                Ok(source_revision) => source_revision,
                Err(error) => {
                    self.desktop.fail(event_loop, error);
                    return;
                }
            };
            let resource_counts = (|| {
                let exposed_quads = u64::try_from(artifact.semantic_face_count())
                    .map_err(|_| "exposed quad count cannot be represented".to_owned())?;
                let vertices = u64::try_from(artifact.vertex_count())
                    .map_err(|_| "vertex count cannot be represented".to_owned())?;
                let indices = u64::try_from(artifact.index_count())
                    .map_err(|_| "index count cannot be represented".to_owned())?;
                let vertex_bytes = u64::try_from(artifact.vertex_byte_size())
                    .map_err(|_| "vertex byte count cannot be represented".to_owned())?;
                let index_bytes = u64::try_from(artifact.index_byte_size())
                    .map_err(|_| "index byte count cannot be represented".to_owned())?;
                let geometry_bytes = vertex_bytes
                    .checked_add(index_bytes)
                    .ok_or_else(|| "raster artifact byte count overflowed".to_owned())?;
                Ok::<_, String>(ResourceCounts {
                    occupied_voxels: self.state.evidence.occupied_voxels,
                    exposed_quads,
                    vertices,
                    indices,
                    draw_calls: u64::from(indices > 0),
                    cpu_artifact_bytes: geometry_bytes,
                    gpu_buffer_bytes: geometry_bytes,
                })
            })();
            let elapsed_milliseconds = measurement.elapsed_milliseconds(derived_at);
            match (resource_counts, elapsed_milliseconds) {
                (Ok(resources), Ok(elapsed_milliseconds)) => {
                    if let Err(error) = measurement.record(MeasurementEvent::ArtifactDerived {
                        source_revision,
                        elapsed_milliseconds,
                        resources,
                    }) {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                    measurement.derivation_at = Some(derived_at);
                }
                (Err(error), _) | (_, Err(error)) => {
                    self.desktop.fail(event_loop, error);
                    return;
                }
            }
        }
        let installer = match preparation_target {
            RasterPreparationTarget::Initial => self.desktop.artifact_installer.as_ref(),
            RasterPreparationTarget::Replacement => {
                self.desktop.raster_replacement_installer.as_ref()
            }
        };
        let Some(installer) = installer else {
            self.desktop
                .fail(event_loop, "the raster artifact installer is unavailable");
            return;
        };
        if let Err(error) = installer.publish_complete(artifact) {
            self.desktop.fail(event_loop, error);
            return;
        }
        let Some(backend) = &mut self.desktop.backend else {
            self.desktop.fail(
                event_loop,
                "the Render Backend is unavailable for artifact installation",
            );
            return;
        };
        if let Err(error) = backend.refresh_render_path() {
            let installed_revision = installer
                .installed_source_revision()
                .map(|revision| format!("{revision:?}"))
                .unwrap_or_else(|status_error| format!("unavailable ({status_error})"));
            self.desktop.fail(
                event_loop,
                format!(
                    "{error}; installed raster artifact revision after failure: {installed_revision}"
                ),
            );
            return;
        }
        let installed_revision = match installer.installed_source_revision() {
            Ok(Some(revision)) => revision,
            Ok(None) => {
                self.desktop.fail(
                    event_loop,
                    "the Render Path refresh did not install a complete raster artifact",
                );
                return;
            }
            Err(error) => {
                self.desktop.fail(event_loop, error);
                return;
            }
        };
        let expected_revision = match preparation_target {
            RasterPreparationTarget::Initial => self.desktop.published_revision,
            RasterPreparationTarget::Replacement => self
                .desktop
                .interactive_switch
                .as_ref()
                .map(|active_switch| active_switch.revision),
        };
        if Some(installed_revision) != expected_revision {
            self.desktop.fail(
                event_loop,
                format!(
                    "installed raster artifact revision {installed_revision} does not match the requested Voxel Scene Revision"
                ),
            );
            return;
        }
        if let Some(measurement) = &mut self.state.evidence.measurement {
            let installation_at = Instant::now();
            let elapsed_milliseconds = match measurement.elapsed_milliseconds(installation_at) {
                Ok(elapsed_milliseconds) => elapsed_milliseconds,
                Err(error) => {
                    self.desktop.fail(event_loop, error);
                    return;
                }
            };
            if let Err(error) = measurement.record(MeasurementEvent::ArtifactInstalled {
                source_revision: match measurement_revision(installed_revision) {
                    Ok(source_revision) => source_revision,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                },
                elapsed_milliseconds,
            }) {
                self.desktop.fail(event_loop, error);
                return;
            }
            measurement.installation_at = Some(installation_at);
        }
        println!("Raster artifact installed: revision={installed_revision} count=1");
        if preparation_target == RasterPreparationTarget::Replacement {
            self.state.compute.render_path_control_feedback =
                format!("Raster-replacement-ready-{installed_revision}");
            if let Err(error) = self.set_render_path_overlay() {
                self.desktop.fail(event_loop, error);
                return;
            }
            if let Some(window) = &self.desktop.window {
                window.request_redraw();
            }
            return;
        }
        if self.desktop.render_configuration.edit_burst_demo {
            let plan = match self
                .desktop
                .frontend
                .as_ref()
                .ok_or_else(|| "the edit burst Voxel Frontend is unavailable".to_owned())
                .and_then(|frontend| frontend.scene_view().map_err(|error| error.to_string()))
                .and_then(|view| {
                    fixed_edit_burst(
                        &view,
                        self.desktop.render_configuration.raster_region_extent,
                    )
                }) {
                Ok(plan) => plan,
                Err(error) => {
                    self.desktop.fail(event_loop, error);
                    return;
                }
            };
            self.state.raster.edit_burst_stage = Some(EditBurstStage::AwaitingKey(plan));
            if let Err(error) = self.set_convergence_overlay(RasterConvergenceStatus {
                required_revision: installed_revision,
                visible_revision: installed_revision,
                affected_region_count: 0,
                unaffected_region_count: self.state.raster.raster_region_count,
            }) {
                self.desktop.fail(event_loop, error);
                return;
            }
            println!("Edit burst ready: press Space");
        }
        if let Some(command) = self.state.raster.lifecycle_edit.take() {
            let outcome = match self.desktop.frontend.as_ref() {
                Some(frontend) => match frontend.edit(command) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                },
                None => {
                    self.desktop
                        .fail(event_loop, "the lifecycle Voxel Frontend is unavailable");
                    return;
                }
            };
            if let Some(controller) = &self.desktop.lifecycle_controller
                && let Err(error) = controller.submit(outcome)
            {
                self.desktop.fail(event_loop, error);
                return;
            }
        }
        if !self.desktop.render_configuration.edit_burst_demo {
            self.desktop
                .set_status(&format!("artifact-ready revision {installed_revision}"));
        }
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
    }

    pub(super) fn prepare_scene_qualification(
        &mut self,
        event_loop: &ActiveEventLoop,
        view: &VoxelSceneView,
    ) {
        if self.desktop.render_configuration.hold_post_upload_candidate {
            let volume_identity = VoxelVolumeId::new("canonical-volume");
            let coordinate = VoxelCoordinate::new(0, 0, 0);
            let sample = match view.read_region(
                &volume_identity,
                VoxelRegion::new(coordinate, VoxelExtent::new(1, 1, 1)),
            ) {
                Ok(samples) => match samples.first() {
                    Some(sample) => sample.value().clone(),
                    None => {
                        self.desktop.application_error = Some(
                            "the post-upload lifecycle edit coordinate had no Voxel Sample"
                                .to_owned(),
                        );
                        event_loop.exit();
                        return;
                    }
                },
                Err(error) => {
                    self.desktop.application_error = Some(error.to_string());
                    event_loop.exit();
                    return;
                }
            };
            let requested = match sample {
                VoxelValue::Empty => VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm")),
                VoxelValue::Occupied(_) => VoxelValue::Empty,
            };
            self.state.raster.lifecycle_edit = Some(VoxelEditCommand::new(
                volume_identity,
                coordinate,
                requested,
            ));
        }
    }

    pub(super) fn configure_raster_qualification(
        &mut self,
        event_loop: &ActiveEventLoop,
        render_path: &mut RasterRenderPathAdapter,
        artifact_installer: &RasterArtifactInstaller,
        view: &VoxelSceneView,
    ) {
        if self.desktop.render_configuration.hold_post_upload_candidate
            || self
                .desktop
                .render_configuration
                .hold_background_preparation
            || self.desktop.render_configuration.edit_burst_demo
            || self.desktop.render_configuration.compute_switch_demo
        {
            self.desktop.lifecycle_controller = Some(render_path.enable_lifecycle_control(
                self.desktop.render_configuration.hold_post_upload_candidate
                    || self.desktop.render_configuration.edit_burst_demo,
            ));
        }
        if self.desktop.render_configuration.edit_burst_demo
            && let Some(controller) = &self.desktop.lifecycle_controller
            && let Err(error) = controller.hold_next_cpu_generation_after_regions(1)
        {
            self.desktop.application_error = Some(error.to_string());
            event_loop.exit();
            return;
        }
        if self.desktop.render_configuration.compute_switch_demo
            && let Err(error) = self
                .state
                .evidence
                .semantic_qualification
                .register_raster(render_path, view)
        {
            self.desktop.application_error = Some(error);
            event_loop.exit();
            return;
        }
        if self
            .desktop
            .render_configuration
            .inject_raster_upload_failure
            && let Err(error) = artifact_installer.inject_next_upload_failure()
        {
            self.desktop.application_error = Some(error.to_string());
            event_loop.exit();
        }
    }
}
