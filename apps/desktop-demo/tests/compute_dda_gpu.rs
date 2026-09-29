#![cfg(windows)]

#[allow(dead_code)]
#[path = "../src/windows_adapter.rs"]
mod windows_adapter;

use ash::vk;
use compute_ray_render_path::{
    BrickmapSceneBundle, ComputeRayRenderPathAdapter, ComputeRepresentation,
};
use render_backend::{CameraState, CameraStateRevision, RenderBackend, RenderBackendOptions};
use semantic_ray_oracle::{
    SemanticRay, SemanticRayDistanceTolerance, SemanticRayProbe, SemanticRayResult, observe,
};
use voxel_frontend::{
    DenseVoxelBatch, DenseVoxelScene, DenseVoxelVolume, VoxelCoordinate, VoxelExtent,
    VoxelFrontend, VoxelMaterial, VoxelMaterialId, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelSceneView, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    platform::windows::EventLoopBuilderExtWindows,
    window::{Window, WindowId},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn scene(
    voxel_size: f32,
    extent: VoxelExtent,
    occupied_index: usize,
) -> Result<VoxelSceneView, Box<dyn std::error::Error>> {
    let material = VoxelMaterialId::new("stone");
    let mut values =
        vec![VoxelValue::Empty; usize::try_from(extent.dimensions().into_iter().product::<u32>())?];
    *values
        .get_mut(occupied_index)
        .ok_or("invalid fixture coordinate")? = VoxelValue::Occupied(material.clone());
    Ok(VoxelFrontend::new().publish(DenseVoxelScene::new(
        VoxelSceneId::new("gpu-dda-regression"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material, [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("volume"), extent, [0.0; 3], voxel_size),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                values,
            )],
        )],
    ))?)
}

fn check_gpu(
    window: &Window,
    view: VoxelSceneView,
    probes: Vec<SemanticRayProbe>,
    scale: f64,
) -> TestResult {
    for representation in [
        ComputeRepresentation::Dense,
        ComputeRepresentation::Brickmap {
            budget_bytes: 128 * 1024 * 1024,
        },
    ] {
        check_gpu_representation(
            window,
            view.clone(),
            probes.clone(),
            scale,
            representation,
            vec![],
        )?;
    }
    Ok(())
}

fn check_gpu_representation(
    window: &Window,
    view: VoxelSceneView,
    probes: Vec<SemanticRayProbe>,
    scale: f64,
    representation: ComputeRepresentation,
    successors: Vec<(voxel_frontend::VoxelEditOutcome, VoxelSceneView)>,
) -> TestResult {
    let camera = CameraState::new([0.0, 0.0, 5.0], [0.0; 3], [0.0, 1.0, 0.0], 50.0, 0.1, 100.0)?;
    let (mut path, measurement) =
        ComputeRayRenderPathAdapter::new_with_representation_and_measurement(
            view.clone(),
            camera,
            CameraStateRevision::new(1),
            representation,
        )?;
    let convergence = path.enable_convergence_control();
    let controller = path.enable_semantic_ray_observation();
    controller.request(probes.clone())?;
    let size = window.inner_size();
    let mut backend = RenderBackend::initialize_with_options(
        c"Compute DDA GPU regression",
        &windows_adapter::WindowsPresentationAdapter::new(window),
        vk::Extent2D {
            width: size.width,
            height: size.height,
        },
        path,
        RenderBackendOptions {
            validation_enabled: true,
            presentation_throttling_enabled: false,
            gpu_timestamps_enabled: false,
        },
    )?;
    println!("{}", backend.runtime_context());
    let result = (|| -> TestResult {
        let phases = std::iter::once((None, view)).chain(
            successors
                .into_iter()
                .map(|(outcome, view)| (Some(outcome), view)),
        );
        for (outcome, view) in phases {
            if let Some(outcome) = outcome {
                convergence.submit(outcome)?;
                for _ in 0..1000 {
                    backend.draw_frame()?;
                    if convergence.status()?.visible_revision() == view.revision() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                assert_eq!(convergence.status()?.visible_revision(), view.revision());
                controller.request(probes.clone())?;
            }
            let mut observations = Vec::new();
            for _ in 0..8 {
                backend.draw_frame()?;
                observations.extend(controller.drain()?);
                if observations.len() == probes.len() {
                    break;
                }
            }
            if observations.len() != probes.len() {
                return Err("installed GPU traversal did not return every probe".into());
            }
            let tolerance = SemanticRayDistanceTolerance::new(scale * 1.0e-6)?;
            let cpu_bundle = BrickmapSceneBundle::from_view(&view)?;
            for (probe, actual) in probes.iter().zip(&observations) {
                let expected = observe(&view, probe.ray())?;
                let cpu = cpu_bundle.observe(probe.ray());
                assert!(cpu.agrees_with(&expected, tolerance));
                if actual.probe_identity() != probe.identity()
                    || !actual.observation().agrees_with(&expected, tolerance)
                {
                    return Err(format!(
                        "{}: GPU {:?}, oracle {expected:?}",
                        probe.identity(),
                        actual.observation()
                    )
                    .into());
                }
            }
            let events = measurement.drain()?;
            let upload = events
                .iter()
                .find(|event| event.phase() == compute_ray_render_path::ComputeTimingPhase::Upload)
                .ok_or("missing upload observation")?;
            assert_eq!(upload.revision(), view.revision());
            assert!(upload.uploaded_bytes() > 0);
            assert!(upload.elapsed_milliseconds().is_finite());
        }
        Ok(())
    })();
    backend.shutdown()?;
    assert_eq!(backend.validation_error_count(), 0);
    assert_eq!(backend.validation_warning_count(), 0);
    result
}

fn check_envelope_corner(
    window: &Window,
    origin: [f32; 3],
    dimensions: [u32; 3],
    voxel_size: f32,
    representation: ComputeRepresentation,
) -> TestResult {
    use voxel_frontend::{
        SparseVoxelBackground, SparseVoxelBatch, SparseVoxelScene, SparseVoxelVolume,
        VoxelRegionFill,
    };
    let material = VoxelMaterialId::new("stone");
    let coordinate = VoxelCoordinate::new(
        i32::try_from(dimensions[0] - 1)?,
        i32::try_from(dimensions[1] - 1)?,
        i32::try_from(dimensions[2] - 1)?,
    );
    let view = VoxelFrontend::new().publish_sparse(SparseVoxelScene::new(
        VoxelSceneId::new("envelope-corner"),
        VoxelSceneRevision::new(1),
        vec![VoxelMaterial::new(material.clone(), [1.0; 4])],
        vec![SparseVoxelVolume::new(
            VoxelVolumeMetadata::new(
                VoxelVolumeId::new("volume"),
                VoxelExtent::new(dimensions[0], dimensions[1], dimensions[2]),
                origin,
                voxel_size,
            ),
            SparseVoxelBackground::Empty,
            vec![SparseVoxelBatch::Fill(VoxelRegionFill::new(
                VoxelRegion::new(coordinate, VoxelExtent::new(1, 1, 1)),
                VoxelValue::Occupied(material),
            ))],
        )],
    ))?;
    let scale = f64::from(voxel_size);
    let minimum: [f64; 3] = std::array::from_fn(|axis| {
        f64::from(origin[axis]) + f64::from(dimensions[axis] - 1) * scale
    });
    for axis in 0..3 {
        let rays = [
            ("face-positive", [-1.0, 0.5, 0.5], [1.0, 0.0, 0.0]),
            ("face-negative", [2.0, 0.5, 0.5], [-1.0, 0.0, 0.0]),
            ("edge-positive", [-1.0, -1.0, 0.5], [1.0, 1.0, 0.0]),
            ("edge-negative", [2.0, 2.0, 0.5], [-1.0, -1.0, 0.0]),
            ("corner-positive", [-1.0; 3], [1.0; 3]),
            ("corner-negative", [2.0; 3], [-1.0; 3]),
            (
                "nearly-axis-positive",
                [-1.0, 0.5, 0.5],
                [1.0, 0.00001, -0.00001],
            ),
            (
                "nearly-axis-negative",
                [2.0, 0.5, 0.5],
                [-1.0, -0.00001, 0.00001],
            ),
        ];
        let probes = rays
            .into_iter()
            .map(|(name, offset, direction)| {
                let ray_origin = std::array::from_fn(|component| {
                    minimum[component] + offset[(component + axis) % 3] * scale
                });
                let direction = std::array::from_fn(|component| direction[(component + axis) % 3]);
                Ok(SemanticRayProbe::new(
                    format!(
                        "{representation:?}-{origin:?}-{dimensions:?}-{voxel_size}-{axis}-{name}"
                    ),
                    SemanticRay::new(ray_origin, direction, 0.0, 4.0 * scale)?,
                )?)
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        for probe in &probes {
            let expected = semantic_ray_oracle::observe_along_ray(&view, probe.ray())?;
            let SemanticRayResult::Contact(contact) = expected.result() else {
                return Err(format!("{} must hit the far-corner voxel", probe.identity()).into());
            };
            assert_eq!(contact.coordinate(), coordinate);
        }
        check_gpu_representation(window, view.clone(), probes, scale, representation, vec![])?;
    }
    Ok(())
}

fn check_sparse_envelope(window: &Window) -> TestResult {
    check_envelope_corner(
        window,
        [-65536.0; 3],
        [1; 3],
        0.125,
        ComputeRepresentation::Brickmap {
            budget_bytes: 128 * 1024 * 1024,
        },
    )?;
    for origin in [[-65536.0; 3], [65536.0; 3]] {
        for (voxel_size, length) in [(0.125, 65536), (1.0, 65536), (16.0, 4096)] {
            for axis in 0..3 {
                // Thin volumes reach the exact envelope without a cubic oracle allocation.
                let mut dimensions = [2; 3];
                dimensions[axis] = length;
                check_envelope_corner(
                    window,
                    origin,
                    dimensions,
                    voxel_size,
                    ComputeRepresentation::Brickmap {
                        budget_bytes: 128 * 1024 * 1024,
                    },
                )?;
            }
        }
    }
    for (origin, dimensions, voxel_size) in [
        ([200000.0; 3], [2; 3], 1.0),
        ([-200000.0; 3], [2; 3], 1.0),
        ([0.0; 3], [65537, 2, 2], 1.0),
        ([65536.0; 3], [4097, 2, 2], 16.0),
        ([0.0; 3], [2; 3], 0.0625),
        ([0.0; 3], [2; 3], 32.0),
    ] {
        check_envelope_corner(
            window,
            origin,
            dimensions,
            voxel_size,
            ComputeRepresentation::Dense,
        )?;
    }
    Ok(())
}

#[cfg(feature = "qualification")]
#[derive(Clone, Copy, Eq, PartialEq)]
enum BrickmapLifecycleScenario {
    Transitions,
    CapacityExhaustion,
    RoundedAllocationRejection,
    PartialWriteFailure,
}

#[cfg(feature = "qualification")]
fn check_brickmap_lifecycle(window: &Window, scenario: BrickmapLifecycleScenario) -> TestResult {
    use compute_ray_render_path::ComputeConvergenceEvent;
    use voxel_frontend::{VoxelEdit, VoxelEditCommand};
    let frontend = VoxelFrontend::new();
    let extent = VoxelExtent::new(16, 8, 8);
    let initial = frontend.publish(DenseVoxelScene::new(
        VoxelSceneId::new("brickmap-lifecycle"),
        VoxelSceneRevision::new(0),
        vec![VoxelMaterial::new(VoxelMaterialId::new("stone"), [1.0; 4])],
        vec![DenseVoxelVolume::new(
            VoxelVolumeMetadata::new(VoxelVolumeId::new("volume"), extent, [0.0; 3], 1.0),
            vec![DenseVoxelBatch::new(
                VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), extent),
                (0..1024)
                    .map(|index| {
                        if scenario == BrickmapLifecycleScenario::PartialWriteFailure && index == 7
                        {
                            VoxelValue::Occupied(VoxelMaterialId::new("stone"))
                        } else {
                            VoxelValue::Empty
                        }
                    })
                    .collect(),
            )],
        )],
    ))?;
    let camera = CameraState::new([0.0, 0.0, 5.0], [0.0; 3], [0.0, 1.0, 0.0], 50.0, 0.1, 100.0)?;
    let mut path = ComputeRayRenderPathAdapter::new_with_representation(
        initial.clone(),
        camera,
        CameraStateRevision::new(1),
        ComputeRepresentation::Brickmap {
            budget_bytes: match scenario {
                BrickmapLifecycleScenario::CapacityExhaustion => 3000,
                BrickmapLifecycleScenario::RoundedAllocationRejection => 2276,
                _ => 8192,
            },
        },
    )?;
    let convergence = path.enable_convergence_control_with_hold(true);
    let controller = path.enable_semantic_ray_observation();
    let size = window.inner_size();
    let mut backend = RenderBackend::initialize_with_options(
        c"Brickmap lifecycle regression",
        &windows_adapter::WindowsPresentationAdapter::new(window),
        vk::Extent2D {
            width: size.width,
            height: size.height,
        },
        path,
        RenderBackendOptions {
            validation_enabled: true,
            presentation_throttling_enabled: false,
            gpu_timestamps_enabled: false,
        },
    )?;
    let probes = [0.5, 1.5, 8.5]
        .into_iter()
        .enumerate()
        .map(|(index, x)| {
            Ok(SemanticRayProbe::new(
                format!("lifecycle-{index}"),
                SemanticRay::new([x, 0.5, -1.0], [0.0, 0.0, 1.0], 0.0, 20.0)?,
            )?)
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let result = (|| -> TestResult {
        let verify = |backend: &mut RenderBackend, view: &VoxelSceneView| -> TestResult {
            controller.request(probes.clone())?;
            let mut actual = Vec::new();
            for _ in 0..8 {
                backend.draw_frame()?;
                actual.extend(controller.drain()?);
                if actual.len() == probes.len() {
                    break;
                }
            }
            assert_eq!(actual.len(), probes.len());
            for (probe, actual) in probes.iter().zip(actual) {
                assert!(actual.observation().agrees_with(
                    &observe(view, probe.ray())?,
                    SemanticRayDistanceTolerance::new(1.0e-6)?
                ));
            }
            Ok(())
        };
        verify(&mut backend, &initial)?;
        let occupied = VoxelValue::Occupied(VoxelMaterialId::new("stone"));
        let first = frontend.edit(VoxelEditCommand::new(
            VoxelVolumeId::new("volume"),
            VoxelCoordinate::new(0, 0, 0),
            occupied.clone(),
        ))?;
        convergence.submit(first)?;
        for _ in 0..1000 {
            backend.draw_frame()?;
            if scenario == BrickmapLifecycleScenario::RoundedAllocationRejection
                && convergence.drain_events()?.iter().any(|event| matches!(event, ComputeConvergenceEvent::Failure(failure)
                    if failure.phase() == compute_ray_render_path::ComputeConvergenceFailurePhase::Upload
                        && failure.source().contains("exceeds configured budget 2276 bytes")))
            {
                assert_eq!(convergence.status()?.visible_revision(), initial.revision());
                assert!(convergence.status()?.hidden().is_none());
                verify(&mut backend, &initial)?;
                return Ok(());
            }
            if convergence.post_upload_revision()?.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            convergence.post_upload_revision()?,
            Some(VoxelSceneRevision::new(1))
        );
        assert_eq!(convergence.status()?.visible_revision(), initial.revision());
        if scenario == BrickmapLifecycleScenario::PartialWriteFailure {
            assert_eq!(
                convergence
                    .status()?
                    .hidden_patch
                    .ok_or("missing patch")?
                    .slots_reserved,
                1
            );
        } else {
            let growth = convergence
                .status()?
                .hidden_growth
                .ok_or("missing growth")?;
            assert_eq!((growth.old_capacity, growth.new_capacity), (0, 1));
            assert_eq!(growth.trigger_revision, VoxelSceneRevision::new(1));
            assert!((2276..=3000).contains(&growth.predicted_peak_bytes));
            assert!(
                growth.actual_peak_bytes.ok_or("missing actual peak")?
                    <= growth.predicted_peak_bytes
            );
        }
        verify(&mut backend, &initial)?;
        convergence.release_post_upload()?;
        backend.draw_frame()?;
        let visible = frontend.scene_view()?;
        verify(&mut backend, &visible)?;
        if scenario != BrickmapLifecycleScenario::Transitions {
            let partial = scenario == BrickmapLifecycleScenario::PartialWriteFailure;
            if partial {
                convergence.inject_partial_write_failure()?;
            }
            let outcome = frontend.edit(VoxelEditCommand::new(
                VoxelVolumeId::new("volume"),
                VoxelCoordinate::new(if partial { 1 } else { 8 }, 0, 0),
                occupied,
            ))?;
            convergence.submit(outcome)?;
            let mut failed = false;
            for _ in 0..1000 {
                let frame = backend.draw_frame();
                if partial && frame.is_err() {
                    failed = true;
                    break;
                }
                frame?;
                if convergence
                    .drain_events()?
                    .iter()
                    .any(|event| matches!(event, ComputeConvergenceEvent::Failure(_)))
                {
                    failed = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(failed);
            assert_eq!(convergence.status()?.visible_revision(), visible.revision());
            if partial {
                for _ in 0..3 {
                    assert!(backend.draw_frame().is_err());
                }
            } else {
                verify(&mut backend, &visible)?;
            }
        } else {
            convergence.submit(frontend.edit(VoxelEditCommand::new(
                VoxelVolumeId::new("volume"),
                VoxelCoordinate::new(1, 0, 0),
                occupied.clone(),
            ))?)?;
            let grown_view = frontend.scene_view()?;
            for _ in 0..1000 {
                backend.draw_frame()?;
                if convergence.status()?.visible_revision() == grown_view.revision() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let growth = convergence
                .status()?
                .installed_growth
                .ok_or("missing installed growth")?;
            assert_eq!(growth.trigger_revision, grown_view.revision());
            assert_eq!((growth.old_capacity, growth.new_capacity), (1, 2));
            assert!((5348..=8192).contains(&growth.predicted_peak_bytes));
            assert!(
                growth.actual_peak_bytes.ok_or("missing actual peak")?
                    <= growth.predicted_peak_bytes
            );
            verify(&mut backend, &grown_view)?;
            // Subsequent transitions reuse slots released at installation.
            for (step, fill) in [Some(VoxelValue::Empty), None, Some(occupied.clone()), None]
                .into_iter()
                .enumerate()
            {
                let outcome = if let Some(value) = fill {
                    frontend.edit(VoxelEditCommand::from_edits(
                        (0..512)
                            .map(|index| {
                                VoxelEdit::new(
                                    VoxelVolumeId::new("volume"),
                                    VoxelCoordinate::new(index % 8, index / 8 % 8, index / 64),
                                    value.clone(),
                                )
                            })
                            .collect(),
                    ))?
                } else {
                    frontend.edit(VoxelEditCommand::new(
                        VoxelVolumeId::new("volume"),
                        VoxelCoordinate::new(0, 0, 0),
                        if step == 1 {
                            occupied.clone()
                        } else {
                            VoxelValue::Empty
                        },
                    ))?
                };
                convergence.submit(outcome)?;
                let view = frontend.scene_view()?;
                for _ in 0..1000 {
                    backend.draw_frame()?;
                    if convergence.status()?.visible_revision() == view.revision() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                assert_eq!(convergence.status()?.visible_revision(), view.revision());
                let patch = convergence.status()?.installed_patch;
                assert_eq!(
                    (patch.slots_reserved, patch.slots_retired),
                    if step % 2 == 0 { (0, 1) } else { (1, 0) }
                );
                verify(&mut backend, &view)?;
            }
        }
        Ok(())
    })();
    backend.shutdown()?;
    assert_eq!(backend.validation_error_count(), 0);
    assert_eq!(backend.validation_warning_count(), 0);
    result
}

fn run_fixtures(window: &Window) -> TestResult {
    check_sparse_envelope(window)?;
    #[cfg(feature = "qualification")]
    for scenario in [
        BrickmapLifecycleScenario::Transitions,
        BrickmapLifecycleScenario::CapacityExhaustion,
        BrickmapLifecycleScenario::RoundedAllocationRejection,
        BrickmapLifecycleScenario::PartialWriteFailure,
    ] {
        check_brickmap_lifecycle(window, scenario)?;
    }
    check_gpu(
        window,
        scene(1.0, VoxelExtent::new(16, 8, 1), 8 + 16 * 4)?,
        vec![SemanticRayProbe::new(
            "empty-brick-negative-internal-boundary",
            SemanticRay::new([0.0, 12.0, 0.5], [1.0, -1.0, 0.0], 0.0, 30.0)?,
        )?],
        1.0,
    )?;

    check_gpu(
        window,
        scene(1.0, VoxelExtent::new(16, 8, 1), 8 + 16 * 3)?,
        vec![SemanticRayProbe::new(
            "empty-brick-negative-internal-face-tie",
            SemanticRay::new([0.0, 12.0, 0.5], [1.0, -1.0, 0.0], 0.0, 30.0)?,
        )?],
        1.0,
    )?;
    let frontend = VoxelFrontend::new();
    let view = frontend.publish_sparse(
        canonical_scene::generate_canonical_scene(canonical_scene::CanonicalSceneScale::Small)?
            .into_scene(),
    )?;
    let mut successors = Vec::new();
    for x in [0, 8, 16] {
        let outcome = frontend.edit(voxel_frontend::VoxelEditCommand::new(
            VoxelVolumeId::new("canonical-volume"),
            VoxelCoordinate::new(x, 0, 0),
            VoxelValue::Occupied(VoxelMaterialId::new("canonical-warm")),
        ))?;
        successors.push((outcome, frontend.scene_view()?));
    }
    let probes = [0.125, 2.125, 4.125]
        .into_iter()
        .enumerate()
        .map(|(index, x)| {
            Ok(SemanticRayProbe::new(
                format!("canonical-edit-{index}"),
                SemanticRay::new([x, 0.125, -1.0], [0.0, 0.0, 1.0], 0.0, 30.0)?,
            )?)
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    check_gpu_representation(
        window,
        view,
        probes,
        1.0,
        ComputeRepresentation::Brickmap {
            budget_bytes: 128 * 1024 * 1024,
        },
        successors,
    )?;
    let probes = [
        ([-1.0, 8.5, 8.5], [1.0, 0.0, 0.0]),
        ([26.0, 8.5, 8.5], [-1.0, 0.0, 0.0]),
        ([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0]),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (origin, direction))| {
        Ok(SemanticRayProbe::new(
            format!("clipped-{index}"),
            SemanticRay::new(origin, direction, 0.0, 40.0)?,
        )?)
    })
    .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    check_gpu(
        window,
        scene(1.0, VoxelExtent::new(25, 9, 9), 25 * 9 * 9 - 1)?,
        probes,
        1.0,
    )?;

    for voxel_size in [1.0_f32, 0.125, 16.0] {
        let scale = f64::from(voxel_size);
        for negative in [false, true] {
            for first_axis in 0..3 {
                let second_axis = (first_axis + 1) % 3;
                let mut origin = [0.5; 3];
                let mut direction = [0.0; 3];
                let mut coordinate = [0_usize; 3];
                if negative {
                    origin[first_axis] = 1.5;
                    origin[second_axis] = 1.500002;
                    direction[first_axis] = -1.0;
                    direction[second_axis] = -1.0;
                    coordinate[second_axis] = 1;
                } else {
                    origin[second_axis] = 0.499998;
                    direction[first_axis] = 1.0;
                    direction[second_axis] = 1.0;
                    coordinate[first_axis] = 1;
                }
                // Use the same representable origin in the oracle and the GPU input buffer.
                let origin = origin.map(|component| f64::from((component * scale) as f32));
                let occupied_index = coordinate[0] + 2 * coordinate[1] + 4 * coordinate[2];
                let probe = SemanticRayProbe::new(
                    format!("near-crossing-scale-{scale}-negative-{negative}-axis-{first_axis}"),
                    SemanticRay::new(origin, direction, 0.0, 4.0 * scale)?,
                )?;
                let mut simultaneous_origin = origin;
                simultaneous_origin[second_axis] = simultaneous_origin[first_axis];
                let simultaneous = SemanticRayProbe::new(
                    "simultaneous-crossing-skips-zero-length-cell",
                    SemanticRay::new(simultaneous_origin, direction, 0.0, 4.0 * scale)?,
                )?;
                let extent = VoxelExtent::new(2, 2, if first_axis == 0 { 1 } else { 2 });
                check_gpu(
                    window,
                    scene(voxel_size, extent, occupied_index)?,
                    vec![probe, simultaneous],
                    scale,
                )?;
            }
        }

        let rays = [
            ("true-corner", [-1.0; 3], [1.0; 3], 0.0, 6.0),
            ("true-edge", [0.5, 0.5, 1.5], [1.0, 1.0, 0.0], 0.0, 4.0),
            (
                "maximum-inward",
                [2.0, 1.5, 1.5],
                [-1.0, 0.0, 0.0],
                0.0,
                2.0,
            ),
            (
                "internal-boundary",
                [1.0, 1.5, 1.5],
                [-1.0, 0.0, 0.0],
                0.0,
                2.0,
            ),
            (
                "clipped-before-contact",
                [-2.0, 1.5, 1.5],
                [1.0, 0.0, 0.0],
                0.0,
                1.5,
            ),
            (
                "half-open-maximum",
                [-1.0, 2.0, 1.5],
                [1.0, 0.0, 0.0],
                0.0,
                4.0,
            ),
            (
                "clipped-inside",
                [-1.0, 1.5, 1.5],
                [1.0, 0.0, 0.0],
                2.5,
                4.0,
            ),
        ];
        let probes = rays
            .into_iter()
            .map(|(name, origin, direction, minimum, maximum)| {
                Ok(SemanticRayProbe::new(
                    name,
                    SemanticRay::new(
                        origin.map(|component| component * scale),
                        direction,
                        minimum * scale,
                        maximum * scale,
                    )?,
                )?)
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        check_gpu(
            window,
            scene(voxel_size, VoxelExtent::new(2, 2, 2), 7)?,
            probes,
            scale,
        )?;
        let probes = [
            ("negative-true-corner", [1.5; 3], [-1.0; 3]),
            ("negative-true-edge", [1.5, 1.5, 0.5], [-1.0, -1.0, 0.0]),
        ]
        .into_iter()
        .map(|(name, origin, direction)| {
            Ok(SemanticRayProbe::new(
                name,
                SemanticRay::new(
                    origin.map(|component| component * scale),
                    direction,
                    0.0,
                    4.0 * scale,
                )?,
            )?)
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        check_gpu(
            window,
            scene(voxel_size, VoxelExtent::new(2, 2, 2), 0)?,
            probes,
            scale,
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct GpuTest {
    result: Option<TestResult>,
}

impl ApplicationHandler for GpuTest {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some((|| {
            let window = event_loop.create_window(
                Window::default_attributes()
                    .with_visible(false)
                    .with_inner_size(winit::dpi::PhysicalSize::new(64, 64)),
            )?;
            run_fixtures(&window)
        })());
        event_loop.exit();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

#[test]
#[ignore = "requires a Windows Vulkan 1.3 GPU and the Vulkan validation layer"]
fn installed_gpu_dda_matches_semantic_oracle() -> TestResult {
    let event_loop = EventLoop::builder().with_any_thread(true).build()?;
    let mut application = GpuTest::default();
    event_loop.run_app(&mut application)?;
    application.result.ok_or("GPU test did not run")?
}
