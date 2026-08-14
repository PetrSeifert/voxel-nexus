use std::sync::Arc;

use thiserror::Error;
use voxel_frontend::{
    VoxelCoordinate, VoxelFrontendError, VoxelMaterialId, VoxelRegion, VoxelSceneId,
    VoxelSceneRevision, VoxelSceneView, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AxisNormal {
    NegativeX,
    PositiveX,
    NegativeY,
    PositiveY,
    NegativeZ,
    PositiveZ,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SemanticRayContactClassification {
    Entered(AxisNormal),
    StartedInside,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticRayContact {
    volume_identity: VoxelVolumeId,
    coordinate: VoxelCoordinate,
    material_identity: VoxelMaterialId,
    distance: f64,
    classification: SemanticRayContactClassification,
}

impl SemanticRayContact {
    pub fn new(
        volume_identity: VoxelVolumeId,
        coordinate: VoxelCoordinate,
        material_identity: VoxelMaterialId,
        distance: f64,
        classification: SemanticRayContactClassification,
    ) -> Self {
        Self {
            volume_identity,
            coordinate,
            material_identity,
            distance,
            classification,
        }
    }

    pub fn volume_identity(&self) -> &VoxelVolumeId {
        &self.volume_identity
    }

    pub fn coordinate(&self) -> VoxelCoordinate {
        self.coordinate
    }

    pub fn material_identity(&self) -> &VoxelMaterialId {
        &self.material_identity
    }

    pub fn distance(&self) -> f64 {
        self.distance
    }

    pub fn classification(&self) -> SemanticRayContactClassification {
        self.classification.clone()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SemanticRayResult {
    Miss,
    Contact(SemanticRayContact),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticRayObservation {
    scene_identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    result: SemanticRayResult,
}

impl SemanticRayObservation {
    pub fn new(
        scene_identity: VoxelSceneId,
        revision: VoxelSceneRevision,
        result: SemanticRayResult,
    ) -> Self {
        Self {
            scene_identity,
            revision,
            result,
        }
    }

    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn revision(&self) -> VoxelSceneRevision {
        self.revision
    }

    pub fn result(&self) -> &SemanticRayResult {
        &self.result
    }

    pub fn agrees_with(
        &self,
        other: &Self,
        distance_tolerance: SemanticRayDistanceTolerance,
    ) -> bool {
        self.scene_identity == other.scene_identity
            && self.revision == other.revision
            && match (&self.result, &other.result) {
                (SemanticRayResult::Miss, SemanticRayResult::Miss) => true,
                (
                    SemanticRayResult::Contact(contact),
                    SemanticRayResult::Contact(other_contact),
                ) => contact.agrees_with(other_contact, distance_tolerance),
                (SemanticRayResult::Miss, SemanticRayResult::Contact(_))
                | (SemanticRayResult::Contact(_), SemanticRayResult::Miss) => false,
            }
    }
}

impl SemanticRayContact {
    fn agrees_with(&self, other: &Self, distance_tolerance: SemanticRayDistanceTolerance) -> bool {
        self.volume_identity == other.volume_identity
            && self.coordinate == other.coordinate
            && self.material_identity == other.material_identity
            && self.classification == other.classification
            && (self.distance - other.distance).abs()
                <= distance_tolerance.maximum_absolute_difference
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SemanticRayDistanceTolerance {
    maximum_absolute_difference: f64,
}

impl SemanticRayDistanceTolerance {
    pub fn new(
        maximum_absolute_difference: f64,
    ) -> Result<Self, SemanticRayDistanceToleranceError> {
        if !maximum_absolute_difference.is_finite() || maximum_absolute_difference < 0.0 {
            return Err(SemanticRayDistanceToleranceError::InvalidMaximumDifference);
        }
        Ok(Self {
            maximum_absolute_difference,
        })
    }

    pub fn maximum_absolute_difference(self) -> f64 {
        self.maximum_absolute_difference
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticRay {
    origin: [f64; 3],
    direction: [f64; 3],
    minimum_distance: f64,
    maximum_distance: f64,
}

impl SemanticRay {
    pub fn new(
        origin: [f64; 3],
        direction: [f64; 3],
        minimum_distance: f64,
        maximum_distance: f64,
    ) -> Result<Self, SemanticRayError> {
        if origin
            .iter()
            .chain(direction.iter())
            .any(|component| !component.is_finite())
        {
            return Err(SemanticRayError::NonFiniteInput);
        }
        if !minimum_distance.is_finite()
            || !maximum_distance.is_finite()
            || minimum_distance < 0.0
            || maximum_distance < minimum_distance
        {
            return Err(SemanticRayError::InvalidClipInterval);
        }
        let [direction_x, direction_y, direction_z] = direction;
        let maximum_component = direction_x
            .abs()
            .max(direction_y.abs())
            .max(direction_z.abs());
        if maximum_component == 0.0 {
            return Err(SemanticRayError::ZeroLengthDirection);
        }
        let scaled_direction = [
            direction_x / maximum_component,
            direction_y / maximum_component,
            direction_z / maximum_component,
        ];
        let [scaled_x, scaled_y, scaled_z] = scaled_direction;
        let scaled_length = scaled_x.hypot(scaled_y).hypot(scaled_z);
        Ok(Self {
            origin,
            direction: [
                scaled_x / scaled_length,
                scaled_y / scaled_length,
                scaled_z / scaled_length,
            ],
            minimum_distance,
            maximum_distance,
        })
    }

    pub fn origin(&self) -> [f64; 3] {
        self.origin
    }

    pub fn direction(&self) -> [f64; 3] {
        self.direction
    }

    pub fn minimum_distance(&self) -> f64 {
        self.minimum_distance
    }

    pub fn maximum_distance(&self) -> f64 {
        self.maximum_distance
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticRayProbe {
    identity: Arc<str>,
    ray: SemanticRay,
}

impl SemanticRayProbe {
    pub fn new(
        identity: impl Into<String>,
        ray: SemanticRay,
    ) -> Result<Self, SemanticRayProbeError> {
        let identity = identity.into();
        if identity.is_empty() {
            return Err(SemanticRayProbeError::EmptyIdentity);
        }
        Ok(Self {
            identity: Arc::from(identity),
            ray,
        })
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn ray(&self) -> &SemanticRay {
        &self.ray
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticRayProbeObservation {
    probe_identity: Arc<str>,
    observation: SemanticRayObservation,
}

impl SemanticRayProbeObservation {
    pub fn probe_identity(&self) -> &str {
        &self.probe_identity
    }

    pub fn observation(&self) -> &SemanticRayObservation {
        &self.observation
    }
}

#[derive(Debug, Error)]
pub enum SemanticRayError {
    #[error("Semantic Ray origin and direction must be finite")]
    NonFiniteInput,
    #[error("Semantic Ray direction must have nonzero finite length")]
    ZeroLengthDirection,
    #[error("Semantic Ray clip distances must form a finite nonnegative interval")]
    InvalidClipInterval,
}

#[derive(Debug, Error)]
pub enum SemanticRayProbeError {
    #[error("Semantic Ray probe identity must not be empty")]
    EmptyIdentity,
}

#[derive(Debug, Error)]
pub enum SemanticRayDistanceToleranceError {
    #[error("Semantic Ray distance tolerance must be finite and nonnegative")]
    InvalidMaximumDifference,
}

#[derive(Debug, Error)]
pub enum SemanticRayOracleError {
    #[error("could not read the pinned Voxel Scene View")]
    VoxelFrontend(#[from] VoxelFrontendError),
}

pub fn observe(
    view: &VoxelSceneView,
    ray: &SemanticRay,
) -> Result<SemanticRayObservation, SemanticRayOracleError> {
    let mut nearest_contact: Option<SemanticRayContact> = None;
    for volume in view.volumes() {
        let samples = view.read_region(
            volume.identity(),
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), volume.extent()),
        )?;
        for sample in samples {
            let VoxelValue::Occupied(material_identity) = sample.value() else {
                continue;
            };
            let Some(intersection) = intersect_cell(ray, volume, sample.coordinate()) else {
                continue;
            };
            let contact = SemanticRayContact {
                volume_identity: volume.identity().clone(),
                coordinate: sample.coordinate(),
                material_identity: material_identity.clone(),
                distance: intersection.distance,
                classification: intersection.classification,
            };
            if nearest_contact
                .as_ref()
                .is_none_or(|nearest| contact_precedes(&contact, nearest))
            {
                nearest_contact = Some(contact);
            }
        }
    }

    Ok(SemanticRayObservation {
        scene_identity: view.scene_id().clone(),
        revision: view.revision(),
        result: nearest_contact.map_or(SemanticRayResult::Miss, SemanticRayResult::Contact),
    })
}

pub fn observe_probe(
    view: &VoxelSceneView,
    probe: &SemanticRayProbe,
) -> Result<SemanticRayProbeObservation, SemanticRayOracleError> {
    Ok(SemanticRayProbeObservation {
        probe_identity: probe.identity.clone(),
        observation: observe(view, probe.ray())?,
    })
}

fn contact_precedes(candidate: &SemanticRayContact, current: &SemanticRayContact) -> bool {
    match candidate.distance.total_cmp(&current.distance) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Equal => {
            match candidate.volume_identity.cmp(&current.volume_identity) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Equal => matches!(
                    (&candidate.classification, &current.classification),
                    (
                        SemanticRayContactClassification::StartedInside,
                        SemanticRayContactClassification::Entered(_)
                    )
                ),
                std::cmp::Ordering::Greater => false,
            }
        }
        std::cmp::Ordering::Greater => false,
    }
}

struct CellIntersection {
    distance: f64,
    classification: SemanticRayContactClassification,
}

fn intersect_cell(
    ray: &SemanticRay,
    volume: &VoxelVolumeMetadata,
    coordinate: VoxelCoordinate,
) -> Option<CellIntersection> {
    let [coordinate_x, coordinate_y, coordinate_z] = coordinate.components();
    let [scene_origin_x, scene_origin_y, scene_origin_z] = volume.scene_origin().map(f64::from);
    let voxel_size = f64::from(volume.voxel_size());
    let minimum = [
        scene_origin_x + f64::from(coordinate_x) * voxel_size,
        scene_origin_y + f64::from(coordinate_y) * voxel_size,
        scene_origin_z + f64::from(coordinate_z) * voxel_size,
    ];
    let [minimum_x, minimum_y, minimum_z] = minimum;
    let maximum = [
        minimum_x + voxel_size,
        minimum_y + voxel_size,
        minimum_z + voxel_size,
    ];
    let clipped_start = point_at_distance(ray, ray.minimum_distance);
    if half_open_contains(minimum, maximum, clipped_start) {
        return Some(CellIntersection {
            distance: ray.minimum_distance,
            classification: SemanticRayContactClassification::StartedInside,
        });
    }
    let mut entry_distance = f64::NEG_INFINITY;
    let mut exit_distance = f64::INFINITY;
    let mut entry_normal = AxisNormal::NegativeX;

    for axis in Axis::ALL {
        let origin = axis.component(ray.origin);
        let direction = axis.component(ray.direction);
        let axis_minimum = axis.component(minimum);
        let axis_maximum = axis.component(maximum);
        if direction == 0.0 {
            if origin < axis_minimum || origin >= axis_maximum {
                return None;
            }
            continue;
        }
        let minimum_distance = (axis_minimum - origin) / direction;
        let maximum_distance = (axis_maximum - origin) / direction;
        let (axis_entry, axis_exit, normal) = if direction > 0.0 {
            (minimum_distance, maximum_distance, axis.negative_normal())
        } else {
            (maximum_distance, minimum_distance, axis.positive_normal())
        };
        if axis_entry > entry_distance {
            entry_distance = axis_entry;
            entry_normal = normal;
        }
        exit_distance = exit_distance.min(axis_exit);
    }

    if exit_distance <= entry_distance
        || exit_distance <= ray.minimum_distance
        || entry_distance > ray.maximum_distance
    {
        return None;
    }
    Some(CellIntersection {
        distance: entry_distance.max(ray.minimum_distance),
        classification: SemanticRayContactClassification::Entered(entry_normal),
    })
}

fn point_at_distance(ray: &SemanticRay, distance: f64) -> [f64; 3] {
    let [origin_x, origin_y, origin_z] = ray.origin;
    let [direction_x, direction_y, direction_z] = ray.direction;
    [
        origin_x + direction_x * distance,
        origin_y + direction_y * distance,
        origin_z + direction_z * distance,
    ]
}

fn half_open_contains(minimum: [f64; 3], maximum: [f64; 3], point: [f64; 3]) -> bool {
    let [minimum_x, minimum_y, minimum_z] = minimum;
    let [maximum_x, maximum_y, maximum_z] = maximum;
    let [point_x, point_y, point_z] = point;
    point_x >= minimum_x
        && point_x < maximum_x
        && point_y >= minimum_y
        && point_y < maximum_y
        && point_z >= minimum_z
        && point_z < maximum_z
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];

    fn component(self, value: [f64; 3]) -> f64 {
        let [x, y, z] = value;
        match self {
            Self::X => x,
            Self::Y => y,
            Self::Z => z,
        }
    }

    fn negative_normal(self) -> AxisNormal {
        match self {
            Self::X => AxisNormal::NegativeX,
            Self::Y => AxisNormal::NegativeY,
            Self::Z => AxisNormal::NegativeZ,
        }
    }

    fn positive_normal(self) -> AxisNormal {
        match self {
            Self::X => AxisNormal::PositiveX,
            Self::Y => AxisNormal::PositiveY,
            Self::Z => AxisNormal::PositiveZ,
        }
    }
}
