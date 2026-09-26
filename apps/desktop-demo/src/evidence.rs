use super::*;

pub(super) struct MeasurementSession {
    pub(super) mode: MeasurementMode,
    output: BufWriter<File>,
    pub(super) publication_at: Option<Instant>,
    pub(super) derivation_at: Option<Instant>,
    pub(super) installation_at: Option<Instant>,
    steady_frames: Option<SteadyFrameCollection>,
}

pub(super) struct CpuFrameMeasurement {
    pub(super) sequence: u64,
    pub(super) started_at: Instant,
    pub(super) milliseconds: f64,
}

pub(super) struct SteadyFrameCollection {
    pub(super) warmup_ends_at: Instant,
    pub(super) collection_ends_at: Instant,
    pub(super) recorded_frame_count: u64,
    pub(super) first_submitted_sequence: Option<u64>,
    pub(super) pending_cpu_frames: VecDeque<CpuFrameMeasurement>,
}

impl SteadyFrameCollection {
    pub(super) fn new(matching_presentation_at: Instant) -> Self {
        let warmup_ends_at = matching_presentation_at + Duration::from_secs(5);
        Self {
            warmup_ends_at,
            collection_ends_at: warmup_ends_at + Duration::from_secs(30),
            recorded_frame_count: 0,
            first_submitted_sequence: None,
            pending_cpu_frames: VecDeque::new(),
        }
    }

    pub(super) fn submit(&mut self, frame: CpuFrameMeasurement) {
        self.first_submitted_sequence.get_or_insert(frame.sequence);
        self.pending_cpu_frames.push_back(frame);
    }

    pub(super) fn complete(
        &mut self,
        gpu_observation: render_backend::FrameObservation,
    ) -> Result<Option<MeasurementEvent>, String> {
        let cpu_frame_position = self
            .pending_cpu_frames
            .iter()
            .position(|frame| frame.sequence == gpu_observation.sequence);
        let Some(cpu_frame_position) = cpu_frame_position else {
            if self
                .first_submitted_sequence
                .is_none_or(|first_sequence| gpu_observation.sequence < first_sequence)
            {
                return Ok(None);
            }
            return Err(format!(
                "GPU observation sequence {} has no matching CPU frame",
                gpu_observation.sequence
            ));
        };
        let cpu_frame = self
            .pending_cpu_frames
            .remove(cpu_frame_position)
            .ok_or_else(|| "matching CPU frame disappeared before measurement".to_owned())?;
        if cpu_frame.started_at < self.warmup_ends_at
            || cpu_frame.started_at >= self.collection_ends_at
        {
            return Ok(None);
        }
        self.recorded_frame_count = self
            .recorded_frame_count
            .checked_add(1)
            .ok_or_else(|| "steady frame count overflowed".to_owned())?;
        Ok(Some(MeasurementEvent::SteadyFrame {
            sequence: gpu_observation.sequence,
            cpu_frame_milliseconds: cpu_frame.milliseconds,
            gpu_frame_milliseconds: gpu_observation.gpu_frame_milliseconds,
        }))
    }

    pub(super) fn collection_has_ended(&self, now: Instant) -> bool {
        now >= self.collection_ends_at
    }
}

impl MeasurementSession {
    pub(super) fn new(configuration: &MeasurementConfiguration) -> Result<Self, String> {
        let output = File::create(&configuration.output).map_err(|error| {
            format!(
                "could not create measurement output {}: {error}",
                configuration.output.display()
            )
        })?;
        Ok(Self {
            mode: configuration.mode,
            output: BufWriter::new(output),
            publication_at: None,
            derivation_at: None,
            installation_at: None,
            steady_frames: None,
        })
    }

    pub(super) fn elapsed_milliseconds(&self, at: Instant) -> Result<f64, String> {
        self.publication_at
            .map(|publication_at| at.duration_since(publication_at).as_secs_f64() * 1_000.0)
            .ok_or_else(|| "measurement began before Voxel Scene Revision publication".to_owned())
    }

    pub(super) fn record(&mut self, event: MeasurementEvent) -> Result<(), String> {
        let line = event
            .to_json_line()
            .map_err(|error| format!("could not serialize measurement event: {error}"))?;
        writeln!(self.output, "{line}")
            .and_then(|()| self.output.flush())
            .map_err(|error| format!("could not write measurement event: {error}"))
    }

    pub(super) fn begin_steady_frames(&mut self, matching_presentation_at: Instant) {
        self.steady_frames = Some(SteadyFrameCollection::new(matching_presentation_at));
    }

    pub(super) fn submit_cpu_frame(&mut self, frame: CpuFrameMeasurement) {
        if let Some(steady_frames) = &mut self.steady_frames {
            steady_frames.submit(frame);
        }
    }

    pub(super) fn complete_gpu_frame(
        &mut self,
        observation: render_backend::FrameObservation,
    ) -> Result<(), String> {
        let Some(steady_frames) = &mut self.steady_frames else {
            return Ok(());
        };
        let event = steady_frames.complete(observation)?;
        if let Some(event) = event {
            self.record(event)?;
        }
        Ok(())
    }

    pub(super) fn steady_collection_has_ended(&self, now: Instant) -> bool {
        self.steady_frames
            .as_ref()
            .is_some_and(|steady_frames| steady_frames.collection_has_ended(now))
    }

    pub(super) fn recorded_steady_frame_count(&self) -> u64 {
        self.steady_frames
            .as_ref()
            .map(|steady_frames| steady_frames.recorded_frame_count)
            .unwrap_or(0)
    }
}

#[derive(Default)]
pub(super) struct SemanticQualificationState {
    oracle_observations: Vec<SemanticRayProbeObservation>,
    compute_controllers: Vec<ComputeSemanticRayController>,
    raster_controllers: Vec<RasterSemanticFaceController>,
    compute_observations: Vec<ComputeSemanticRayProbeObservation>,
    raster_observations: Vec<RasterSemanticFaceObservation>,
    presented_frame_sequences: Vec<u64>,
    pending_compute_observation_count: usize,
    pending_raster_observation_count: usize,
}

pub(super) fn semantic_result_json(observation: &SemanticRayObservation) -> serde_json::Value {
    match observation.result() {
        SemanticRayResult::Miss => serde_json::json!({ "result": "miss" }),
        SemanticRayResult::Contact(contact) => {
            let (classification, outward_normal) = match contact.classification() {
                SemanticRayContactClassification::Entered(normal) => {
                    ("entered", Some(format!("{normal:?}")))
                }
                SemanticRayContactClassification::StartedInside => ("started_inside", None),
            };
            serde_json::json!({
                "result": "contact",
                "volume_identity": format!("{:?}", contact.volume_identity()),
                "coordinate": contact.coordinate().components(),
                "material_identity": format!("{:?}", contact.material_identity()),
                "distance": contact.distance(),
                "classification": classification,
                "outward_normal": outward_normal,
            })
        }
    }
}

pub(super) fn report_semantic_probe_definitions(probes: &[SemanticRayProbe]) {
    for probe in probes {
        let ray = probe.ray();
        println!(
            "Semantic evidence: {}",
            serde_json::json!({
                "kind": "probe_definition",
                "probe_identity": probe.identity(),
                "origin": ray.origin(),
                "direction": ray.direction(),
                "minimum_distance": ray.minimum_distance(),
                "maximum_distance": ray.maximum_distance(),
            })
        );
    }
}

impl SemanticQualificationState {
    pub(super) fn register_compute(
        &mut self,
        adapter: &mut ComputeRayRenderPathAdapter,
        view: &VoxelSceneView,
    ) -> Result<(), String> {
        let probes = canonical_edit_semantic_ray_probes().map_err(|error| error.to_string())?;
        report_semantic_probe_definitions(&probes);
        let oracle = probes
            .iter()
            .map(|probe| observe_probe(view, probe).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let controller = adapter.enable_semantic_ray_observation();
        controller
            .request(probes.to_vec())
            .map_err(|error| error.to_string())?;
        self.pending_compute_observation_count = self
            .pending_compute_observation_count
            .checked_add(probes.len())
            .ok_or_else(|| "compute Semantic Ray observation count overflowed".to_owned())?;
        self.oracle_observations.extend(oracle);
        self.compute_controllers.push(controller);
        Ok(())
    }

    pub(super) fn register_raster(
        &mut self,
        adapter: &mut RasterRenderPathAdapter,
        view: &VoxelSceneView,
    ) -> Result<(), String> {
        let probes = canonical_edit_semantic_ray_probes().map_err(|error| error.to_string())?;
        report_semantic_probe_definitions(&probes);
        let oracle = probes
            .iter()
            .map(|probe| observe_probe(view, probe).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let controller = adapter.enable_semantic_face_observation();
        controller
            .request(oracle.clone())
            .map_err(|error| error.to_string())?;
        self.pending_raster_observation_count = self
            .pending_raster_observation_count
            .checked_add(oracle.len())
            .ok_or_else(|| "raster Semantic Face observation count overflowed".to_owned())?;
        self.oracle_observations.extend(oracle);
        self.raster_controllers.push(controller);
        Ok(())
    }

    pub(super) fn request_active_compute(&mut self, view: &VoxelSceneView) -> Result<(), String> {
        let probes = canonical_edit_semantic_ray_probes().map_err(|error| error.to_string())?;
        report_semantic_probe_definitions(&probes);
        let oracle = probes
            .iter()
            .map(|probe| observe_probe(view, probe).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        self.compute_controllers
            .last()
            .ok_or_else(|| "compute Semantic Ray observation control is unavailable".to_owned())?
            .request(probes.to_vec())
            .map_err(|error| error.to_string())?;
        self.pending_compute_observation_count = self
            .pending_compute_observation_count
            .checked_add(probes.len())
            .ok_or_else(|| "compute Semantic Ray observation count overflowed".to_owned())?;
        self.oracle_observations.extend(oracle);
        Ok(())
    }

    pub(super) fn collect(&mut self, presented_frame_sequence: u64) -> Result<bool, String> {
        if !self
            .presented_frame_sequences
            .contains(&presented_frame_sequence)
        {
            self.presented_frame_sequences
                .push(presented_frame_sequence);
        }
        let tolerance =
            SemanticRayDistanceTolerance::new(1.0e-4).map_err(|error| error.to_string())?;
        for controller in &self.compute_controllers {
            for observation in controller.drain().map_err(|error| error.to_string())? {
                if !self
                    .presented_frame_sequences
                    .contains(&observation.frame_sequence())
                {
                    return Err(format!(
                        "compute Semantic Ray observation {} came from unpresented frame {}",
                        observation.probe_identity(),
                        observation.frame_sequence()
                    ));
                }
                let actual = observation.observation();
                if actual.revision() == VoxelSceneRevision::new(2)
                    || actual.revision() == VoxelSceneRevision::new(3)
                {
                    return Err(format!(
                        "obsolete compute revision {} produced Semantic Ray observation {}",
                        actual.revision(),
                        observation.probe_identity()
                    ));
                }
                let oracle = self
                    .oracle_observations
                    .iter()
                    .find(|oracle| {
                        oracle.probe_identity() == observation.probe_identity()
                            && oracle.observation().scene_identity() == actual.scene_identity()
                            && oracle.observation().revision() == actual.revision()
                    })
                    .ok_or_else(|| {
                        format!(
                            "compute Semantic Ray observation {} at revision {} has no oracle attribution",
                            observation.probe_identity(),
                            actual.revision()
                        )
                    })?;
                if !actual.agrees_with(oracle.observation(), tolerance) {
                    return Err(format!(
                        "compute Semantic Ray observation {} at revision {} disagrees with the oracle: compute={actual:?} oracle={:?}",
                        observation.probe_identity(),
                        actual.revision(),
                        oracle.observation()
                    ));
                }
                println!(
                    "Semantic qualification: path=ComputeRay probe={} revision={} frame={} result=pass",
                    observation.probe_identity(),
                    actual.revision(),
                    observation.frame_sequence()
                );
                println!(
                    "Semantic evidence: {}",
                    serde_json::json!({
                        "kind": "observation",
                        "render_path": "compute_ray",
                        "probe_identity": observation.probe_identity(),
                        "revision": actual.revision().to_string(),
                        "frame_sequence": observation.frame_sequence(),
                        "actual": semantic_result_json(actual),
                        "oracle": semantic_result_json(oracle.observation()),
                        "distance_tolerance": tolerance.maximum_absolute_difference(),
                        "passed": true,
                    })
                );
                self.pending_compute_observation_count = self
                    .pending_compute_observation_count
                    .checked_sub(1)
                    .ok_or_else(|| {
                        "compute Semantic Ray observation arrived without a pending request"
                            .to_owned()
                    })?;
                self.compute_observations.push(observation);
            }
        }
        for controller in &self.raster_controllers {
            for observation in controller.drain().map_err(|error| error.to_string())? {
                if !self
                    .presented_frame_sequences
                    .contains(&observation.frame_sequence())
                {
                    return Err(format!(
                        "raster Semantic Face observation {} came from unpresented frame {}",
                        observation.probe_identity(),
                        observation.frame_sequence()
                    ));
                }
                if !observation.passed() {
                    return Err(format!(
                        "raster Semantic Face correspondence failed for probe {} at revision {}: {:?}",
                        observation.probe_identity(),
                        observation.revision(),
                        observation.correspondence()
                    ));
                }
                let oracle = self
                    .oracle_observations
                    .iter()
                    .find(|oracle| {
                        oracle.probe_identity() == observation.probe_identity()
                            && oracle.observation().scene_identity() == observation.scene_identity()
                            && oracle.observation().revision() == observation.revision()
                    })
                    .ok_or_else(|| {
                        format!(
                            "raster Semantic Face observation {} at revision {} has no oracle attribution",
                            observation.probe_identity(),
                            observation.revision()
                        )
                    })?;
                println!(
                    "Semantic qualification: path=Raster probe={} revision={} frame={} correspondence={:?} result=pass",
                    observation.probe_identity(),
                    observation.revision(),
                    observation.frame_sequence(),
                    observation.correspondence()
                );
                println!(
                    "Semantic evidence: {}",
                    serde_json::json!({
                        "kind": "observation",
                        "render_path": "raster",
                        "probe_identity": observation.probe_identity(),
                        "revision": observation.revision().to_string(),
                        "frame_sequence": observation.frame_sequence(),
                        "oracle": semantic_result_json(oracle.observation()),
                        "correspondence": format!("{:?}", observation.correspondence()),
                        "passed": true,
                    })
                );
                self.pending_raster_observation_count = self
                    .pending_raster_observation_count
                    .checked_sub(1)
                    .ok_or_else(|| {
                        "raster Semantic Face observation arrived without a pending request"
                            .to_owned()
                    })?;
                self.raster_observations.push(observation);
            }
        }
        Ok(self.pending_compute_observation_count > 0 || self.pending_raster_observation_count > 0)
    }
}

pub(super) struct EvidenceCollection {
    pub(super) measurement: Option<MeasurementSession>,
    pub(super) occupied_voxels: u64,
    pub(super) semantic_qualification: SemanticQualificationState,
    pub(super) first_matching_frame_presented: bool,
}

impl ScenarioExecution<'_> {
    pub(super) fn report_compute_timing_events(&self) -> Result<(), String> {
        let Some(controller) = &self.desktop.compute_measurement_controller else {
            return Ok(());
        };
        for event in controller.drain().map_err(|error| error.to_string())? {
            println!(
                "Compute timing event: phase={:?} scene={:?} revision={} generation={} elapsed_ms={:.6}",
                event.phase(),
                event.scene_identity(),
                event.revision(),
                event.generation(),
                event.elapsed_milliseconds(),
            );
        }
        Ok(())
    }

    pub(super) fn report_compute_resource_observations(&self) -> Result<(), String> {
        let Some(controller) = &self.desktop.compute_lifecycle_controller else {
            return Ok(());
        };
        for observation in controller
            .drain_resource_observations()
            .map_err(|error| error.to_string())?
        {
            let status = observation.status();
            let resources = observation.resources();
            println!(
                "Compute resource observation: sequence={} point={:?} role={:?} scene={:?} installed_revision={} installed_generation={} preparing={:?} pending={:?} paused={:?} hidden={:?} cleanup_debt={:?} bytes={} objects={} allocations={} workers={} views={}",
                observation.sequence(),
                observation.point(),
                observation.role(),
                observation.scene_identity(),
                status.installed().revision(),
                status.installed().generation().value(),
                status.preparing(),
                status.pending(),
                status.paused(),
                status.hidden(),
                status.cleanup_debt(),
                resources.bytes(),
                resources.objects(),
                resources.allocations(),
                resources.workers(),
                resources.views(),
            );
        }
        Ok(())
    }

    pub(super) fn after_draw(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame_started_at: Instant,
        submitted_frame_sequence: Option<u64>,
        gpu_observation: Option<render_backend::FrameObservation>,
        presentation_extent: Option<ash::vk::Extent2D>,
    ) {
        if matches!(
            self.desktop
                .render_configuration
                .measurement
                .as_ref()
                .map(|measurement| measurement.mode),
            Some(MeasurementMode::SteadyState)
        ) && presentation_extent != Some(STEADY_MEASUREMENT_EXTENT)
        {
            self.desktop.fail(
                event_loop,
                "steady-state measurement lost its 1920x1080 presentation extent",
            );
            return;
        }
        let cpu_frame_milliseconds = frame_started_at.elapsed().as_secs_f64() * 1_000.0;
        if self.desktop.render_configuration.compute_switch_demo
            && let Some(diagnostics) = self
                .desktop
                .backend
                .as_ref()
                .and_then(RenderBackend::render_path_switch_diagnostics)
        {
            println!(
                "Render Path timing event: phase=Presentation render_path={:?} revision={} elapsed_ms={cpu_frame_milliseconds:.6}",
                diagnostics.presenting().strategy(),
                diagnostics.presenting().visible_revision(),
            );
        }
        if let Err(error) = self.report_compute_timing_events() {
            self.desktop.fail(event_loop, error);
            return;
        }
        if let Err(error) = self.report_compute_resource_observations() {
            self.desktop.fail(event_loop, error);
            return;
        }
        if let Some(measurement) = &mut self.state.evidence.measurement
            && measurement.mode == MeasurementMode::SteadyState
            && let Some(sequence) = submitted_frame_sequence
        {
            measurement.submit_cpu_frame(CpuFrameMeasurement {
                sequence,
                started_at: frame_started_at,
                milliseconds: cpu_frame_milliseconds,
            });
        }
        if let Some(measurement) = &mut self.state.evidence.measurement
            && measurement.mode == MeasurementMode::SteadyState
            && let Some(gpu_observation) = gpu_observation
            && let Err(error) = measurement.complete_gpu_frame(gpu_observation)
        {
            self.desktop.fail(event_loop, error);
        }
    }
}

impl EvidenceCollection {
    pub(super) fn scene_published(&mut self, revision: VoxelSceneRevision) -> Result<(), String> {
        if let Some(measurement) = &mut self.measurement {
            measurement.publication_at = Some(Instant::now());
            measurement.record(MeasurementEvent::SceneRevisionPublished {
                source_revision: measurement_revision(revision)?,
                elapsed_milliseconds: 0.0,
            })?;
        }
        Ok(())
    }
}
