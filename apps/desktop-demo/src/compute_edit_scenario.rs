use super::*;
use compute_ray_render_path::COMPUTE_RAY_STRATEGY;

impl ScenarioExecution<'_> {
    pub(super) fn submit_compute_edit_burst_command(
        &mut self,
        plan: &mut EditBurstPlan,
        expected_revision: VoxelSceneRevision,
    ) -> Result<(), String> {
        let command = plan.take_next_owned_command()?;
        let outcome = self
            .desktop
            .frontend
            .as_ref()
            .ok_or_else(|| "the compute edit-burst Voxel Frontend is unavailable".to_owned())?
            .edit(command)
            .map_err(|error| error.to_string())?;
        let revision = match &outcome {
            VoxelEditOutcome::Changed { view, .. } => view.revision(),
            VoxelEditOutcome::Unchanged(_) => {
                return Err("the fixed compute edit-burst command changed no voxel".to_owned());
            }
        };
        if revision != expected_revision {
            return Err(format!(
                "the compute edit burst produced revision {revision}; expected {expected_revision}"
            ));
        }
        self.desktop
            .backend
            .as_mut()
            .ok_or_else(|| "the Render Backend is unavailable".to_owned())?
            .submit_edit_outcome(outcome)
            .map_err(|error| error.to_string())?;
        self.desktop.published_revision = Some(revision);
        println!("Compute edit requirement submitted: Required={revision}");
        Ok(())
    }

    pub(super) fn start_compute_edit_burst(&mut self) -> Result<(), String> {
        let diagnostics = self
            .desktop
            .backend
            .as_ref()
            .and_then(RenderBackend::render_path_switch_diagnostics)
            .ok_or_else(|| "Render Path switching diagnostics are unavailable".to_owned())?;
        compute_edit_burst_admission(
            &diagnostics,
            matches!(
                self.state.compute.compute_edit_burst_stage,
                Some(ComputeEditBurstStage::AwaitingSpace(_))
            ),
        )?;
        let Some(ComputeEditBurstStage::AwaitingSpace(mut plan)) =
            self.state.compute.compute_edit_burst_stage.take()
        else {
            return Err("the fixed compute edit burst is not awaiting Space".to_owned());
        };
        plan.claim_space_keypress()?;
        self.desktop
            .compute_convergence_controller
            .as_ref()
            .ok_or_else(|| "the compute convergence controller is unavailable".to_owned())?
            .hold_next_preparation_after_blocks(1)
            .map_err(|error| error.to_string())?;
        self.state.raster.edit_burst_started_at = Some(Instant::now());
        self.submit_compute_edit_burst_command(&mut plan, VoxelSceneRevision::new(2))?;
        self.state.compute.compute_edit_burst_stage = Some(
            ComputeEditBurstStage::WaitingForRevisionTwoRequirement(plan),
        );
        self.state.compute.render_path_control_feedback = "Space-accepted".to_owned();
        println!("Space edit burst accepted while compute presents and switching is idle");
        Ok(())
    }

    pub(super) fn handle_compute_edit_burst_key(&mut self, event_loop: &ActiveEventLoop) {
        match self.start_compute_edit_burst() {
            Ok(()) => {}
            Err(error) => {
                println!("Space edit burst rejected: {error}");
                self.state.compute.render_path_control_feedback = format!("Space-rejected-{error}");
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

    pub(super) fn update_compute_edit_burst(&mut self) -> Result<bool, String> {
        let Some(stage) = self.state.compute.compute_edit_burst_stage.take() else {
            return Ok(false);
        };
        let Some(controller) = self.desktop.compute_convergence_controller.clone() else {
            self.state.compute.compute_edit_burst_stage = Some(stage);
            return Ok(false);
        };
        let events = controller
            .drain_events()
            .map_err(|error| error.to_string())?;
        for event in &events {
            println!("Compute convergence event: {event:?}");
        }
        self.state.compute.compute_edit_burst_events.extend(events);
        let status = controller.status().map_err(|error| error.to_string())?;
        if let Some(diagnostics) = self
            .desktop
            .backend
            .as_ref()
            .and_then(RenderBackend::render_path_switch_diagnostics)
            && diagnostics.roles().presenting() == COMPUTE_RAY_STRATEGY
        {
            self.state
                .compute
                .compute_edit_burst_presented_revisions
                .push(diagnostics.presenting().visible_revision());
        }

        let discarded = |revision, disposition| {
            self.state
                .compute
                .compute_edit_burst_events
                .iter()
                .any(|event| {
                    matches!(
                        event,
                        ComputeConvergenceEvent::CandidateDiscarded {
                            stamp,
                            disposition: actual,
                        } if stamp.revision() == revision && *actual == disposition
                    )
                })
        };
        let uploaded = |revision| {
            self.state
                .compute
                .compute_edit_burst_events
                .iter()
                .any(|event| {
                    matches!(
                        event,
                        ComputeConvergenceEvent::CandidateUploaded { stamp }
                            if stamp.revision() == revision
                    )
                })
        };
        let installed = |revision| {
            self.state
                .compute
                .compute_edit_burst_events
                .iter()
                .any(|event| {
                    matches!(
                        event,
                        ComputeConvergenceEvent::CandidateInstalled { stamp }
                            if stamp.revision() == revision
                    )
                })
        };

        let next_stage = match stage {
            ComputeEditBurstStage::AwaitingSpace(plan) => {
                ComputeEditBurstStage::AwaitingSpace(plan)
            }
            ComputeEditBurstStage::WaitingForRevisionTwoRequirement(plan) => {
                if status.required_revision() == VoxelSceneRevision::new(2) {
                    ComputeEditBurstStage::WaitingForPreparationBarrier(plan)
                } else {
                    ComputeEditBurstStage::WaitingForRevisionTwoRequirement(plan)
                }
            }
            ComputeEditBurstStage::WaitingForPreparationBarrier(mut plan) => {
                let observation = controller
                    .preparation_barrier_observation()
                    .map_err(|error| error.to_string())?;
                if observation.is_some_and(|observation| {
                    observation.reached_revision() == Some(VoxelSceneRevision::new(2))
                        && observation.completed_block_count() == 1
                }) {
                    println!(
                        "Compute revision 2 held after one bounded 32-cubed preparation block"
                    );
                    match self
                        .desktop
                        .render_configuration
                        .compute_shutdown_qualification
                    {
                        Some(ComputeShutdownQualification::ActivePreparation) => {
                            self.state.compute.render_path_control_feedback =
                                "shutdown-active-preparation-ready".to_owned();
                            ComputeEditBurstStage::ShutdownActivePreparationHeld(plan)
                        }
                        Some(ComputeShutdownQualification::HiddenCandidate) => {
                            controller
                                .release_preparation_barrier()
                                .map_err(|error| error.to_string())?;
                            ComputeEditBurstStage::WaitingForShutdownHiddenCandidate(plan)
                        }
                        Some(
                            ComputeShutdownQualification::Presenting
                            | ComputeShutdownQualification::Replacement,
                        )
                        | None => {
                            self.submit_compute_edit_burst_command(
                                &mut plan,
                                VoxelSceneRevision::new(3),
                            )?;
                            ComputeEditBurstStage::WaitingForRevisionThreeRequirement(plan)
                        }
                    }
                } else {
                    ComputeEditBurstStage::WaitingForPreparationBarrier(plan)
                }
            }
            ComputeEditBurstStage::WaitingForRevisionThreeRequirement(plan) => {
                if status.required_revision() == VoxelSceneRevision::new(3) {
                    controller
                        .release_preparation_barrier()
                        .map_err(|error| error.to_string())?;
                    println!("Compute revision 2 preparation barrier released after Required=3");
                    ComputeEditBurstStage::WaitingForRevisionTwoCancellation(plan)
                } else {
                    ComputeEditBurstStage::WaitingForRevisionThreeRequirement(plan)
                }
            }
            ComputeEditBurstStage::WaitingForRevisionTwoCancellation(plan) => {
                let observation = controller
                    .preparation_barrier_observation()
                    .map_err(|error| error.to_string())?;
                if discarded(
                    VoxelSceneRevision::new(2),
                    ComputeCandidateDisposition::SupersededBeforeUpload,
                ) && observation.is_some_and(|observation| {
                    observation.completed_block_count() == 1
                        && observation.finished()
                        && observation.cancelled()
                }) {
                    println!("Compute revision 2 cancelled after exactly one preparation block");
                    ComputeEditBurstStage::WaitingForRevisionThreeUpload(plan)
                } else {
                    ComputeEditBurstStage::WaitingForRevisionTwoCancellation(plan)
                }
            }
            ComputeEditBurstStage::WaitingForRevisionThreeUpload(mut plan) => {
                if controller
                    .post_upload_revision()
                    .map_err(|error| error.to_string())?
                    == Some(VoxelSceneRevision::new(3))
                    && uploaded(VoxelSceneRevision::new(3))
                {
                    println!("Compute revision 3 uploaded and retained hidden");
                    let expected_final_revision = plan.expected_final_revision;
                    self.submit_compute_edit_burst_command(&mut plan, expected_final_revision)?;
                    ComputeEditBurstStage::WaitingForRevisionFourRequirement(plan)
                } else {
                    ComputeEditBurstStage::WaitingForRevisionThreeUpload(plan)
                }
            }
            ComputeEditBurstStage::WaitingForRevisionFourRequirement(plan) => {
                if status.required_revision() == plan.expected_final_revision {
                    ComputeEditBurstStage::WaitingForRevisionThreeRejection(plan)
                } else {
                    ComputeEditBurstStage::WaitingForRevisionFourRequirement(plan)
                }
            }
            ComputeEditBurstStage::WaitingForRevisionThreeRejection(plan) => {
                if discarded(
                    VoxelSceneRevision::new(3),
                    ComputeCandidateDisposition::SupersededAfterUpload,
                ) {
                    controller
                        .release_post_upload()
                        .map_err(|error| error.to_string())?;
                    println!("Compute revision 3 rejected after upload with Required=4");
                    ComputeEditBurstStage::WaitingForFinalVisibility(plan)
                } else {
                    ComputeEditBurstStage::WaitingForRevisionThreeRejection(plan)
                }
            }
            ComputeEditBurstStage::WaitingForFinalVisibility(plan) => {
                if status.required_revision() == plan.expected_final_revision
                    && status.visible_revision() == plan.expected_final_revision
                    && installed(plan.expected_final_revision)
                {
                    let installed_revisions = self
                        .state
                        .compute
                        .compute_edit_burst_events
                        .iter()
                        .filter_map(|event| match event {
                            ComputeConvergenceEvent::CandidateInstalled { stamp } => {
                                Some(stamp.revision())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if installed_revisions != vec![plan.expected_final_revision] {
                        return Err(format!(
                            "compute edit burst installed unexpected revisions {installed_revisions:?}"
                        ));
                    }
                    if uploaded(VoxelSceneRevision::new(2))
                        || installed(VoxelSceneRevision::new(2))
                        || installed(VoxelSceneRevision::new(3))
                        || self
                            .state
                            .compute
                            .compute_edit_burst_presented_revisions
                            .iter()
                            .any(|revision| {
                                *revision == VoxelSceneRevision::new(2)
                                    || *revision == VoxelSceneRevision::new(3)
                            })
                    {
                        return Err(
                            "obsolete compute revisions reached upload, installation, or presentation"
                                .to_owned(),
                        );
                    }
                    let elapsed_milliseconds = self
                        .state
                        .raster
                        .edit_burst_started_at
                        .take()
                        .ok_or_else(|| "the compute edit-burst timestamp is missing".to_owned())?
                        .elapsed()
                        .as_secs_f64()
                        * 1_000.0;
                    println!(
                        "Compute edit burst converged newest-only: Required={} Visible={} installed_revisions={installed_revisions:?} obsolete_presented_frames=0 obsolete_semantic_observations=0 elapsed_ms={elapsed_milliseconds:.6}",
                        plan.expected_final_revision, plan.expected_final_revision
                    );
                    let view = self
                        .desktop
                        .frontend
                        .as_ref()
                        .ok_or_else(|| "the Voxel Frontend is unavailable".to_owned())?
                        .scene_view()
                        .map_err(|error| error.to_string())?;
                    self.state
                        .evidence
                        .semantic_qualification
                        .request_active_compute(&view)?;
                    if let Some(window) = &self.desktop.window {
                        window.request_redraw();
                    }
                    self.state.compute.render_path_control_feedback =
                        "Space-complete-Tab-ready".to_owned();
                    ComputeEditBurstStage::Complete
                } else {
                    ComputeEditBurstStage::WaitingForFinalVisibility(plan)
                }
            }
            ComputeEditBurstStage::ShutdownActivePreparationHeld(plan) => {
                ComputeEditBurstStage::ShutdownActivePreparationHeld(plan)
            }
            ComputeEditBurstStage::WaitingForShutdownHiddenCandidate(plan) => {
                if controller
                    .post_upload_revision()
                    .map_err(|error| error.to_string())?
                    == Some(VoxelSceneRevision::new(2))
                    && uploaded(VoxelSceneRevision::new(2))
                {
                    println!("Compute revision 2 uploaded and retained for shutdown");
                    self.state.compute.render_path_control_feedback =
                        "shutdown-hidden-candidate-ready".to_owned();
                    ComputeEditBurstStage::ShutdownHiddenCandidateHeld(plan)
                } else {
                    ComputeEditBurstStage::WaitingForShutdownHiddenCandidate(plan)
                }
            }
            ComputeEditBurstStage::ShutdownHiddenCandidateHeld(plan) => {
                ComputeEditBurstStage::ShutdownHiddenCandidateHeld(plan)
            }
            ComputeEditBurstStage::Complete => ComputeEditBurstStage::Complete,
        };
        let in_progress = !matches!(
            next_stage,
            ComputeEditBurstStage::AwaitingSpace(_)
                | ComputeEditBurstStage::ShutdownActivePreparationHeld(_)
                | ComputeEditBurstStage::ShutdownHiddenCandidateHeld(_)
                | ComputeEditBurstStage::Complete
        );
        self.state.compute.compute_edit_burst_stage = Some(next_stage);
        Ok(in_progress)
    }
}
