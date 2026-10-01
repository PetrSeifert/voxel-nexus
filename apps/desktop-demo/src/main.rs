#[cfg(not(feature = "qualification"))]
mod qualification_unavailable;
#[cfg(not(feature = "qualification"))]
use qualification_unavailable::*;

#[cfg(target_os = "windows")]
mod camera_scenario;
#[cfg(target_os = "windows")]
mod compute_edit_scenario;
#[cfg(target_os = "windows")]
mod compute_switch_scenario;
#[cfg(target_os = "windows")]
mod evidence;
#[cfg(target_os = "windows")]
mod raster_scenario;
#[cfg(target_os = "windows")]
mod scenarios;
#[cfg(target_os = "windows")]
use evidence::*;
#[cfg(target_os = "windows")]
use scenarios::*;

#[cfg(target_os = "windows")]
mod windows_adapter;

#[cfg(any(target_os = "windows", test))]
mod free_fly_camera;
#[cfg(target_os = "windows")]
mod interactive_scenario;
#[cfg(target_os = "windows")]
mod render_path_switch;
#[cfg(any(target_os = "windows", test))]
mod voxel_editing;
#[cfg(target_os = "windows")]
use free_fly_camera::{FreeFlyCamera, MovementInput};
#[cfg(target_os = "windows")]
use interactive_scenario::*;
#[cfg(target_os = "windows")]
use render_path_switch::*;
#[cfg(target_os = "windows")]
use voxel_editing::*;

use canonical_inspection::{CanonicalCameraPose, overview_to_cavity_camera_move};
use canonical_scene::canonical_edit_semantic_ray_probes;
#[cfg(target_os = "windows")]
use compute_ray_render_path::{
    ComputeCandidateDisposition, ComputeConvergenceEvent, ComputeLifecycleController,
    ComputeRayRenderPathAdapter, ComputeSemanticRayController, ComputeSemanticRayProbeObservation,
};
#[cfg(target_os = "windows")]
use measurement_evidence::{MeasurementEvent, ResourceCounts, VoxelSceneRevisionIdentity};
use raster_render_path::{
    RasterArtifactInstaller, RasterArtifactPreparation, RasterArtifactPreparationEvent,
    RasterConvergenceCharacterization, RasterConvergenceStatus, RasterLifecycleController,
    RasterSafeRetirementDisposition,
};
#[cfg(target_os = "windows")]
use raster_render_path::{
    RasterRenderPathAdapter, RasterSemanticFaceController, RasterSemanticFaceObservation,
};
#[cfg(target_os = "windows")]
use render_backend::{
    CameraStateRevision, PresentationConfigurationId, RenderBackend, RenderPathHandoffControl,
    RenderPathReadiness, RenderPathSwitchDiagnostics, RenderPathSwitchRequestError,
};
#[cfg(target_os = "windows")]
use semantic_ray_oracle::{
    SemanticRayContactClassification, SemanticRayDistanceTolerance, SemanticRayObservation,
    SemanticRayProbe, SemanticRayProbeObservation, SemanticRayResult, observe_probe,
};
#[cfg(target_os = "windows")]
use std::collections::VecDeque;
#[cfg(target_os = "windows")]
use std::fs::File;
#[cfg(target_os = "windows")]
use std::io::{BufWriter, Write};
use std::process::ExitCode;
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};
use voxel_frontend::{
    VoxelCoordinate, VoxelEditCommand, VoxelEditOutcome, VoxelExtent, VoxelMaterialId, VoxelRegion,
    VoxelSceneRevision, VoxelSceneView, VoxelValue, VoxelVolumeId,
};
#[cfg(target_os = "windows")]
use winit::event::ElementState;
#[cfg(target_os = "windows")]
use winit::event_loop::ActiveEventLoop;
#[cfg(target_os = "windows")]
use winit::keyboard::{Key, NamedKey};
#[cfg(target_os = "windows")]
use winit::window::Window;

fn measurement_revision(
    revision: VoxelSceneRevision,
) -> Result<VoxelSceneRevisionIdentity, String> {
    VoxelSceneRevisionIdentity::new(revision.to_string()).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn main() -> ExitCode {
    application_exit_code(run())
}

fn application_exit_code(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Voxel Nexus could not start: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|argument| argument == "--streamed-world")
        && let Err(error) = parse_render_configuration(arguments.clone().into_iter())
    {
        return application_exit_code(Err(error));
    }
    for argument in &arguments {
        if let Err(error) = require_qualification_argument(argument) {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    }
    if let Some(result) = background_preparation_failure_diagnostic(arguments.clone().into_iter()) {
        return application_exit_code(result);
    }
    if let Some(result) = render_path_failure_diagnostic(arguments.clone().into_iter()) {
        return application_exit_code(result);
    }
    if let Some(result) = unsupported_prerequisite_diagnostic(arguments.clone().into_iter()) {
        return application_exit_code(result);
    }
    if arguments
        .iter()
        .any(|argument| argument == "--report-canonical-configuration")
    {
        let result = parse_render_configuration(arguments.into_iter())
            .and_then(|(configuration, _)| report_render_configuration(&configuration));
        return application_exit_code(result);
    }
    eprintln!("The Voxel Nexus desktop demo currently supports Windows only.");
    ExitCode::SUCCESS
}

mod configuration;
use configuration::*;
#[cfg(any(target_os = "windows", test))]
mod streamed_crossing;
#[cfg(any(target_os = "windows", test))]
mod streamed_fixture_recipe;
mod streamed_neighbourhood;
#[cfg(any(target_os = "windows", test))]
mod streamed_world;

#[cfg(target_os = "windows")]
mod application;
#[cfg(target_os = "windows")]
use application::*;

mod diagnostics;
#[cfg(not(target_os = "windows"))]
use diagnostics::{
    background_preparation_failure_diagnostic, render_path_failure_diagnostic,
    unsupported_prerequisite_diagnostic,
};

#[cfg(all(test, target_os = "windows"))]
mod measurement_tests;
