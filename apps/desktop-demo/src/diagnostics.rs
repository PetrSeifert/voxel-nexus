use canonical_scene::{CanonicalSceneScale, generate_canonical_scene};
use raster_render_path::{
    RasterArtifactInstallationError, RasterArtifactInstallationPhase, RasterArtifactPreparation,
    RasterArtifactPreparationEvent,
};
use render_backend::{
    DeviceCandidate, QueueFamilyCapabilities, RenderPathPhase, run_render_path_phase,
};
use std::sync::mpsc;
use std::{error::Error, fmt};
use voxel_frontend::{VoxelFrontend, VoxelSceneRevision};

pub(super) fn background_preparation_failure_diagnostic(
    mut arguments: impl Iterator<Item = String>,
) -> Option<Result<(), String>> {
    if arguments.next().as_deref() != Some("--verify-background-preparation-failure") {
        return None;
    }
    let result = match arguments.next().as_deref() {
        Some("derivation") => (|| {
            let canonical =
                generate_canonical_scene(CanonicalSceneScale::Small).map_err(|error| {
                    format!("could not generate the diagnostic Voxel Scene: {error}")
                })?;
            let view = VoxelFrontend::new()
                .publish_sparse(canonical.into_scene())
                .map_err(|error| {
                    format!("could not publish the diagnostic Voxel Scene: {error}")
                })?;
            let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
            let mut preparation = RasterArtifactPreparation::start(
                view,
                voxel_frontend::VoxelVolumeId::new("injected-missing-volume"),
                move |event| {
                    if matches!(event, RasterArtifactPreparationEvent::Completed { .. })
                        && completion_sender.send(()).is_err()
                    {
                        eprintln!("background preparation diagnostic receiver closed");
                    }
                },
            )
            .map_err(|error| error.to_string())?;
            completion_receiver
                .recv()
                .map_err(|error| format!("background preparation diagnostic hung: {error}"))?;
            match preparation.try_complete() {
                Err(error) => Err(error.to_string()),
                Ok(Some(_)) => Err(
                    "the injected background derivation failure produced an artifact".to_owned(),
                ),
                Ok(None) => {
                    Err("the injected background derivation failure did not complete".to_owned())
                }
            }
        })(),
        Some(case) => Err(format!(
            "unknown background-preparation diagnostic {case:?}; expected derivation"
        )),
        None => Err("missing background-preparation diagnostic; expected derivation".to_owned()),
    };
    Some(result)
}

#[derive(Debug)]
pub(super) struct InjectedRenderPathFailure;

impl fmt::Display for InjectedRenderPathFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("injected proof failure")
    }
}

impl Error for InjectedRenderPathFailure {}

pub(super) fn render_path_failure_diagnostic(
    mut arguments: impl Iterator<Item = String>,
) -> Option<Result<(), String>> {
    if arguments.next().as_deref() != Some("--verify-render-path-failure") {
        return None;
    }
    let phase = match arguments.next().as_deref() {
        Some("release") => Ok(RenderPathPhase::Release),
        Some("configure") => Ok(RenderPathPhase::Configure),
        Some("advance-frame-boundary") => Ok(RenderPathPhase::AdvanceFrameBoundary),
        Some("record") => Ok(RenderPathPhase::Record),
        Some("shutdown") => Ok(RenderPathPhase::Shutdown),
        Some("upload") => {
            return Some(Err(RasterArtifactInstallationError::new(
                RasterArtifactInstallationPhase::Upload,
                VoxelSceneRevision::new(41),
                Box::new(InjectedRenderPathFailure),
            )
            .to_string()));
        }
        Some(phase) => Err(format!(
            "unknown Render Path phase {phase:?}; expected release, configure, advance-frame-boundary, record, shutdown, or upload"
        )),
        None => Err(
            "missing Render Path phase; expected release, configure, advance-frame-boundary, record, shutdown, or upload"
                .to_owned(),
        ),
    };
    Some(phase.and_then(|phase| {
        run_render_path_phase::<()>(phase, || Err(Box::new(InjectedRenderPathFailure)))
            .map_err(|error| error.to_string())
    }))
}

pub(super) fn unsupported_prerequisite_diagnostic(
    mut arguments: impl Iterator<Item = String>,
) -> Option<Result<(), String>> {
    if arguments.next().as_deref() != Some("--verify-unsupported-prerequisite") {
        return None;
    }
    let result = match arguments.next().as_deref() {
        Some("vulkan-1.2") => verify_rejected_candidate(DeviceCandidate {
            name: "Deterministic Vulkan 1.2 device".to_owned(),
            api_version: ash::vk::make_api_version(0, 1, 2, 0),
            driver_version: 1,
            supports_swapchain: true,
            has_surface_formats: true,
            has_present_modes: true,
            queue_families: vec![QueueFamilyCapabilities {
                supports_graphics: true,
                supports_compute: true,
                supports_presentation: true,
            }],
        }),
        Some("presentation") => verify_rejected_candidate(DeviceCandidate {
            name: "Deterministic device without presentation".to_owned(),
            api_version: ash::vk::API_VERSION_1_3,
            driver_version: 1,
            supports_swapchain: false,
            has_surface_formats: false,
            has_present_modes: false,
            queue_families: vec![QueueFamilyCapabilities {
                supports_graphics: true,
                supports_compute: true,
                supports_presentation: false,
            }],
        }),
        Some(case) => Err(format!(
            "unknown unsupported-prerequisite diagnostic {case:?}; expected vulkan-1.2 or presentation"
        )),
        None => Err(
            "missing unsupported-prerequisite diagnostic; expected vulkan-1.2 or presentation"
                .to_owned(),
        ),
    };
    Some(result)
}

fn verify_rejected_candidate(candidate: DeviceCandidate) -> Result<(), String> {
    match render_backend::select_device(vec![candidate]) {
        Ok(candidate) => Err(format!(
            "the deterministic unsupported-prerequisite diagnostic unexpectedly accepted {}",
            candidate.name
        )),
        Err(error) => Err(error.to_string()),
    }
}
