use super::compute_scene::ComputeSceneBundle;
use super::resource_error::ComputeRenderPathError;
use super::{
    CAMERA_WORD_COUNT, MAXIMUM_SEMANTIC_RAY_PROBES, SEMANTIC_RAY_BUFFER_WORD_COUNT,
    SEMANTIC_RAY_INPUT_START, SEMANTIC_RAY_INPUT_WORD_COUNT, SEMANTIC_RAY_OUTPUT_START,
    SEMANTIC_RAY_OUTPUT_WORD_COUNT,
};
use ash::vk;
use render_backend::CameraState;
use semantic_ray_oracle::{
    AxisNormal, SemanticRay, SemanticRayContact, SemanticRayContactClassification,
    SemanticRayError, SemanticRayObservation, SemanticRayProbe, SemanticRayResult,
};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use voxel_frontend::{VoxelCoordinate, VoxelSceneRevision};

#[derive(Debug, Error)]
pub enum ComputeCameraRayError {
    #[error("compute camera rays require a nonzero drawable extent")]
    EmptyDrawableExtent,
    #[error("pixel {pixel:?} lies outside drawable extent {extent:?}")]
    PixelOutsideDrawable { pixel: [u32; 2], extent: [u32; 2] },
    #[error("the shared Camera State could not produce a Semantic Ray")]
    SemanticRay(#[from] SemanticRayError),
}

pub fn camera_semantic_ray(
    camera: CameraState,
    extent: vk::Extent2D,
    pixel: [u32; 2],
) -> Result<SemanticRay, ComputeCameraRayError> {
    if extent.width == 0 || extent.height == 0 {
        return Err(ComputeCameraRayError::EmptyDrawableExtent);
    }
    let [pixel_x, pixel_y] = pixel;
    if pixel_x >= extent.width || pixel_y >= extent.height {
        return Err(ComputeCameraRayError::PixelOutsideDrawable {
            pixel,
            extent: [extent.width, extent.height],
        });
    }
    let eye = camera.eye();
    let forward = normalize(subtract(camera.target(), eye));
    let right = normalize(cross(forward, camera.up()));
    let upward = cross(right, forward);
    let normalized_x = 2.0 * (pixel_x as f32 + 0.5) / extent.width as f32 - 1.0;
    let normalized_y = 1.0 - 2.0 * (pixel_y as f32 + 0.5) / extent.height as f32;
    let tangent = (camera.field_of_view_degrees().to_radians() * 0.5).tan();
    let aspect_ratio = extent.width as f32 / extent.height as f32;
    let direction = normalize(add(
        forward,
        add(
            scale(right, normalized_x * tangent * aspect_ratio),
            scale(upward, normalized_y * tangent),
        ),
    ));
    let forward_cosine = dot(direction, forward);
    let maximum_distance = if camera.radial_far_clip() {
        camera.far_plane()
    } else {
        camera.far_plane() / forward_cosine
    };
    Ok(SemanticRay::new(
        eye.map(f64::from),
        direction.map(f64::from),
        f64::from(camera.near_plane() / forward_cosine),
        f64::from(maximum_distance),
    )?)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComputeSemanticRayProbeObservation {
    probe_identity: String,
    frame_sequence: u64,
    observation: SemanticRayObservation,
}

impl ComputeSemanticRayProbeObservation {
    pub fn probe_identity(&self) -> &str {
        &self.probe_identity
    }

    pub fn frame_sequence(&self) -> u64 {
        self.frame_sequence
    }

    pub fn observation(&self) -> &SemanticRayObservation {
        &self.observation
    }
}

#[derive(Clone, Debug)]
pub struct ComputeSemanticRayController {
    pub(super) state: Arc<Mutex<ComputeSemanticRayControlState>>,
}

#[derive(Debug, Default)]
pub(super) struct ComputeSemanticRayControlState {
    pending: Option<Vec<SemanticRayProbe>>,
    retained: Vec<ComputeSemanticRayProbeObservation>,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum ComputeSemanticRayControlError {
    #[error("at most {maximum} compute Semantic Ray probes can be requested at once")]
    TooManyProbes { maximum: usize },
    #[error("compute Semantic Ray probe {probe_identity} cannot be represented as f32 GPU input")]
    InputNotRepresentable { probe_identity: String },
    #[error("a compute Semantic Ray observation request is already pending")]
    RequestPending,
    #[error("compute Semantic Ray observation control is unavailable")]
    Unavailable,
}

impl ComputeSemanticRayController {
    pub fn request(
        &self,
        probes: Vec<SemanticRayProbe>,
    ) -> Result<(), ComputeSemanticRayControlError> {
        if probes.len() > MAXIMUM_SEMANTIC_RAY_PROBES {
            return Err(ComputeSemanticRayControlError::TooManyProbes {
                maximum: MAXIMUM_SEMANTIC_RAY_PROBES,
            });
        }
        if let Some(probe) = probes.iter().find(|probe| !probe_is_representable(probe)) {
            return Err(ComputeSemanticRayControlError::InputNotRepresentable {
                probe_identity: probe.identity().to_owned(),
            });
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeSemanticRayControlError::Unavailable)?;
        if state.pending.is_some() {
            return Err(ComputeSemanticRayControlError::RequestPending);
        }
        state.pending = Some(probes);
        Ok(())
    }

    pub fn drain(
        &self,
    ) -> Result<Vec<ComputeSemanticRayProbeObservation>, ComputeSemanticRayControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeSemanticRayControlError::Unavailable)?;
        Ok(std::mem::take(&mut state.retained))
    }

    pub(super) fn take_pending(
        &self,
    ) -> Result<Option<Vec<SemanticRayProbe>>, ComputeSemanticRayControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeSemanticRayControlError::Unavailable)?;
        Ok(state.pending.take())
    }

    pub(super) fn retain(
        &self,
        observations: Vec<ComputeSemanticRayProbeObservation>,
    ) -> Result<(), ComputeSemanticRayControlError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ComputeSemanticRayControlError::Unavailable)?;
        state.retained.extend(observations);
        Ok(())
    }
}

fn probe_is_representable(probe: &SemanticRayProbe) -> bool {
    let ray = probe.ray();
    ray.origin()
        .into_iter()
        .chain(ray.direction())
        .chain([ray.minimum_distance(), ray.maximum_distance()])
        .all(|value| (value as f32).is_finite())
}

pub(super) fn semantic_ray_input_words(
    probes: &[SemanticRayProbe],
) -> [u32; SEMANTIC_RAY_BUFFER_WORD_COUNT] {
    let mut words = [0_u32; SEMANTIC_RAY_BUFFER_WORD_COUNT];
    let mut probe_count = 0_u32;
    let (input_records, _) = words[SEMANTIC_RAY_INPUT_START..SEMANTIC_RAY_OUTPUT_START]
        .as_chunks_mut::<SEMANTIC_RAY_INPUT_WORD_COUNT>();
    for (record, probe) in input_records.iter_mut().zip(probes) {
        let ray = probe.ray();
        let [origin_x, origin_y, origin_z] = ray.origin().map(|value| (value as f32).to_bits());
        let [direction_x, direction_y, direction_z] =
            ray.direction().map(|value| (value as f32).to_bits());
        *record = [
            origin_x,
            origin_y,
            origin_z,
            (ray.minimum_distance() as f32).to_bits(),
            direction_x,
            direction_y,
            direction_z,
            (ray.maximum_distance() as f32).to_bits(),
        ];
        probe_count += 1;
    }
    words[0] = probe_count;
    words
}

pub(super) fn decode_semantic_ray_output(
    probes: &[SemanticRayProbe],
    words: &[u32; SEMANTIC_RAY_BUFFER_WORD_COUNT],
    bundle: &ComputeSceneBundle,
    revision: VoxelSceneRevision,
    frame_sequence: u64,
) -> Result<Vec<ComputeSemanticRayProbeObservation>, ComputeRenderPathError> {
    if bundle.revision() != revision {
        return Err(ComputeRenderPathError::InvalidSemanticRayOutput {
            probe_identity: probes
                .first()
                .map(|probe| probe.identity().to_owned())
                .unwrap_or_else(|| "empty-request".to_owned()),
            reason: "the installed CPU bundle no longer matches the recorded GPU revision",
        });
    }
    probes
        .iter()
        .enumerate()
        .map(|(index, probe)| {
            let offset = SEMANTIC_RAY_OUTPUT_START + index * SEMANTIC_RAY_OUTPUT_WORD_COUNT;
            let &[
                hit_flag,
                volume_word,
                coordinate_x,
                coordinate_y,
                coordinate_z,
                material_word,
                distance_bits,
                normal_code,
            ] = words
                .get(offset..)
                .and_then(|remaining| remaining.first_chunk::<SEMANTIC_RAY_OUTPUT_WORD_COUNT>())
                .ok_or(ComputeRenderPathError::InvalidSemanticRayOutput {
                    probe_identity: probe.identity().to_owned(),
                    reason: "the output record is outside the readback buffer",
                })?;
            let result = match hit_flag {
                0 => SemanticRayResult::Miss,
                1 => {
                    let volume_index = usize::try_from(volume_word).map_err(|_| {
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the volume index cannot address host memory",
                        }
                    })?;
                    let volume = bundle.volume_headers().get(volume_index).ok_or(
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the volume index is outside the installed bundle",
                        },
                    )?;
                    let material_index = material_word.checked_sub(1).ok_or(
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "a contact returned the empty material word",
                        },
                    )?;
                    let material_index = usize::try_from(material_index).map_err(|_| {
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the material index cannot address host memory",
                        }
                    })?;
                    let material = bundle.material_identities().get(material_index).ok_or(
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the material index is outside the installed bundle",
                        },
                    )?;
                    let distance = f32::from_bits(distance_bits);
                    if !distance.is_finite() {
                        return Err(ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the contact distance is not finite",
                        });
                    }
                    let classification = semantic_ray_classification(normal_code).ok_or(
                        ComputeRenderPathError::InvalidSemanticRayOutput {
                            probe_identity: probe.identity().to_owned(),
                            reason: "the contact normal code is invalid",
                        },
                    )?;
                    SemanticRayResult::Contact(SemanticRayContact::new(
                        volume.identity().clone(),
                        VoxelCoordinate::new(
                            i32::from_ne_bytes(coordinate_x.to_ne_bytes()),
                            i32::from_ne_bytes(coordinate_y.to_ne_bytes()),
                            i32::from_ne_bytes(coordinate_z.to_ne_bytes()),
                        ),
                        material.clone(),
                        f64::from(distance),
                        classification,
                    ))
                }
                _ => {
                    return Err(ComputeRenderPathError::InvalidSemanticRayOutput {
                        probe_identity: probe.identity().to_owned(),
                        reason: "the hit flag is invalid",
                    });
                }
            };
            Ok(ComputeSemanticRayProbeObservation {
                probe_identity: probe.identity().to_owned(),
                frame_sequence,
                observation: SemanticRayObservation::new(
                    bundle.scene_identity().clone(),
                    revision,
                    result,
                ),
            })
        })
        .collect()
}

fn semantic_ray_classification(code: u32) -> Option<SemanticRayContactClassification> {
    let normal = match code {
        0 => return Some(SemanticRayContactClassification::StartedInside),
        1 => AxisNormal::NegativeX,
        2 => AxisNormal::PositiveX,
        3 => AxisNormal::NegativeY,
        4 => AxisNormal::PositiveY,
        5 => AxisNormal::NegativeZ,
        6 => AxisNormal::PositiveZ,
        _ => return None,
    };
    Some(SemanticRayContactClassification::Entered(normal))
}

pub(super) fn camera_storage_words(
    camera: CameraState,
    extent: vk::Extent2D,
) -> [f32; CAMERA_WORD_COUNT] {
    let eye = camera.eye();
    let forward = normalize(subtract(camera.target(), eye));
    let right = normalize(cross(forward, camera.up()));
    let upward = cross(right, forward);
    let mut words = [0.0; CAMERA_WORD_COUNT];
    words[0..3].copy_from_slice(&eye);
    words[3] = if camera.radial_far_clip() {
        camera.far_plane()
    } else {
        0.0
    };
    words[4..7].copy_from_slice(&forward);
    words[8..11].copy_from_slice(&right);
    words[12..15].copy_from_slice(&upward);
    words[16] = camera.near_plane();
    words[17] = camera.far_plane();
    words[18] = (camera.field_of_view_degrees().to_radians() * 0.5).tan();
    words[19] = extent.width as f32 / extent.height as f32;
    words
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    [left_x - right_x, left_y - right_y, left_z - right_z]
}

fn add(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    [left_x + right_x, left_y + right_y, left_z + right_z]
}

fn scale(vector: [f32; 3], factor: f32) -> [f32; 3] {
    let [x, y, z] = vector;
    [x * factor, y * factor, z * factor]
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    left_x * right_x + left_y * right_y + left_z * right_z
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    [
        left_y * right_z - left_z * right_y,
        left_z * right_x - left_x * right_z,
        left_x * right_y - left_y * right_x,
    ]
}

fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = dot(vector, vector).sqrt();
    let [vector_x, vector_y, vector_z] = vector;
    [vector_x / length, vector_y / length, vector_z / length]
}
