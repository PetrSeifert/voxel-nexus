use super::{
    allocation::{self, Category},
    streamed_residency_source::{Cache, Key, Snapshot},
};
use ash::vk;
use compute_ray_render_path::{
    ComputeRayRenderPathAdapter, ComputeRepresentation, ComputeSceneBundle,
    ComputeSemanticRayController, camera_semantic_ray,
};
use raster_render_path::{
    RasterArtifact, RasterRenderPathAdapter, RasterSemanticFaceController, derive_raster_regions,
};
use render_backend::{
    CameraState, CameraStateRevision, GpuAllocationClass, RenderPath, RenderPathDeviceContext,
    RenderPathFrameContext, RenderPathReadiness, RenderPathResult, RenderPathTarget,
    SwitchableRenderPath, with_gpu_allocation_owner,
};
use semantic_ray_oracle::{
    SemanticRayDistanceTolerance, SemanticRayProbe, SemanticRayProbeObservation,
    observe_probe_along_ray,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant};
use voxel_frontend::VoxelExtent;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Raster,
    Brickmap,
}
impl Kind {
    pub fn other(self) -> Self {
        match self {
            Self::Raster => Self::Brickmap,
            Self::Brickmap => Self::Raster,
        }
    }
    fn category(self) -> Category {
        match self {
            Self::Raster => Category::Raster,
            Self::Brickmap => Category::Brickmap,
        }
    }
    fn gpu_class(self) -> GpuAllocationClass {
        match self {
            Self::Raster => GpuAllocationClass::Raster,
            Self::Brickmap => GpuAllocationClass::Brickmap,
        }
    }
}
enum Probes {
    Raster(RasterSemanticFaceController),
    Brickmap(ComputeSemanticRayController),
}
pub struct Path {
    pub kind: Kind,
    pub selection: Vec<Key>,
    identity: u64,
    scene_bytes: u64,
    adapter: Box<dyn SwitchableRenderPath>,
    probes: Probes,
}
pub struct Artifacts {
    raster: HashMap<Key, RasterArtifact>,
    next_owner: u64,
    pub fail_next_upload: bool,
}
impl Artifacts {
    pub fn new() -> Self {
        Self {
            raster: HashMap::new(),
            next_owner: 1,
            fail_next_upload: false,
        }
    }
    pub fn retain(&mut self, installed: &[Key], newest: &[Key]) {
        self.raster
            .retain(|key, _| installed.contains(key) || newest.contains(key));
    }
    pub fn count(&self) -> usize {
        self.raster.len()
    }
    pub fn build(
        &mut self,
        kind: Kind,
        cache: &Cache,
        source: &Snapshot,
        selection: &[Key],
        camera: CameraState,
        camera_revision: CameraStateRevision,
    ) -> Result<Path, String> {
        self.next_owner += 1;
        let identity = self.next_owner;
        allocation::within(kind.category(), || {
            let view = cache.assemble(source, selection)?;
            let (adapter, probes, scene_bytes): (Box<dyn SwitchableRenderPath>, Probes, u64) =
                match kind {
                    Kind::Raster => {
                        let mut selected = Vec::new();
                        for key in selection {
                            if !self.raster.contains_key(key) {
                                let volume = cache.assemble(source, std::slice::from_ref(key))?;
                                let artifact =
                                    derive_raster_regions(&volume, VoxelExtent::new(16, 16, 16))
                                        .map_err(|error| error.to_string())?;
                                self.raster.insert(key.clone(), artifact);
                            }
                            selected.push(
                                self.raster
                                    .get(key)
                                    .expect("a selected artifact was inserted above")
                                    .clone(),
                            );
                        }
                        let artifact = RasterArtifact::qualification_assemble(&view, &selected)
                            .map_err(|error| error.to_string())?;
                        let (mut adapter, installer, _) =
                            RasterRenderPathAdapter::awaiting_artifact_with_camera_control(
                                camera,
                                camera_revision,
                                view.scene_id().clone(),
                                view.revision(),
                            );
                        installer
                            .publish_complete(artifact)
                            .map_err(|error| error.to_string())?;
                        if self.fail_next_upload {
                            installer
                                .inject_next_upload_failure()
                                .map_err(|error| error.to_string())?;
                            self.fail_next_upload = false;
                        }
                        let probes = adapter.enable_semantic_face_observation();
                        (Box::new(adapter), Probes::Raster(probes), 0)
                    }
                    Kind::Brickmap => {
                        let bundle = ComputeSceneBundle::qualification_streamed(
                            &view,
                            ComputeRepresentation::Brickmap {
                                budget_bytes: 1 << 30,
                            },
                        )
                        .map_err(|error| error.to_string())?;
                        let scene_bytes = bundle.storage_words().len() as u64 * 4;
                        let mut adapter = ComputeRayRenderPathAdapter::qualification_from_bundle(
                            bundle,
                            camera,
                            camera_revision,
                        );
                        let probes = adapter.enable_semantic_ray_observation();
                        (Box::new(adapter), Probes::Brickmap(probes), scene_bytes)
                    }
                };
            Ok(Path {
                kind,
                selection: selection.to_vec(),
                identity,
                scene_bytes,
                adapter,
                probes,
            })
        })
    }
}
impl Path {
    fn operation<T>(
        &mut self,
        operation: impl FnOnce(&mut dyn SwitchableRenderPath) -> RenderPathResult<T>,
    ) -> RenderPathResult<T> {
        allocation::within(self.kind.category(), || {
            with_gpu_allocation_owner(
                self.identity,
                self.kind.gpu_class(),
                self.scene_bytes,
                || operation(self.adapter.as_mut()),
            )
        })
    }
    pub fn request_probes(
        &self,
        source: &Snapshot,
        camera: CameraState,
    ) -> Result<Vec<SemanticRayProbeObservation>, String> {
        let oracle = source.oracle_view()?;
        let mut observations = Vec::new();
        let mut probes = Vec::new();
        for pixel in [[960, 540], [480, 810], [1440, 810], [960, 1000]] {
            let ray = camera_semantic_ray(
                camera,
                vk::Extent2D {
                    width: 1920,
                    height: 1080,
                },
                pixel,
            )
            .map_err(|error| error.to_string())?;
            let probe = SemanticRayProbe::new(format!("pixel-{}-{}", pixel[0], pixel[1]), ray)
                .map_err(|error| error.to_string())?;
            let observation =
                observe_probe_along_ray(&oracle, &probe).map_err(|error| error.to_string())?;
            observations.push(observation);
            probes.push(probe);
        }
        match &self.probes {
            Probes::Raster(controller) => controller
                .request(observations.clone())
                .map_err(|error| error.to_string())?,
            Probes::Brickmap(controller) => controller
                .request(probes)
                .map_err(|error| error.to_string())?,
        }
        Ok(observations)
    }
    pub fn verify_probes(&self, expected: &[SemanticRayProbeObservation]) -> Result<usize, String> {
        match &self.probes {
            Probes::Raster(controller) => {
                let actual = controller.drain().map_err(|error| error.to_string())?;
                if actual.iter().any(|entry| {
                    !entry.passed()
                        || !expected.iter().any(|expected| {
                            expected.probe_identity() == entry.probe_identity()
                                && expected.observation().revision() == entry.revision()
                        })
                }) {
                    return Err(
                        "presented raster faces disagree with the whole-scene oracle".into(),
                    );
                }
                Ok(actual.len())
            }
            Probes::Brickmap(controller) => {
                let actual = controller.drain().map_err(|error| error.to_string())?;
                let tolerance =
                    SemanticRayDistanceTolerance::new(0.002).map_err(|error| error.to_string())?;
                if actual.iter().any(|entry| {
                    !expected.iter().any(|expected| {
                        expected.probe_identity() == entry.probe_identity()
                            && entry
                                .observation()
                                .agrees_with(expected.observation(), tolerance)
                    })
                }) {
                    return Err(
                        "rendered Brickmap probes disagree with the whole-scene oracle".into(),
                    );
                }
                Ok(actual.len())
            }
        }
    }
}
pub struct Install {
    pub candidate: Path,
    pub replacement: Option<Path>,
    pub crossing: Instant,
    pub switch_requested: Option<Instant>,
}
impl Install {
    fn prepare(
        &mut self,
        mut configure: impl FnMut(&mut Path) -> RenderPathResult<()>,
        mut shutdown: impl FnMut(&mut Path) -> RenderPathResult<()>,
    ) -> RenderPathResult<()> {
        let prepared = (|| {
            configure(&mut self.candidate)?;
            if let Some(replacement) = &mut self.replacement {
                configure(replacement)?;
                let active = self.candidate.adapter.stamp();
                let ready = replacement.adapter.stamp();
                if self.candidate.selection != replacement.selection
                    || active.scene_identity() != ready.scene_identity()
                    || active.visible_revision() != ready.visible_revision()
                    || ready.required_revision() != ready.visible_revision()
                    || active.camera_state_revision() != ready.camera_state_revision()
                    || active.presentation_configuration() != ready.presentation_configuration()
                    || ready.readiness() != RenderPathReadiness::Recordable
                {
                    return Err(
                        "replacement selection/revision/camera/configuration/readiness mismatch"
                            .into(),
                    );
                }
            }
            Ok(())
        })();
        if prepared.is_err() {
            // Both prepared owners need cleanup even if one shutdown reports an error.
            let candidate_cleanup = shutdown(&mut self.candidate);
            let replacement_cleanup = self.replacement.as_mut().map(shutdown).transpose();
            candidate_cleanup?;
            replacement_cleanup?;
        }
        prepared
    }
}
pub struct Boundary {
    pub presenting: Path,
    retiring: Vec<Path>,
    pub pending: Option<Install>,
    pub installed_at: Option<Instant>,
    pub handoff_at: Option<Instant>,
    pub crossing_seconds: f64,
    pub switch_seconds: f64,
    pub peak_owners: usize,
    pub installed_targets: u64,
    pub retired_owners: u64,
    pub camera: CameraState,
    pub camera_revision: CameraStateRevision,
    pub expected_failure: bool,
    pub failure: Option<String>,
}
impl Boundary {
    pub fn new(presenting: Path, camera: CameraState) -> Self {
        Self {
            presenting,
            retiring: Vec::new(),
            pending: None,
            installed_at: None,
            handoff_at: None,
            crossing_seconds: 0.0,
            switch_seconds: 0.0,
            peak_owners: 1,
            installed_targets: 0,
            retired_owners: 0,
            camera,
            camera_revision: CameraStateRevision::new(1),
            expected_failure: false,
            failure: None,
        }
    }
    pub fn owners(&self) -> usize {
        1 + self.retiring.len()
            + self
                .pending
                .as_ref()
                .map_or(0, |install| 1 + usize::from(install.replacement.is_some()))
    }
}
pub struct Shared(pub Rc<RefCell<Boundary>>);
impl RenderPath for Shared {
    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.0
            .borrow_mut()
            .presenting
            .operation(|path| path.release(device))
    }
    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        self.0
            .borrow_mut()
            .presenting
            .operation(|path| path.configure(device, target))
    }
    fn advance_frame_boundary(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        let mut state = self.0.borrow_mut();
        let retired_count = state.retiring.len();
        for mut retired in state.retiring.drain(..) {
            retired.operation(|path| path.shutdown(device))?;
        }
        state.retired_owners += retired_count as u64;
        let camera = state.camera;
        let revision = state.camera_revision;
        state.presenting.operation(|path| {
            path.publish_camera_state(camera, revision)?;
            path.advance_frame_boundary(device, target)
        })?;
        if let Some(mut install) = state.pending.take() {
            state.peak_owners = state
                .peak_owners
                .max(2 + usize::from(install.replacement.is_some()));
            let configured = install.prepare(
                |prepared| {
                    prepared.operation(|path| {
                        path.configure(device, target)?;
                        path.publish_camera_state(camera, revision)?;
                        path.advance_frame_boundary(device, target)
                    })
                },
                |prepared| prepared.operation(|path| path.shutdown(device)),
            );
            if let Err(error) = configured {
                state.failure = Some(error.to_string());
                if !std::mem::take(&mut state.expected_failure) {
                    return Err(error);
                }
                return Ok(());
            }
            let old = std::mem::replace(&mut state.presenting, install.candidate);
            state.retiring.push(old);
            let installed_at = Instant::now();
            state.crossing_seconds = installed_at.duration_since(install.crossing).as_secs_f64();
            state.installed_at = Some(installed_at);
            state.installed_targets += 1;
            if let Some(replacement) = install.replacement {
                let old = std::mem::replace(&mut state.presenting, replacement);
                state.retiring.push(old);
                let handoff_at = Instant::now();
                state.switch_seconds = handoff_at
                    .duration_since(
                        install
                            .switch_requested
                            .unwrap_or(install.crossing)
                            .max(install.crossing),
                    )
                    .as_secs_f64();
                state.handoff_at = Some(handoff_at);
            }
        }
        Ok(())
    }
    fn shutdown(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        let mut state = self.0.borrow_mut();
        state.presenting.operation(|path| path.shutdown(device))?;
        for mut retired in state.retiring.drain(..) {
            retired.operation(|path| path.shutdown(device))?;
        }
        if let Some(mut install) = state.pending.take() {
            install.candidate.operation(|path| path.shutdown(device))?;
            if let Some(mut replacement) = install.replacement {
                replacement.operation(|path| path.shutdown(device))?;
            }
        }
        Ok(())
    }
    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        self.0
            .borrow_mut()
            .presenting
            .operation(|path| path.record(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::{Artifacts, Cache, Install, Kind, Snapshot};
    use crate::streamed_residency_route;
    use render_backend::CameraStateRevision;
    use std::{cell::Cell, time::Instant};

    fn installation() -> Install {
        let source = Snapshot::new(16);
        let mut cache = Cache::new();
        let selection = vec![super::Key {
            coordinate: (3, 3),
            version: source.version((3, 3)),
        }];
        cache.ensure(&source, selection.first().unwrap()).unwrap();
        let camera = streamed_residency_route::camera(0.0).unwrap();
        let mut artifacts = Artifacts::new();
        let mut build = |kind| {
            artifacts
                .build(
                    kind,
                    &cache,
                    &source,
                    &selection,
                    camera,
                    CameraStateRevision::new(1),
                )
                .unwrap()
        };
        Install {
            candidate: build(Kind::Raster),
            replacement: Some(build(Kind::Brickmap)),
            crossing: Instant::now(),
            switch_requested: None,
        }
    }

    #[test]
    fn handoff_mismatch_cleans_both_prepared_owners() {
        let mut install = installation();
        install.replacement.as_mut().unwrap().selection.clear();
        let live = Cell::new(0);
        let error = install
            .prepare(
                |_| {
                    live.set(live.get() + 1);
                    Ok(())
                },
                |_| {
                    live.set(live.get() - 1);
                    Ok(())
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("mismatch"));
        assert_eq!(live.get(), 0);
    }

    #[test]
    fn failed_cleanup_does_not_skip_the_other_owner() {
        let mut install = installation();
        let cleaned = Cell::new(0);
        let error = install
            .prepare(
                |_| Ok(()),
                |_| {
                    cleaned.set(cleaned.get() + 1);
                    if cleaned.get() == 1 {
                        Err("candidate cleanup failed".into())
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "candidate cleanup failed");
        assert_eq!(cleaned.get(), 2);
    }

    #[test]
    fn successful_preparation_keeps_candidate_resources() {
        let mut install = installation();
        install.replacement = None;
        let live = Cell::new(0);
        install
            .prepare(
                |_| {
                    live.set(live.get() + 1);
                    Ok(())
                },
                |_| {
                    live.set(live.get() - 1);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(live.get(), 1);
    }
}
