use super::*;
use compute_ray_render_path::COMPUTE_RAY_STRATEGY;

pub(super) struct ScenarioState {
    pub(super) raster: RasterScenarioState,
    pub(super) compute: ComputeScenarioState,
    pub(super) camera: CameraScenarioState,
    pub(super) evidence: EvidenceCollection,
    pub(super) interactive: Option<InteractiveState>,
}

pub(super) struct RasterScenarioState {
    pub(super) preparation_is_paused: bool,
    pub(super) lifecycle_edit: Option<VoxelEditCommand>,
    pub(super) post_upload_hold_reported: bool,
    pub(super) edit_burst_stage: Option<EditBurstStage>,
    pub(super) raster_region_count: usize,
    pub(super) last_overlay_report: Option<String>,
    pub(super) edit_burst_started_at: Option<Instant>,
}

pub(super) struct ComputeScenarioState {
    pub(super) compute_switch_requested: bool,
    pub(super) compute_switch_requested_at: Option<Instant>,
    pub(super) compute_first_frame_presented: bool,
    pub(super) compute_switch_complete_reported: bool,
    pub(super) last_held_replacement_stamp:
        Option<(CameraStateRevision, Option<PresentationConfigurationId>)>,
    pub(super) compute_switch_lifecycle_stage: Option<ComputeSwitchLifecycleStage>,
    pub(super) portable_milestone_lifecycle_complete: bool,
    pub(super) completed_interactive_switches: usize,
    pub(super) render_path_control_feedback: String,
    pub(super) compute_edit_burst_stage: Option<ComputeEditBurstStage>,
    pub(super) compute_edit_burst_events: Vec<ComputeConvergenceEvent>,
    pub(super) compute_edit_burst_presented_revisions: Vec<VoxelSceneRevision>,
}

pub(super) struct CameraScenarioState {
    pub(super) pending_camera_report: Option<String>,
    pub(super) camera_move_step: Option<u32>,
    pub(super) last_presented_camera: Option<String>,
}

impl ScenarioState {
    pub(super) fn new(configuration: &DesktopRenderConfiguration) -> Result<Self, String> {
        let measurement = configuration
            .measurement
            .as_ref()
            .map(MeasurementSession::new)
            .transpose()?;
        let compute_switch_lifecycle_stage = configuration
            .compute_switch_lifecycle_demo
            .then_some(ComputeSwitchLifecycleStage::Replacement);
        let interactive = if configuration.interactive {
            let mut state = InteractiveState::new(configuration.camera_pose()?);
            if let Err(reason) = configuration.admit_path_switch() {
                state.control_feedback = format!("Tab-rejected-{reason}");
            }
            Some(state)
        } else {
            None
        };
        Ok(Self {
            raster: RasterScenarioState {
                preparation_is_paused: false,
                lifecycle_edit: None,
                post_upload_hold_reported: false,
                edit_burst_stage: None,
                raster_region_count: 0,
                last_overlay_report: None,
                edit_burst_started_at: None,
            },
            compute: ComputeScenarioState {
                compute_switch_requested: false,
                compute_switch_requested_at: None,
                compute_first_frame_presented: false,
                compute_switch_complete_reported: false,
                last_held_replacement_stamp: None,
                compute_switch_lifecycle_stage,
                portable_milestone_lifecycle_complete: false,
                completed_interactive_switches: 0,
                render_path_control_feedback: "Tab-waiting-for-convergence".to_owned(),
                compute_edit_burst_stage: None,
                compute_edit_burst_events: Vec::new(),
                compute_edit_burst_presented_revisions: Vec::new(),
            },
            camera: CameraScenarioState {
                pending_camera_report: None,
                camera_move_step: None,
                last_presented_camera: None,
            },
            evidence: EvidenceCollection {
                measurement,
                occupied_voxels: 0,
                semantic_qualification: SemanticQualificationState::default(),
                first_matching_frame_presented: false,
            },
            interactive,
        })
    }
}

pub(super) struct ScenarioExecution<'a> {
    pub(super) desktop: &'a mut DesktopRuntime,
    pub(super) state: &'a mut ScenarioState,
}

pub(super) struct EditBurstPlan {
    pub(super) commands: VecDeque<VoxelEditCommand>,
    pub(super) expected_final_revision: VoxelSceneRevision,
    pub(super) input_owner: Option<EditBurstInputOwner>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EditBurstInputOwner {
    SpaceKeypress,
}

impl EditBurstPlan {
    pub(super) fn claim_space_keypress(&mut self) -> Result<(), String> {
        if self.input_owner.is_some() {
            return Err("the edit burst was already claimed by a Space keypress".to_owned());
        }
        self.input_owner = Some(EditBurstInputOwner::SpaceKeypress);
        Ok(())
    }

    pub(super) fn take_next_owned_command(&mut self) -> Result<VoxelEditCommand, String> {
        if self.input_owner != Some(EditBurstInputOwner::SpaceKeypress) {
            return Err("the edit burst commands are not owned by a Space keypress".to_owned());
        }
        self.commands
            .pop_front()
            .ok_or_else(|| "the edit burst has no remaining command".to_owned())
    }
}

pub(super) enum EditBurstStage {
    AwaitingKey(EditBurstPlan),
    WaitingForCpuBarrier(EditBurstPlan),
    WaitingForSecondRequirement(EditBurstPlan),
    CpuBarrierHeld(EditBurstPlan),
    WaitingForCpuCancellation(EditBurstPlan),
    WaitingForPostUploadCandidate(EditBurstPlan),
    PostUploadCandidateHeld(EditBurstPlan),
    WaitingForPostUploadCandidateAfterLifecycle(EditBurstPlan),
    WaitingForFinalRequirement(EditBurstPlan),
    PostUploadBarrierHeld(EditBurstPlan),
    WaitingForCandidateRejection(EditBurstPlan),
    WaitingForFinalVisibility(EditBurstPlan),
    Complete,
}

pub(super) enum ComputeEditBurstStage {
    AwaitingSpace(EditBurstPlan),
    WaitingForRevisionTwoRequirement(EditBurstPlan),
    WaitingForPreparationBarrier(EditBurstPlan),
    WaitingForRevisionThreeRequirement(EditBurstPlan),
    WaitingForRevisionTwoCancellation(EditBurstPlan),
    WaitingForRevisionThreeUpload(EditBurstPlan),
    WaitingForRevisionFourRequirement(EditBurstPlan),
    WaitingForRevisionThreeRejection(EditBurstPlan),
    WaitingForFinalVisibility(EditBurstPlan),
    ShutdownActivePreparationHeld(EditBurstPlan),
    WaitingForShutdownHiddenCandidate(EditBurstPlan),
    ShutdownHiddenCandidateHeld(EditBurstPlan),
    Complete,
}

impl ComputeEditBurstStage {
    pub(super) fn overlay_label(&self) -> &'static str {
        match self {
            Self::AwaitingSpace(_) => "awaiting-space",
            Self::WaitingForRevisionTwoRequirement(_) => "revision-2-requirement",
            Self::WaitingForPreparationBarrier(_) => "revision-2-one-block",
            Self::WaitingForRevisionThreeRequirement(_) => "revision-3-requirement",
            Self::WaitingForRevisionTwoCancellation(_) => "revision-2-cancellation",
            Self::WaitingForRevisionThreeUpload(_) => "revision-3-upload",
            Self::WaitingForRevisionFourRequirement(_) => "revision-4-requirement",
            Self::WaitingForRevisionThreeRejection(_) => "revision-3-rejection",
            Self::WaitingForFinalVisibility(_) => "revision-4-visibility",
            Self::ShutdownActivePreparationHeld(_) => "shutdown-active-preparation-held",
            Self::WaitingForShutdownHiddenCandidate(_) => "shutdown-hidden-candidate-upload",
            Self::ShutdownHiddenCandidateHeld(_) => "shutdown-hidden-candidate-held",
            Self::Complete => "complete",
        }
    }
}

impl EditBurstStage {
    pub(super) fn overlay_label(&self) -> &'static str {
        match self {
            Self::AwaitingKey(_) => "awaiting-key",
            Self::WaitingForCpuBarrier(_) => "waiting-cpu-barrier",
            Self::WaitingForSecondRequirement(_) => "second-requirement",
            Self::CpuBarrierHeld(_) => "cpu-barrier-held",
            Self::WaitingForCpuCancellation(_) => "cpu-cancellation",
            Self::WaitingForPostUploadCandidate(_) => "post-upload-candidate",
            Self::PostUploadCandidateHeld(_) => "post-upload-candidate-held",
            Self::WaitingForPostUploadCandidateAfterLifecycle(_) => {
                "post-upload-candidate-after-lifecycle"
            }
            Self::WaitingForFinalRequirement(_) => "final-requirement",
            Self::PostUploadBarrierHeld(_) => "post-upload-barrier-held",
            Self::WaitingForCandidateRejection(_) => "candidate-rejection",
            Self::WaitingForFinalVisibility(_) => "final-visibility",
            Self::Complete => "complete",
        }
    }

    pub(super) fn waits_for_external_event(&self) -> bool {
        matches!(
            self,
            Self::AwaitingKey(_)
                | Self::CpuBarrierHeld(_)
                | Self::PostUploadCandidateHeld(_)
                | Self::PostUploadBarrierHeld(_)
                | Self::Complete
        )
    }
}

pub(super) fn format_convergence_overlay(
    stage: &str,
    status: RasterConvergenceStatus,
    camera: &str,
) -> String {
    format!(
        "EditBurst={stage} Required={} Visible={} Affected={} Unaffected={} Camera={camera}",
        status.required_revision,
        status.visible_revision,
        status.affected_region_count,
        status.unaffected_region_count
    )
}

pub(super) fn format_convergence_characterization(
    characterization: &RasterConvergenceCharacterization,
) -> String {
    let cancellation_events = characterization
        .cancellation_observations
        .iter()
        .map(|observation| {
            format!(
                "{}:{}:{}:{}",
                observation.revision,
                observation.scheduled_regions,
                observation.completed_regions,
                observation.cancelled_regions
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    let safe_retirement_events = characterization
        .safe_retirements
        .iter()
        .map(|event| {
            let disposition = match event.disposition {
                RasterSafeRetirementDisposition::StaleCandidate => "stale-candidate",
                RasterSafeRetirementDisposition::ReplacedInstallation => "replaced-installation",
            };
            format!(
                "{}:{disposition}:{}:{}",
                event.revision, event.resources.bytes, event.resources.resources
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "submission_bookkeeping_ms={:.6} queued_wait_ms={:.6} cpu_derivation_ms={:.6} upload_ms={:.6} frame_boundary_commit_ms={:.6} scheduled_regions={} completed_regions={} cancelled_regions={} stale_regions={} installed_bytes={} installed_resources={} hidden_bytes={} hidden_resources={} retired_bytes={} retired_resources={} peak_bytes={} peak_resources={} cancellation_events={} safe_retirement_events={}",
        characterization.phases.submission_bookkeeping_milliseconds,
        characterization.phases.queued_wait_milliseconds,
        characterization.phases.cpu_derivation_milliseconds,
        characterization.phases.upload_milliseconds,
        characterization.phases.frame_boundary_commit_milliseconds,
        characterization.work.scheduled,
        characterization.work.completed,
        characterization.work.cancelled,
        characterization.work.stale,
        characterization.installed.bytes,
        characterization.installed.resources,
        characterization.hidden.bytes,
        characterization.hidden.resources,
        characterization.retired.bytes,
        characterization.retired.resources,
        characterization.peak.bytes,
        characterization.peak.resources,
        cancellation_events,
        safe_retirement_events,
    )
}

pub(super) fn should_start_edit_burst(
    edit_burst_demo: bool,
    awaiting_key: bool,
    state: ElementState,
    repeat: bool,
    key: &Key,
) -> bool {
    edit_burst_demo
        && awaiting_key
        && state == ElementState::Pressed
        && !repeat
        && matches!(key, Key::Named(NamedKey::Space))
}

pub(super) fn should_request_render_path_switch(
    compute_switch_demo: bool,
    state: ElementState,
    repeat: bool,
    key: &Key,
) -> bool {
    compute_switch_demo
        && state == ElementState::Pressed
        && !repeat
        && matches!(key, Key::Named(NamedKey::Tab))
}

pub(super) fn should_request_compute_edit_burst(
    compute_switch_demo: bool,
    state: ElementState,
    repeat: bool,
    key: &Key,
) -> bool {
    compute_switch_demo
        && state == ElementState::Pressed
        && !repeat
        && matches!(key, Key::Named(NamedKey::Space))
}

pub(super) fn should_wait_for_initial_raster_artifact(
    configuration: &DesktopRenderConfiguration,
    first_matching_frame_presented: bool,
) -> bool {
    configuration.compute_switch_demo && !first_matching_frame_presented
}

pub(super) fn compute_edit_burst_admission(
    diagnostics: &RenderPathSwitchDiagnostics,
    awaiting_space: bool,
) -> Result<(), String> {
    let roles = diagnostics.roles();
    if roles.presenting() != COMPUTE_RAY_STRATEGY {
        return Err("the compute Render Path is not presenting".to_owned());
    }
    if roles.replacement().is_some() || roles.retiring().is_some() {
        return Err("a Render Path switch is in progress".to_owned());
    }
    if !awaiting_space {
        return Err("the fixed compute edit burst is not awaiting Space".to_owned());
    }
    Ok(())
}

pub(super) fn render_path_switch_admission(
    diagnostics: &RenderPathSwitchDiagnostics,
) -> Result<(), RenderPathSwitchRequestError> {
    let roles = diagnostics.roles();
    if roles.replacement().is_some() || roles.retiring().is_some() {
        return Err(RenderPathSwitchRequestError::SwitchInProgress);
    }
    let presenting = diagnostics.presenting();
    if (presenting.required_selection() == presenting.installed_selection()
        && presenting.required_revision() != presenting.visible_revision())
        || presenting.readiness() != RenderPathReadiness::Recordable
    {
        return Err(RenderPathSwitchRequestError::PresentingPathNotConverged {
            required_revision: presenting.required_revision(),
            visible_revision: presenting.visible_revision(),
            readiness: presenting.readiness(),
        });
    }
    Ok(())
}

pub(super) fn render_path_switch_phase(diagnostics: &RenderPathSwitchDiagnostics) -> &'static str {
    match (diagnostics.replacement(), diagnostics.retiring()) {
        (Some(replacement), _) if replacement.readiness() == RenderPathReadiness::Recordable => {
            "handoff-ready"
        }
        (Some(_), _) => "preparing",
        (None, Some(_)) => "retiring",
        (None, None) => "idle",
    }
}

pub(super) fn format_render_path_overlay(
    diagnostics: &RenderPathSwitchDiagnostics,
    burst_stage: &str,
    camera: &str,
    control_feedback: &str,
) -> String {
    let switch_phase = render_path_switch_phase(diagnostics);
    let replacement_revision = diagnostics
        .replacement()
        .map(|replacement| replacement.required_revision().to_string())
        .unwrap_or_else(|| "none".to_owned());
    let presenting = diagnostics.presenting();
    format!(
        "Presenter={} Switch={switch_phase} ReplacementRevision={replacement_revision} Required={} Visible={} Burst={burst_stage} Camera={camera} Control={control_feedback}",
        presenting.strategy().identifier(),
        presenting.required_revision(),
        presenting.visible_revision(),
    )
}

pub(super) fn voxel_value_identity(value: &VoxelValue) -> String {
    match value {
        VoxelValue::Empty => "empty".to_owned(),
        VoxelValue::Occupied(identity) => format!("occupied:{identity:?}"),
    }
}

pub(super) fn fixed_edit_burst(
    view: &voxel_frontend::VoxelSceneView,
    raster_region_extent: u32,
) -> Result<EditBurstPlan, String> {
    let volume_identity = VoxelVolumeId::new("canonical-volume");
    let volume_width = view
        .volumes()
        .iter()
        .find(|metadata| metadata.identity() == &volume_identity)
        .ok_or_else(|| "the canonical edit-burst volume metadata is unavailable".to_owned())?
        .extent()
        .dimensions()[0];
    let command_spacing = 40.min(volume_width / 3);
    if command_spacing == 0 {
        return Err("the canonical edit-burst volume is too narrow for three commands".to_owned());
    }
    let third_coordinate = command_spacing
        .checked_mul(2)
        .ok_or_else(|| "the canonical edit-burst command coordinate overflowed".to_owned())?;
    let second_coordinate = i32::try_from(command_spacing)
        .map_err(|_| "the canonical edit-burst command coordinate is out of range".to_owned())?;
    let third_coordinate = i32::try_from(third_coordinate)
        .map_err(|_| "the canonical edit-burst command coordinate is out of range".to_owned())?;
    let coordinates = [
        VoxelCoordinate::new(0, 0, 0),
        VoxelCoordinate::new(second_coordinate, 0, 0),
        VoxelCoordinate::new(third_coordinate, 0, 0),
    ];
    let mut commands = VecDeque::new();
    for (index, coordinate) in coordinates.into_iter().enumerate() {
        let samples = view
            .read_region(
                &volume_identity,
                VoxelRegion::new(coordinate, VoxelExtent::new(1, 1, 1)),
            )
            .map_err(|error| error.to_string())?;
        let old_value = samples
            .first()
            .ok_or_else(|| format!("edit burst coordinate {coordinate:?} has no Voxel Sample"))?
            .value()
            .clone();
        let requested_value = match &old_value {
            VoxelValue::Empty => VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm")),
            VoxelValue::Occupied(_) => VoxelValue::Empty,
        };
        println!(
            "Edit burst command: order={} volume={volume_identity:?} coordinate={coordinate:?} old={} requested={}",
            index + 1,
            voxel_value_identity(&old_value),
            voxel_value_identity(&requested_value)
        );
        commands.push_back(VoxelEditCommand::new(
            volume_identity.clone(),
            coordinate,
            requested_value,
        ));
    }
    let expected_final_revision =
        (0..commands.len()).try_fold(view.revision(), |revision, _| {
            revision.checked_successor().ok_or_else(|| {
                "the checked expected final Voxel Scene Revision overflowed".to_owned()
            })
        })?;
    println!(
        "Edit burst inputs: scene={:?} generator=voxel-nexus-canonical-dense generator_version=1 initial_revision={} raster_region_extent={raster_region_extent}x{raster_region_extent}x{raster_region_extent} camera=overview installed_revision={} installed_complete=true expected_final_revision={expected_final_revision}",
        view.scene_id(),
        view.revision(),
        view.revision()
    );
    Ok(EditBurstPlan {
        commands,
        expected_final_revision,
        input_owner: None,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ComputeSwitchLifecycleStage {
    Replacement,
    CameraAcknowledgement,
    Landscape {
        previous_configuration: PresentationConfigurationId,
    },
    Portrait {
        previous_configuration: PresentationConfigurationId,
    },
    Suspension {
        previous_configuration: PresentationConfigurationId,
    },
    Restore {
        previous_configuration: PresentationConfigurationId,
    },
    PresentationRecreation {
        previous_configuration: PresentationConfigurationId,
    },
    Handoff,
}

impl ScenarioExecution<'_> {
    pub(super) fn report_drawable_extent(&self, drawable_extent: ash::vk::Extent2D) {
        if self.state.raster.preparation_is_paused {
            if drawable_extent.width == 0 || drawable_extent.height == 0 {
                println!("Desktop lifecycle serviced while preparation paused: suspended");
                self.desktop.set_status("preparation-paused suspended");
            } else {
                println!(
                    "Desktop lifecycle serviced while preparation paused: resize={}x{}",
                    drawable_extent.width, drawable_extent.height
                );
            }
        }
        if self.desktop.render_configuration.hold_post_upload_candidate {
            if drawable_extent.width == 0 || drawable_extent.height == 0 {
                println!("Desktop lifecycle serviced while post-upload candidate held: suspended");
                self.desktop.set_status("post-upload-held suspended");
            } else {
                println!(
                    "Desktop lifecycle serviced while post-upload candidate held: resize={}x{}",
                    drawable_extent.width, drawable_extent.height
                );
                self.desktop.set_status(&format!(
                    "post-upload-held lifecycle-responsive {}x{}",
                    drawable_extent.width, drawable_extent.height
                ));
            }
        }
        if self
            .desktop
            .render_configuration
            .compute_switch_lifecycle_demo
            && self.state.compute.compute_switch_requested
            && self
                .desktop
                .render_path_handoff_control
                .as_ref()
                .is_some_and(RenderPathHandoffControl::is_held)
        {
            if drawable_extent.width == 0 || drawable_extent.height == 0 {
                println!("Compute replacement held during zero-size presentation suspension");
                self.desktop
                    .set_status("compute-replacement-held suspended");
            } else {
                println!(
                    "Compute replacement held during presentation resize: {}x{}",
                    drawable_extent.width, drawable_extent.height
                );
                self.desktop.set_status(&format!(
                    "compute-replacement-held resize={}x{}",
                    drawable_extent.width, drawable_extent.height
                ));
            }
        }
    }
}

impl ScenarioExecution<'_> {
    pub(super) fn after_presented(
        &mut self,
        event_loop: &ActiveEventLoop,
        submitted_frame_sequence: Option<u64>,
    ) {
        let installed_revision = match &self.desktop.artifact_installer {
            Some(installer) => match installer.installed_source_revision() {
                Ok(revision) => revision,
                Err(error) => {
                    self.desktop.fail(event_loop, error);
                    return;
                }
            },
            None if matches!(
                self.desktop.render_configuration.scene,
                DesktopSceneSelection::StreamedWorld
            ) =>
            {
                match self.desktop.switch_diagnostics() {
                    Ok(diagnostics) if diagnostics.presenting().is_fully_converged() => {
                        Some(diagnostics.presenting().visible_revision())
                    }
                    Ok(_) => None,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                }
            }
            None => None,
        };
        if !self.state.evidence.first_matching_frame_presented
            && installed_revision.is_some()
            && installed_revision == self.desktop.published_revision
        {
            let revision = installed_revision.unwrap_or(VoxelSceneRevision::new(0));
            self.state.evidence.first_matching_frame_presented = true;
            println!("First matching raster frame presented: revision={revision}");
            if (self.desktop.render_configuration.compute_switch_demo
                && !self
                    .desktop
                    .render_configuration
                    .compute_switch_lifecycle_demo)
                || self.desktop.render_configuration.interactive
            {
                self.set_control_feedback("Tab-ready".to_owned());
            }
            if let Some(measurement) = &mut self.state.evidence.measurement {
                let presented_at = Instant::now();
                let elapsed_milliseconds = match measurement.elapsed_milliseconds(presented_at) {
                    Ok(elapsed_milliseconds) => elapsed_milliseconds,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                if let Err(error) =
                    measurement.record(MeasurementEvent::MatchingArtifactPresented {
                        source_revision: match measurement_revision(revision) {
                            Ok(source_revision) => source_revision,
                            Err(error) => {
                                self.desktop.fail(event_loop, error);
                                return;
                            }
                        },
                        elapsed_milliseconds,
                    })
                {
                    self.desktop.fail(event_loop, error);
                    return;
                }
                match measurement.mode {
                    MeasurementMode::FirstCorrectFrame => {
                        if let Some(backend) = &mut self.desktop.backend
                            && let Err(error) = backend.shutdown()
                        {
                            self.desktop.fail(event_loop, error);
                            return;
                        }
                        event_loop.exit();
                        return;
                    }
                    MeasurementMode::SteadyState => {
                        measurement.begin_steady_frames(presented_at);
                    }
                }
            }
            if self
                .desktop
                .render_configuration
                .compute_switch_lifecycle_demo
                && let Err(error) = self.request_compute_switch()
            {
                self.desktop.fail(event_loop, error);
                return;
            }
        }
        let compute_switch_in_progress = if self
            .desktop
            .render_configuration
            .compute_switch_lifecycle_demo
            && !self.state.compute.portable_milestone_lifecycle_complete
        {
            match self.update_compute_switch_demo() {
                Ok(in_progress) => in_progress,
                Err(error) => {
                    self.desktop.fail(event_loop, error);
                    return;
                }
            }
        } else if self
            .desktop
            .render_configuration
            .render_path_switching_enabled()
        {
            match self.update_interactive_render_path_switch() {
                Ok(in_progress) => in_progress,
                Err(error) => {
                    self.desktop.fail(event_loop, error);
                    return;
                }
            }
        } else {
            false
        };
        let compute_edit_burst_in_progress = if self.interactive_compute_switch_demo_active() {
            match self.update_compute_edit_burst() {
                Ok(in_progress) => in_progress,
                Err(error) => {
                    self.desktop.fail(event_loop, error);
                    return;
                }
            }
        } else {
            false
        };
        if self.desktop.render_configuration.compute_switch_demo {
            let Some(presented_frame_sequence) = submitted_frame_sequence else {
                self.desktop.fail(
                    event_loop,
                    "a presented frame has no submitted frame sequence",
                );
                return;
            };
            match self
                .state
                .evidence
                .semantic_qualification
                .collect(presented_frame_sequence)
            {
                Ok(true) => {
                    if let Some(window) = &self.desktop.window {
                        window.request_redraw();
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    self.desktop.fail(event_loop, error);
                    return;
                }
            }
        }
        if self
            .desktop
            .render_configuration
            .compute_switch_lifecycle_demo
            && !self.state.compute.portable_milestone_lifecycle_complete
        {
            if let Err(error) = self.drive_compute_lifecycle_after_presented() {
                self.desktop.fail(event_loop, error);
                return;
            }
            if self.state.compute.compute_switch_complete_reported
                && self.state.compute.compute_switch_lifecycle_stage
                    == Some(ComputeSwitchLifecycleStage::Handoff)
            {
                if self.desktop.render_configuration.portable_milestone_demo {
                    self.state.compute.portable_milestone_lifecycle_complete = true;
                    self.state.compute.compute_switch_lifecycle_stage = None;
                    self.state.compute.completed_interactive_switches = 1;
                    self.state.compute.render_path_control_feedback = "Space-ready".to_owned();
                    println!(
                        "Portable compute-ray milestone lifecycle complete: completed_switches=1 validation_errors=0"
                    );
                    if let Err(error) = self.set_render_path_overlay() {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                    if let Some(window) = &self.desktop.window {
                        window.request_redraw();
                    }
                } else {
                    if let Some(backend) = &mut self.desktop.backend
                        && let Err(error) = backend.shutdown()
                    {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                    println!(
                        "Compute replacement lifecycle qualification complete: validation_errors=0"
                    );
                    event_loop.exit();
                    return;
                }
            }
        }
        if (compute_switch_in_progress || compute_edit_burst_in_progress)
            && let Some(window) = &self.desktop.window
        {
            window.request_redraw();
        }
        if self.interactive_compute_switch_demo_active()
            && let Err(error) = self.set_render_path_overlay()
        {
            self.desktop.fail(event_loop, error);
            return;
        }
        let now = Instant::now();
        if let Some(measurement) = &mut self.state.evidence.measurement
            && measurement.mode == MeasurementMode::SteadyState
        {
            if measurement.steady_collection_has_ended(now) {
                if measurement.recorded_steady_frame_count() == 0 {
                    self.desktop.fail(
                        event_loop,
                        "steady-state collection produced no valid GPU timestamp samples",
                    );
                    return;
                }
                if let Some(backend) = &mut self.desktop.backend
                    && let Err(error) = backend.shutdown()
                {
                    self.desktop.fail(event_loop, error);
                    return;
                }
                event_loop.exit();
                return;
            }
            if let Some(window) = &self.desktop.window {
                window.request_redraw();
            }
        }
        if let Some(camera) = self.state.camera.pending_camera_report.take() {
            println!("Canonical camera presented: camera={camera}");
            self.desktop
                .set_status(&format!("camera-presented {camera}"));
            self.state.camera.last_presented_camera = Some(camera);
        }
        if self.state.raster.preparation_is_paused {
            self.desktop.set_status(&format!(
                "preparation-paused lifecycle-responsive {}x{}",
                self.desktop.drawable_extent.width, self.desktop.drawable_extent.height
            ));
        }
        self.advance_camera_move(event_loop);
        if let Some(controller) = &self.desktop.lifecycle_controller {
            match controller.post_upload_revision() {
                Ok(Some(revision)) => {
                    if !self.state.raster.post_upload_hold_reported {
                        println!("Post-upload raster candidate held: revision={revision}");
                        let status = match &self.state.camera.last_presented_camera {
                            Some(camera) => format!(
                                "camera-presented {camera} post-upload-held revision {revision}"
                            ),
                            None => {
                                format!("post-upload-held revision {revision}")
                            }
                        };
                        self.desktop.set_status(&status);
                        self.state.raster.post_upload_hold_reported = true;
                    }
                }
                Ok(None) => {
                    self.state.raster.post_upload_hold_reported = false;
                    if let Some(window) = &self.desktop.window {
                        window.request_redraw();
                    }
                }
                Err(error) => {
                    self.desktop.fail(event_loop, error);
                }
            }
        }
        self.advance_edit_burst(event_loop);
        if self.desktop.render_configuration.interactive {
            self.after_interactive_presented(event_loop);
            if event_loop.exiting() {
                return;
            }
        }
        if self.desktop.render_configuration.edit_burst_demo
            && let Some(controller) = &self.desktop.lifecycle_controller
        {
            match controller.convergence_status() {
                Ok(Some(status)) => {
                    if let Err(error) = self.set_convergence_overlay(status) {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    self.desktop.fail(event_loop, error);
                    return;
                }
            }
        }
        if self.desktop.render_configuration.edit_burst_demo
            && self
                .state
                .raster
                .edit_burst_stage
                .as_ref()
                .is_some_and(|stage| !stage.waits_for_external_event())
            && let Some(window) = &self.desktop.window
        {
            window.request_redraw();
        }
    }

    pub(super) fn keyboard_input(
        &mut self,
        event_loop: &ActiveEventLoop,
        event: &winit::event::KeyEvent,
    ) {
        if should_start_edit_burst(
            self.desktop.render_configuration.edit_burst_demo,
            matches!(
                self.state.raster.edit_burst_stage,
                Some(EditBurstStage::AwaitingKey(_))
            ),
            event.state,
            event.repeat,
            &event.logical_key,
        ) {
            self.start_edit_burst(event_loop);
        }
        if should_request_render_path_switch(
            self.interactive_compute_switch_demo_active(),
            event.state,
            event.repeat,
            &event.logical_key,
        ) {
            self.handle_render_path_switch_key(event_loop);
        }
        if should_request_compute_edit_burst(
            self.interactive_compute_switch_demo_active(),
            event.state,
            event.repeat,
            &event.logical_key,
        ) {
            self.handle_compute_edit_burst_key(event_loop);
        }
        if self.desktop.render_configuration.interactive {
            self.interactive_keyboard_input(event_loop, event);
        }
    }

    pub(super) fn before_close(&mut self) {
        if self.interactive_compute_switch_demo_active() {
            if self
                .desktop
                .render_configuration
                .compute_shutdown_qualification
                .is_some()
            {
                match self.compute_shutdown_qualification_report() {
                    Ok(report) => println!("{report}"),
                    Err(error) => self.desktop.record_close_error(error),
                }
            } else {
                match self.desktop.backend
                    .as_ref()
                    .and_then(RenderBackend::render_path_switch_diagnostics)
                {
                    Some(diagnostics)
                        if diagnostics.roles().presenting()
                            == COMPUTE_RAY_STRATEGY
                            && diagnostics.roles().replacement().is_none()
                            && diagnostics.roles().retiring().is_none()
                            && self.state.compute.completed_interactive_switches >= 3
                            && matches!(
                                self.state.compute.compute_edit_burst_stage,
                                Some(ComputeEditBurstStage::Complete)
                            ) =>
                    {
                        println!(
                            "Render Path round trip complete: raster-to-compute-to-raster-to-compute switches={} closing_presenter=ComputeRay",
                            self.state.compute.completed_interactive_switches
                        );
                    }
                    Some(diagnostics) => self.desktop.record_close_error(format!(
                        "the Render Path round trip must close idle with compute presenting after the newest-only edit burst and at least three switches: Presenting={:?} Replacement={:?} Retiring={:?} completed_switches={} burst={}",
                        diagnostics.roles().presenting(),
                        diagnostics.roles().replacement(),
                        diagnostics.roles().retiring(),
                        self.state.compute.completed_interactive_switches,
                        self.state.compute.compute_edit_burst_stage
                            .as_ref()
                            .map(ComputeEditBurstStage::overlay_label)
                            .unwrap_or("inactive")
                    )),
                    None => self.desktop.record_close_error(
                        "Render Path switching diagnostics are unavailable during close",
                    ),
                }
            }
        }
    }

    pub(super) fn verify_held_raster_candidate(&mut self) {
        if self.desktop.render_configuration.hold_post_upload_candidate {
            match self
                .desktop
                .lifecycle_controller
                .as_ref()
                .map(RasterLifecycleController::post_upload_revision)
                .transpose()
            {
                Ok(Some(Some(revision))) => {
                    println!(
                        "Closing with post-upload hidden raster candidate: revision={revision}"
                    );
                }
                Ok(_) => self.desktop.record_close_error(
                    "desktop close did not retain the required post-upload hidden candidate",
                ),
                Err(error) => self.desktop.record_close_error(error),
            }
        }
    }

    pub(super) fn after_shutdown(&mut self) {
        if let Err(error) = self.report_compute_timing_events() {
            self.desktop.record_close_error(error);
        }
        if let Err(error) = self.report_compute_resource_observations() {
            self.desktop.record_close_error(error);
        }
        if self
            .desktop
            .backend
            .as_ref()
            .is_some_and(|backend| backend.validation_error_count() != 0)
        {
            self.desktop
                .record_close_error("Vulkan validation reported errors during shutdown");
        }
        if let Some(controller) = &self.desktop.lifecycle_controller {
            match controller.shutdown_owned_resource_count() {
                Ok(Some(0)) => {
                    println!("Render Path-owned raster resources after shutdown: 0");
                }
                Ok(Some(count)) => self.desktop.record_close_error(format!(
                    "Render Path shutdown retained {count} owned raster resources"
                )),
                Ok(None) => self.desktop.record_close_error(
                    "Render Path shutdown did not report owned raster resource disposal",
                ),
                Err(error) => self.desktop.record_close_error(error),
            }
        }
        // Interactive sessions own compute resources only after switching to compute.
        if self.desktop.render_configuration.compute_switch_demo
            || (self.desktop.render_configuration.interactive
                && self.desktop.compute_lifecycle_controller.is_some())
        {
            let compute_resources = self
                .desktop
                .compute_lifecycle_controller
                .as_ref()
                .map(ComputeLifecycleController::shutdown_owned_resources)
                .transpose();
            match compute_resources {
                Ok(Some(Some(resources))) if resources.is_zero() => println!(
                    "Render Path-owned compute resources after shutdown: objects=0 allocations=0 workers=0 views=0"
                ),
                Ok(Some(Some(resources))) => self.desktop.record_close_error(format!(
                    "compute shutdown retained objects={} allocations={} workers={} views={}",
                    resources.objects(),
                    resources.allocations(),
                    resources.workers(),
                    resources.views()
                )),
                Ok(Some(None)) => self
                    .desktop
                    .record_close_error("compute shutdown did not report owned resource disposal"),
                Ok(None) => self.desktop.record_close_error(
                    "compute lifecycle diagnostics are unavailable during close",
                ),
                Err(error) => self.desktop.record_close_error(error),
            }
            if let Some(diagnostics) = self
                .desktop
                .backend
                .as_ref()
                .and_then(RenderBackend::render_path_switch_diagnostics)
            {
                if diagnostics.roles().replacement().is_none()
                    && diagnostics.roles().retiring().is_none()
                {
                    println!(
                        "Render Path switching resources after shutdown: replacement=0 retiring=0"
                    );
                } else {
                    self.desktop.record_close_error(
                        "Render Path switching retained replacement or retiring ownership after shutdown",
                    );
                }
            }
        }
    }

    pub(super) fn user_event(&mut self, event_loop: &ActiveEventLoop, event: DesktopEvent) {
        match event {
            #[cfg(feature = "qualification")]
            DesktopEvent::Preparation(RasterArtifactPreparationEvent::PausedAtBarrier {
                source_revision,
            }) => {
                self.state.raster.preparation_is_paused = true;
                println!("Background raster preparation paused: revision={source_revision}");
                self.desktop
                    .set_status(&format!("preparation-paused revision {source_revision}"));
            }
            DesktopEvent::Preparation(RasterArtifactPreparationEvent::Completed { .. }) => {
                self.complete_preparation(event_loop);
            }
            DesktopEvent::ReleasePreparation => {
                let Some(release) = self.desktop.preparation_release.take() else {
                    self.desktop
                        .fail(event_loop, "background preparation is not paused");
                    return;
                };
                if let Err(error) = release.release() {
                    self.desktop.fail(event_loop, error);
                    return;
                }
                self.state.raster.preparation_is_paused = false;
                println!("Background raster preparation released");
                self.desktop.set_status("preparation-released");
            }
            DesktopEvent::SelectCamera(pose) => {
                self.select_camera(event_loop, DesktopCameraSelection::Fixed(pose));
            }
            DesktopEvent::StartCameraMove => {
                let movement = match overview_to_cavity_camera_move() {
                    Ok(movement) => movement,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                let pose = match movement.pose_at_step(0) {
                    Ok(pose) => pose,
                    Err(error) => {
                        self.desktop.fail(event_loop, error);
                        return;
                    }
                };
                if let Err(error) = self.desktop.publish_camera_state(pose) {
                    self.desktop.fail(event_loop, error);
                    return;
                }
                self.state.camera.camera_move_step = Some(0);
                println!(
                    "Deterministic camera move started: steps={}",
                    movement.total_steps()
                );
                if let Some(window) = &self.desktop.window {
                    window.request_redraw();
                }
            }
            DesktopEvent::ReleaseEditCpuBarrier => self.release_edit_cpu_barrier(event_loop),
            DesktopEvent::ReleaseEditPostUploadLifecycleBarrier => {
                self.release_edit_post_upload_lifecycle_barrier(event_loop);
            }
            DesktopEvent::ReleaseEditPostUploadBarrier => {
                self.release_edit_post_upload_barrier(event_loop);
            }
            DesktopEvent::ReleaseComputeHandoff => {
                if let Err(error) = self.release_compute_handoff() {
                    self.desktop.fail(event_loop, error);
                }
            }
        }
    }
}
