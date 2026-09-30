use super::{
    allocation::{self, Category},
    streamed_fixture_recipe as recipe, streamed_qualification as cpu,
    streamed_qualification_observation::*,
    streamed_qualification_oracle::Oracle,
    streamed_residency_probes::Probes,
    streamed_residency_route as route, windows_adapter,
};
use ash::vk;
use compute_ray_render_path::{
    ComputeRayRenderPathAdapter, ComputeRepresentation, ComputeSceneBuildError,
};
use raster_render_path::{RASTER_STRATEGY, RasterRenderPathAdapter, derive_raster_residency};
use render_backend::*;
use serde_json::json;
use std::{
    cell::RefCell,
    fs::File,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use voxel_frontend::{
    VoxelExtent, VoxelFrontend, VoxelResidencySelection, VoxelResidencySelectionId, VoxelSceneView,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

const EXTENT: vk::Extent2D = vk::Extent2D {
    width: 1920,
    height: 1080,
};

struct Runner {
    backend: RenderBackend,
    frontend: Arc<VoxelFrontend>,
    oracle: Oracle,
    gpu: GpuAllocationQualification,
    boundary: Rc<RefCell<BoundaryObservation>>,
    probes: Probes,
    replacement: Option<Probes>,
    path_observation: Rc<RefCell<PathObservation>>,
    observations: Vec<Rc<RefCell<PathObservation>>>,
    next_owner: u64,
    next_selection: u64,
    camera: CameraState,
    camera_revision: CameraStateRevision,
    installed: VoxelResidencySelection,
    side: u32,
    history: Vec<VoxelSceneView>,
    history_peak: usize,
}

fn build(
    frontend: &Arc<VoxelFrontend>,
    selection: VoxelResidencySelection,
    raster: bool,
    camera: CameraState,
    revision: CameraStateRevision,
    identity: u64,
) -> Result<(MeasuredPath, Probes), String> {
    let view = frontend.scene_view().map_err(|error| error.to_string())?;
    let mut path = if raster {
        let artifact = allocation::within(Category::Raster, || {
            derive_raster_residency(
                frontend.clone(),
                &view,
                selection,
                VoxelExtent::new(16, 16, 16),
            )
        })
        .map_err(|error| error.to_string())?;
        Path::Raster(Box::new(
            allocation::within(Category::Raster, || {
                RasterRenderPathAdapter::from_residency_artifact(artifact, camera, revision)
            })
            .map_err(|error| error.to_string())?,
        ))
    } else {
        Path::Brickmap(Box::new(
            allocation::within(Category::Brickmap, || {
                ComputeRayRenderPathAdapter::new_streamed(
                    frontend.clone(),
                    selection,
                    camera,
                    revision,
                    ComputeRepresentation::Brickmap {
                        budget_bytes: 1 << 30,
                    },
                )
            })
            .map_err(|error| error.to_string())?,
        ))
    };
    let raster_control = match &mut path {
        Path::Raster(path) => Some(path.enable_lifecycle_control()),
        _ => None,
    };
    let mut path = MeasuredPath {
        path,
        identity,
        observation: Rc::new(RefCell::new(PathObservation {
            owned: true,
            raster,
            ..PathObservation::default()
        })),
        raster_control,
    };
    let probes = path.probes();
    Ok((path, probes))
}
impl Runner {
    fn new(window: &Window, mode: &str) -> Result<Self, String> {
        let gpu = GpuAllocationQualification::start().map_err(|error| error.to_string())?;
        let side = if mode == "matched-8" { 8 } else { 16 };
        let frontend = cpu::publish(side)?;
        let view = frontend.scene_view().map_err(|error| error.to_string())?;
        let camera = route::camera(0.0)?;
        let selection = cpu::selection(&view, 1, (3, 3), side)?;
        cpu::establish(&frontend, selection.clone())?;
        if !matches!(
            ComputeRayRenderPathAdapter::new_streamed(
                frontend.clone(),
                selection.clone(),
                camera,
                CameraStateRevision::new(1),
                ComputeRepresentation::Dense
            ),
            Err(ComputeSceneBuildError::StreamedDense)
        ) {
            return Err("production streamed Dense rejection failed".into());
        }
        let (path, probes) = build(
            &frontend,
            selection.clone(),
            mode != "brickmap",
            camera,
            CameraStateRevision::new(1),
            1,
        )?;
        let path_observation = path.observation.clone();
        let boundary = Rc::new(RefCell::new(BoundaryObservation::default()));
        let owner = ObservedOwner {
            owner: RenderPathSwitchOwner::new(Box::new(path)),
            observation: boundary.clone(),
        };
        let backend = RenderBackend::initialize_with_options(
            c"Production streamed qualification",
            &windows_adapter::WindowsPresentationAdapter::new(window),
            EXTENT,
            owner,
            RenderBackendOptions {
                validation_enabled: true,
                presentation_throttling_enabled: false,
                gpu_timestamps_enabled: true,
            },
        )
        .map_err(|error| error.to_string())?;
        if backend.presentation_extent() != Some(EXTENT) {
            return Err("drawable differs from frozen projection".into());
        }
        Ok(Self {
            backend,
            frontend,
            oracle: Oracle::new(side),
            gpu,
            boundary,
            probes,
            replacement: None,
            observations: vec![path_observation.clone()],
            path_observation,
            next_owner: 2,
            next_selection: 2,
            camera,
            camera_revision: CameraStateRevision::new(1),
            installed: selection,
            side,
            history: Vec::new(),
            history_peak: 0,
        })
    }
    fn draw(&mut self) -> Result<(), String> {
        self.backend
            .draw_frame()
            .map_err(|error| error.to_string())?;
        Ok(())
    }
    fn diagnostics(&self) -> Result<RenderPathSwitchDiagnostics, String> {
        self.backend
            .render_path_switch_diagnostics()
            .ok_or("missing production switch diagnostics".into())
    }
    fn sample(&self, output: &mut File, phase: &str) -> Result<(), String> {
        let cache = self
            .frontend
            .materialization_cache_stats()
            .map_err(|error| error.to_string())?;
        let edits = self
            .frontend
            .streamed_edit_statistics()
            .map_err(|error| error.to_string())?
            .ok_or("missing edit stats")?;
        let memory = self.gpu.snapshot().map_err(|error| error.to_string())?;
        let diagnostics = self.diagnostics()?;
        let owners: usize = self
            .observations
            .iter()
            .map(|owner| {
                let owner = owner.borrow();
                usize::from(owner.owned) * (1 + usize::from(owner.representation_copies > 9))
            })
            .sum();
        let raster_workers: usize = self
            .observations
            .iter()
            .map(|owner| {
                let owner = owner.borrow();
                if owner.owned && owner.raster {
                    owner.workers
                } else {
                    0
                }
            })
            .sum();
        let brickmap_workers: usize = self
            .observations
            .iter()
            .map(|owner| {
                let owner = owner.borrow();
                if owner.owned && !owner.raster {
                    owner.workers
                } else {
                    0
                }
            })
            .sum();
        let observation = self.path_observation.borrow();
        let view = self
            .frontend
            .scene_view()
            .map_err(|error| error.to_string())?;
        cpu::emit(
            output,
            json!({"kind":"residency","phase":phase,"copies":cache.copies,"peak_copies":cache.peak_copies,"query_copies":cache.query_only_copies,
            "owners":owners,"raster_workers":raster_workers,"brickmap_workers":brickmap_workers,"workers":observation.workers,"representation_copies":observation.representation_copies,
            "cpu_live":cpu::CATEGORIES.map(allocation::live),"cpu_peak":cpu::CATEGORIES.map(allocation::peak),"cpu_allocations":cpu::CATEGORIES.map(allocation::count),
            "gpu_objects":memory.object_counts,"gpu_objects_peak":memory.object_peaks,"audit_entries":memory.audit_entries,"gpu_live":memory.live_bytes,"gpu_peak":memory.peak_bytes,"gpu_allocations":memory.live_allocations,"gpu_allocations_peak":memory.peak_allocations,
            "metadata_entries":view.volumes().len(),"material_count":view.materials().len(),"source_recipe_count":super::streamed_world::FIXTURE_RECIPE_COUNT,
            "live_historical_views":self.history.len(),"peak_historical_views":self.history_peak,"generating":cache.generating,"live_versions":edits.live_versions,
            "required_revision":diagnostics.presenting().required_revision().to_string(),"visible_revision":diagnostics.presenting().visible_revision().to_string(),"fully_converged":diagnostics.presenting().is_fully_converged(),
            "edited_coordinates":edits.current_coordinates,"history_entries":edits.retained_entries,
            "coverage_stalls":diagnostics.coverage_stalls(),"presentation_images":self.backend.qualification_presentation_image_count()}),
        )
    }
    fn request(
        &mut self,
        center: (u32, u32),
        switch: bool,
    ) -> Result<VoxelResidencySelection, String> {
        let view = self
            .frontend
            .scene_view()
            .map_err(|error| error.to_string())?;
        let selection = cpu::selection(&view, self.next_selection, center, self.side)?;
        self.next_selection += 1;
        self.frontend
            .require_residency(selection.clone())
            .map_err(|error| error.to_string())?;
        self.frontend
            .establish_residency()
            .map_err(|error| error.to_string())?;
        self.backend
            .submit_residency_selection(selection.clone())
            .map_err(|error| error.to_string())?;
        if switch {
            let raster = self.diagnostics()?.presenting().strategy() != RASTER_STRATEGY;
            let (path, probes) = build(
                &self.frontend,
                selection.clone(),
                raster,
                self.camera,
                self.camera_revision,
                self.next_owner,
            )?;
            self.next_owner += 1;
            self.path_observation = path.observation.clone();
            self.observations.push(path.observation.clone());
            self.backend
                .request_render_path_switch(Box::new(path))
                .map_err(|error| error.to_string())?;
            self.replacement = Some(probes);
        }
        Ok(selection)
    }
    fn converged(&mut self, selection: &VoxelResidencySelection) -> Result<bool, String> {
        let diagnostics = self.diagnostics()?;
        let stamp = diagnostics.presenting();
        let converged = stamp.is_fully_converged()
            && stamp.installed_selection() == Some(selection.identity())
            && stamp.visible_revision().to_string() == self.oracle.revision.to_string()
            && diagnostics.roles().replacement().is_none();
        if converged {
            if let Some(probes) = self.replacement.take() {
                self.probes = probes;
            }
            self.installed = selection.clone();
        }
        Ok(converged)
    }
    fn wait(
        &mut self,
        output: &mut File,
        selection: &VoxelResidencySelection,
        origin: Instant,
    ) -> Result<Instant, String> {
        loop {
            self.draw()?;
            self.sample(output, "pre-retirement")?;
            if self.converged(selection)? {
                let at = self
                    .boundary
                    .borrow()
                    .at
                    .ok_or("missing fence-safe callback")?;
                if at.duration_since(origin).as_secs_f64() > 10.0 {
                    return Err("production edit-replay preparation timed out".into());
                }
                self.draw()?;
                return Ok(at);
            }
            if origin.elapsed().as_secs_f64() > 10.0 {
                return Err("production edit-replay preparation timed out".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn transition(
        &mut self,
        output: &mut File,
        center: (u32, u32),
        switch: bool,
    ) -> Result<(), String> {
        let origin = Instant::now();
        let selection = self.request(center, switch)?;
        self.wait(output, &selection, origin)?;
        Ok(())
    }
    fn camera(&mut self, camera: CameraState) -> Result<(), String> {
        self.camera_revision = self
            .camera_revision
            .checked_successor()
            .ok_or("camera revision overflow")?;
        self.backend
            .publish_camera_state(camera, self.camera_revision)
            .map_err(|error| error.to_string())?;
        self.camera = camera;
        Ok(())
    }
    fn covered(&self) -> Result<bool, String> {
        let view = self
            .frontend
            .scene_view()
            .map_err(|error| error.to_string())?;
        RenderPathCoverage::new(&view, self.installed.clone())
            .map_err(|error| error.to_string())?
            .contains_camera(self.camera, [1920, 1080])
            .map_err(|error| error.to_string())
    }
    fn verify(
        &mut self,
        output: &mut File,
        lap: Option<u32>,
        index: Option<usize>,
    ) -> Result<(), String> {
        if !self.covered()? {
            return Err("oracle probes requested outside installed coverage".into());
        }
        let keys = self
            .installed
            .volumes()
            .iter()
            .map(|identity| {
                let metadata = self
                    .frontend
                    .scene_view()
                    .map_err(|error| error.to_string())?;
                let volume = metadata
                    .volumes()
                    .iter()
                    .find(|volume| volume.identity() == identity)
                    .ok_or("unknown installed volume")?;
                let [x, _, z] = volume.scene_origin();
                Ok(cpu::Key {
                    coordinate: (x as u32 / 64, z as u32 / 64),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if !route::covered(self.camera, &keys, self.side) {
            return Err("frozen coverage rule failed".into());
        }
        let expected = self.probes.request_probes(&self.oracle, self.camera)?;
        let mut observed = 0;
        let deadline = Instant::now() + Duration::from_secs(10);
        while observed < expected.len() {
            self.draw()?;
            observed += self.probes.verify_probes(&expected)?;
            if Instant::now() > deadline {
                return Err("rendered probes timed out".into());
            }
        }
        cpu::emit(
            output,
            json!({"kind":"probes","lap":lap,"index":index,"count":observed,"coverage_contains_view":true,"coverage_contains_ray_domain":true,"matching":true,"revision":self.oracle.revision}),
        )
    }
    fn edit(&mut self, coordinate: (u32, u32), restore: bool) -> Result<(), String> {
        let outcome = cpu::edit(&self.frontend, coordinate, restore)?;
        self.oracle.edit(coordinate, restore);
        let view = self
            .frontend
            .scene_view()
            .map_err(|error| error.to_string())?;
        let selection = view
            .residency_selection(
                VoxelResidencySelectionId::new(self.next_selection),
                self.installed.volumes().to_vec(),
            )
            .map_err(|error| error.to_string())?;
        self.next_selection += 1;
        cpu::establish(&self.frontend, selection.clone())?;
        self.backend
            .submit_edit_outcome(outcome)
            .map_err(|error| error.to_string())?;
        self.backend
            .submit_residency_selection(selection)
            .map_err(|error| error.to_string())?;
        self.draw()
    }
    fn stress(&mut self, output: &mut File, initial_raster: bool) -> Result<(), String> {
        let original = self
            .frontend
            .scene_view()
            .map_err(|error| error.to_string())?;
        let predecessor = original.revision().to_string();
        self.history.push(original);
        self.history_peak = self.history_peak.max(self.history.len());
        let mut steps = vec!["retain-generated"];
        self.edit((3, 3), false)?;
        steps.push("edit-3-3");
        let edited = self
            .frontend
            .scene_view()
            .map_err(|error| error.to_string())?;
        self.history.push(edited);
        self.history_peak = self.history_peak.max(self.history.len());
        steps.push("retain-edited");
        self.transition(output, (3, 3), false)?;
        self.sample(output, "revision-replacement")?;
        self.verify(output, None, None)?;
        cpu::emit(
            output,
            json!({"kind":"revision-replacement","predecessor":predecessor,"successor":self.oracle.revision.to_string()}),
        )?;
        self.transition(output, (4, 3), false)?;
        steps.push("evict-2-2");
        let version = self
            .history
            .get(1)
            .expect("the edited historical view was just retained")
            .volume_content_version(&recipe::volume_identity(3, 3))
            .map_err(|error| error.to_string())?;
        self.edit((2, 2), false)?;
        steps.push("edit-2-2-nonresident");
        let current = self
            .frontend
            .scene_view()
            .map_err(|error| error.to_string())?;
        let reuse = current
            .volume_content_version(&recipe::volume_identity(3, 3))
            .map_err(|error| error.to_string())?
            == version;
        self.transition(output, (4, 3), false)?;
        self.transition(output, (3, 3), false)?;
        steps.push("reload-2-2");
        cpu::record_fingerprint(
            output,
            self.history
                .first()
                .expect("the generated historical view remains retained"),
            (3, 3),
            "generated",
        )?;
        steps.push("historical-generated");
        cpu::record_fingerprint(
            output,
            self.history
                .get(1)
                .expect("the edited historical view remains retained"),
            (3, 3),
            "edited",
        )?;
        steps.push("historical-edited");
        cpu::record_fingerprint(output, &current, (2, 2), "edited")?;
        drop(current);
        self.edit((2, 2), true)?;
        steps.push("restore-2-2");
        self.edit((3, 3), true)?;
        steps.push("restore-3-3");
        self.history.clear();
        steps.push("drop-history");
        self.transition(output, (3, 3), false)?;
        let restored = self
            .frontend
            .scene_view()
            .map_err(|error| error.to_string())?;
        cpu::record_fingerprint(output, &restored, (2, 2), "restored")?;
        let edits = self
            .frontend
            .streamed_edit_statistics()
            .map_err(|error| error.to_string())?
            .ok_or("missing edits")?;
        steps.push("compact");
        cpu::emit(
            output,
            json!({"kind":"compaction","edit_script":steps,"live_historical_views":self.history.len(),"edited_coordinates":edits.current_coordinates,"history_entries":edits.retained_entries,"unchanged_volume_reused":reuse}),
        )?;
        let newest = cpu::selection(&restored, self.next_selection, (12, 12), self.side)?;
        let copies = self
            .frontend
            .materialize_residency(&newest, &restored)
            .map_err(|error| error.to_string())?;
        let query = restored
            .enumerate_cells(&recipe::volume_identity(15, 15), 16, 64)
            .map_err(|error| error.to_string())?;
        let stats = self
            .frontend
            .materialization_cache_stats()
            .map_err(|error| error.to_string())?;
        let rejected = restored
            .enumerate_cells(&recipe::volume_identity(14, 15), 16, 64)
            .is_err();
        self.sample(output, "disjoint-query-overlap")?;
        cpu::emit(
            output,
            json!({"kind":"admission","copies":stats.copies,"second_query_rejected":rejected}),
        )?;
        drop(query);
        drop(copies);
        if self.diagnostics()?.presenting().strategy() != RASTER_STRATEGY {
            self.transition(output, (3, 3), true)?;
        }
        let before = self.diagnostics()?.presenting().clone();
        self.path_observation.borrow_mut().fail_upload = true;
        let selection = self.request((4, 3), false)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.path_observation.borrow().upload_failed {
            self.draw()?;
            if Instant::now() > deadline {
                return Err("upload failure was not observed".into());
            }
        }
        let after = self.diagnostics()?.presenting().clone();
        let preserved = before.visible_revision() == after.visible_revision()
            && before.installed_selection() == after.installed_selection();
        cpu::emit(
            output,
            json!({"kind":"failure-recovery","phase":"raster-upload","presenting_preserved":preserved,"observed_upload_failure":true}),
        )?;
        self.sample(output, "failed-candidate-cleaned")?;
        self.path_observation.borrow_mut().retry = true;
        self.wait(output, &selection, Instant::now())?;
        for index in 0..12 {
            let x = if index % 2 == 0 { 256.5 } else { 255.5 };
            self.camera(
                CameraState::new(
                    [x, 48.0, 224.0],
                    [x + 16.0, 24.0, 224.0],
                    [0.0, 1.0, 0.0],
                    60.0,
                    0.1,
                    34.0,
                )
                .map_err(|error| error.to_string())?,
            )?;
            self.transition(output, route::center(self.camera, self.side), false)?;
        }
        self.camera(route::camera(0.0)?)?;
        self.transition(output, (3, 3), !initial_raster)?;
        cpu::emit(
            output,
            json!({"kind":"boundary-churn","crossings":12,"installations":12,"hysteresis":false,"coverage_stalls":self.diagnostics()?.coverage_stalls()}),
        )?;
        self.sample(output, "stress-settled")
    }
    fn travel(&mut self, output: &mut File, lap: u32) -> Result<(), String> {
        let start = Instant::now();
        let mut next = 0;
        let mut installed = 0;
        let mut pending: Option<(VoxelResidencySelection, Instant, Option<Instant>, String)> = None;
        let mut probes = 0;
        let mut logged = 0;
        let mut frames = 0;
        while start.elapsed().as_secs_f64() < route::DURATION {
            let seconds = start.elapsed().as_secs_f64();
            self.camera(route::camera(seconds)?)?;
            if route::crossing_due(seconds, next, pending.as_ref().map(|_| next - 1))? {
                let crossing = *route::CROSSINGS.get(next).ok_or("invalid crossing index")?;
                let origin = start + Duration::from_secs_f64(crossing);
                let switch = next == 2 || next == 5;
                let switch_at = switch.then(Instant::now);
                let from = self
                    .diagnostics()?
                    .presenting()
                    .strategy()
                    .identifier()
                    .to_string();
                let target = self.request(route::center(self.camera, self.side), switch)?;
                cpu::emit(
                    output,
                    json!({"kind":"crossing","lap":lap,"index":next,"origin_seconds":crossing,"unresolved_origin_seconds":crossing,"switch_requested_seconds":switch_at.map(|at|at.duration_since(start).as_secs_f64()),"switch_requested":switch}),
                )?;
                pending = Some((target, origin, switch_at, from));
                next += 1;
            }
            self.draw()?;
            frames += 1;
            if let Some((target, origin, switch, from)) = &pending
                && self.converged(target)?
            {
                let at = self
                    .boundary
                    .borrow()
                    .at
                    .ok_or("missing actual fence-safe callback")?;
                let crossing_seconds = at.duration_since(*origin).as_secs_f64();
                let switch_seconds =
                    switch.map(|switch| at.duration_since(switch.max(*origin)).as_secs_f64());
                if crossing_seconds > 2.5 {
                    return Err("production crossing exceeded 2.5 seconds; reopen shaping".into());
                }
                let to = self
                    .diagnostics()?
                    .presenting()
                    .strategy()
                    .identifier()
                    .to_string();
                cpu::emit(
                    output,
                    json!({"kind":"installed","lap":lap,"index":next-1,"crossing_seconds":crossing_seconds,"switch_seconds":switch_seconds,"boundary_seconds":at.duration_since(start).as_secs_f64(),"fence_safe":true,"selection_matches":true,"from":from,"to":to}),
                )?;
                self.draw()?;
                self.sample(output, "crossing-settled")?;
                self.verify(output, Some(lap), Some(next - 1))?;
                probes += 4;
                installed += 1;
                pending = None;
            }
            let whole = seconds as u64;
            if whole > logged {
                self.sample(output, "travel")?;
                logged = whole;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        if pending.is_some() || next != 8 || installed != 8 {
            return Err("route ended with outstanding demand".into());
        }
        self.sample(output, "lap-settled")?;
        cpu::emit(
            output,
            json!({"kind":"route-result","lap":lap,"frames":frames,"crossings":next,"installed_crossings":installed,"coverage_stalls":self.diagnostics()?.coverage_stalls(),"rendered_probes":probes}),
        )
    }
}
fn run(window: &Window, output: &mut File, mode: &str) -> Result<(), String> {
    let mut runner = Runner::new(window, mode)?;
    let context = runner.backend.runtime_context();
    cpu::emit(
        output,
        json!({"kind":"device","name":context.device_name,"driver_version":context.driver_version,"api_version":context.api_version,"validation_enabled":context.validation_enabled}),
    )?;
    cpu::emit(
        output,
        json!({"kind":"context","mode":mode,"projection":[1920,1080,60.0,0.1,34.0],"crossings":route::CROSSINGS,"production":true,"typed_dense_rejection":true,"route_start":mode}),
    )?;
    runner.draw()?;
    runner.draw()?;
    runner.sample(output, "initial")?;
    if mode == "raster" || mode == "brickmap" {
        runner.stress(output, mode == "raster")?;
        for lap in 0..2 {
            runner.travel(output, lap)?;
        }
    } else {
        runner.edit((3, 3), false)?;
        runner.transition(output, (3, 3), true)?;
        runner.sample(output, "matched-edited")?;
        runner.verify(output, None, None)?;
    }
    runner
        .backend
        .shutdown()
        .map_err(|error| error.to_string())?;
    let warnings = runner.backend.validation_warning_count();
    let errors = runner.backend.validation_error_count();
    cpu::emit(
        output,
        json!({"kind":"validation","warnings":warnings,"errors":errors}),
    )?;
    let memory = runner.gpu.snapshot().map_err(|error| error.to_string())?;
    let workers = runner.path_observation.borrow().workers;
    let Runner {
        gpu,
        backend,
        frontend,
        probes,
        replacement,
        installed,
        oracle,
        ..
    } = runner;
    drop(backend);
    drop(frontend);
    drop(probes);
    drop(replacement);
    drop(installed);
    drop(oracle);
    cpu::released(output)?;
    cpu::emit(
        output,
        json!({"kind":"released","gpu_live":memory.live_bytes,"gpu_allocations":memory.live_allocations,"gpu_objects":memory.object_counts,"workers":workers}),
    )?;
    drop(gpu);
    Ok(())
}
struct Application {
    output: File,
    mode: String,
    result: Option<Result<(), String>>,
}
impl ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some((|| {
            let window = event_loop
                .create_window(
                    Window::default_attributes()
                        .with_visible(false)
                        .with_decorations(false)
                        .with_inner_size(winit::dpi::PhysicalSize::new(1920, 1080)),
                )
                .map_err(|error| error.to_string())?;
            windows_adapter::set_measurement_extent(&window, EXTENT)
                .map_err(|error| error.to_string())?;
            run(&window, &mut self.output, &self.mode)
        })());
        event_loop.exit();
    }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}
pub fn main() -> Result<(), String> {
    // The standard library caches blocking-channel state until this thread exits.
    // Initialize it as Control so a first Raster wait cannot look like leaked geometry.
    let (sender, receiver) = std::sync::mpsc::sync_channel::<()>(0);
    match receiver.recv_timeout(Duration::from_millis(1)) {
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        result => return Err(format!("channel context initialization failed: {result:?}")),
    }
    drop((sender, receiver));
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let [mode, path, allow] = arguments.as_slice() else {
        return Err("usage: streamed-residency-qualification raster|brickmap|matched-8|matched-16 OUTPUT.jsonl --allow-gpu".into());
    };
    if allow != "--allow-gpu"
        || !["raster", "brickmap", "matched-8", "matched-16"].contains(&mode.as_str())
    {
        return Err("invalid GPU mode or opt-in".into());
    }
    let mut application = Application {
        output: File::create(path).map_err(|error| error.to_string())?,
        mode: mode.clone(),
        result: None,
    };
    EventLoop::new()
        .map_err(|error| error.to_string())?
        .run_app(&mut application)
        .map_err(|error| error.to_string())?;
    application.result.ok_or("GPU qualification did not run")?
}
