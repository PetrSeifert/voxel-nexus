use std::collections::BTreeMap;
use std::sync::Arc;

use thiserror::Error;
use voxel_frontend::{
    VoxelCoordinate, VoxelExtent, VoxelFrontendError, VoxelMaterialId, VoxelRegion, VoxelSceneId,
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
    #[error("Semantic Ray oracle volume dimensions overflowed")]
    ArithmeticOverflow,
    #[error("Semantic Ray oracle values could not be allocated")]
    Allocation,
    #[error("could not read the pinned Voxel Scene View")]
    VoxelFrontend(#[from] VoxelFrontendError),
}

pub fn observe(
    view: &VoxelSceneView,
    ray: &SemanticRay,
) -> Result<SemanticRayObservation, SemanticRayOracleError> {
    let mut nearest_contact: Option<SemanticRayContact> = None;
    for volume in view.volumes() {
        let [width, height, depth] = volume.extent().dimensions();
        let value_count = [width, height, depth]
            .into_iter()
            .try_fold(1_usize, |count, dimension| {
                count.checked_mul(usize::try_from(dimension).ok()?)
            })
            .ok_or(SemanticRayOracleError::ArithmeticOverflow)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(value_count)
            .map_err(|_| SemanticRayOracleError::Allocation)?;
        values.resize(value_count, VoxelValue::Empty);
        view.read_region_into(
            volume.identity(),
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), volume.extent()),
            &mut values,
        )?;
        let coordinates = (0..depth)
            .flat_map(|z| (0..height).flat_map(move |y| (0..width).map(move |x| (x, y, z))));
        for (value, (x, y, z)) in values.iter().zip(coordinates) {
            let VoxelValue::Occupied(material_identity) = value else {
                continue;
            };
            let coordinate = VoxelCoordinate::new(
                i32::try_from(x).map_err(|_| SemanticRayOracleError::ArithmeticOverflow)?,
                i32::try_from(y).map_err(|_| SemanticRayOracleError::ArithmeticOverflow)?,
                i32::try_from(z).map_err(|_| SemanticRayOracleError::ArithmeticOverflow)?,
            );
            let Some(intersection) = intersect_cell(ray, volume, coordinate) else {
                continue;
            };
            let contact = SemanticRayContact {
                volume_identity: volume.identity().clone(),
                coordinate,
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

/// Produces the same observation as [`observe`] while reading only values near the ray, so the
/// cost follows the traversed distance instead of the volume size.
pub fn observe_along_ray(
    view: &VoxelSceneView,
    ray: &SemanticRay,
) -> Result<SemanticRayObservation, SemanticRayOracleError> {
    let mut nearest_contact: Option<SemanticRayContact> = None;
    for volume in view.volumes() {
        for contact in traversed_contacts(view, volume, ray)? {
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

/// Returns contacts in the same z, y, x order that [`observe`] visits them, so equal-distance
/// precedence resolves identically.
fn traversed_contacts(
    view: &VoxelSceneView,
    volume: &VoxelVolumeMetadata,
    ray: &SemanticRay,
) -> Result<Vec<SemanticRayContact>, SemanticRayOracleError> {
    let dimensions = volume.extent().dimensions().map(i64::from);
    if dimensions.contains(&0) {
        return Ok(Vec::new());
    }
    let scene_origin = volume.scene_origin().map(f64::from);
    let voxel_size = f64::from(volume.voxel_size());
    let Some((entry_distance, exit_distance)) =
        volume_interval(ray, scene_origin, voxel_size, dimensions)
    else {
        return Ok(Vec::new());
    };

    let start = point_at_distance(ray, entry_distance);
    let mut cell = [0_i64; 3];
    for axis in Axis::ALL {
        let local = (axis.component(start) - axis.component(scene_origin)) / voxel_size;
        *axis.cell_component_mut(&mut cell) =
            (local.floor() as i64).clamp(0, axis.cell_component(dimensions) - 1);
    }

    let mut evaluated: BTreeMap<[i64; 3], Option<SemanticRayContact>> = BTreeMap::new();
    let mut values = Vec::new();
    let mut nearest_distance: Option<f64> = None;
    let mut cell_entry_distance = entry_distance;
    let step_limit = dimensions.iter().sum::<i64>() + 3;
    for _ in 0..step_limit {
        // Visited cells are ordered by entry distance, so once they pass the nearest contact by
        // a full voxel no later cell can precede it.
        if cell_entry_distance > exit_distance + voxel_size
            || nearest_distance.is_some_and(|nearest| cell_entry_distance > nearest + voxel_size)
        {
            break;
        }
        // Rounding at shared faces, edges, and corners can make the traversal choose a
        // neighbour of the exact cell, so the whole neighbourhood is evaluated exactly.
        let neighbourhood_nearest =
            evaluate_neighbourhood(view, volume, ray, cell, &mut evaluated, &mut values)?;
        if let Some(distance) = neighbourhood_nearest {
            nearest_distance =
                Some(nearest_distance.map_or(distance, |nearest| nearest.min(distance)));
        }

        let mut next_axis = None;
        let mut next_distance = f64::INFINITY;
        for axis in Axis::ALL {
            let direction = axis.component(ray.direction);
            if direction == 0.0 {
                continue;
            }
            let boundary_cell = axis.cell_component(cell) + i64::from(direction > 0.0);
            let boundary = axis.component(scene_origin) + boundary_cell as f64 * voxel_size;
            let distance = (boundary - axis.component(ray.origin)) / direction;
            if distance < next_distance {
                next_distance = distance;
                next_axis = Some(axis);
            }
        }
        let Some(axis) = next_axis else {
            break;
        };
        let direction_step = if axis.component(ray.direction) > 0.0 {
            1
        } else {
            -1
        };
        let next_cell = axis.cell_component(cell) + direction_step;
        if !(0..axis.cell_component(dimensions)).contains(&next_cell) {
            break;
        }
        *axis.cell_component_mut(&mut cell) = next_cell;
        cell_entry_distance = next_distance;
    }

    Ok(evaluated.into_values().flatten().collect())
}

/// Conservatively bounds the distances at which the ray can touch the volume's cells.
fn volume_interval(
    ray: &SemanticRay,
    scene_origin: [f64; 3],
    voxel_size: f64,
    dimensions: [i64; 3],
) -> Option<(f64, f64)> {
    let mut entry_distance = ray.minimum_distance;
    let mut exit_distance = ray.maximum_distance;
    for axis in Axis::ALL {
        let origin = axis.component(ray.origin);
        let direction = axis.component(ray.direction);
        let minimum = axis.component(scene_origin);
        let maximum = minimum + axis.cell_component(dimensions) as f64 * voxel_size;
        if direction == 0.0 {
            if origin < minimum - voxel_size || origin > maximum + voxel_size {
                return None;
            }
            continue;
        }
        let minimum_distance = (minimum - origin) / direction;
        let maximum_distance = (maximum - origin) / direction;
        entry_distance = entry_distance.max(minimum_distance.min(maximum_distance));
        exit_distance = exit_distance.min(minimum_distance.max(maximum_distance));
    }
    if entry_distance > exit_distance + voxel_size {
        return None;
    }
    Some((entry_distance, exit_distance))
}

fn voxel_coordinate(coordinate: [i64; 3]) -> Result<VoxelCoordinate, SemanticRayOracleError> {
    let [x, y, z] = coordinate.map(|component| {
        i32::try_from(component).map_err(|_| SemanticRayOracleError::ArithmeticOverflow)
    });
    Ok(VoxelCoordinate::new(x?, y?, z?))
}

fn evaluate_neighbourhood(
    view: &VoxelSceneView,
    volume: &VoxelVolumeMetadata,
    ray: &SemanticRay,
    cell: [i64; 3],
    evaluated: &mut BTreeMap<[i64; 3], Option<SemanticRayContact>>,
    values: &mut Vec<VoxelValue>,
) -> Result<Option<f64>, SemanticRayOracleError> {
    let dimensions = volume.extent().dimensions().map(i64::from);
    let mut minimum = [0_i64; 3];
    let mut extent = [0_i64; 3];
    for axis in Axis::ALL {
        let low = (axis.cell_component(cell) - 1).max(0);
        let high = (axis.cell_component(cell) + 1).min(axis.cell_component(dimensions) - 1);
        *axis.cell_component_mut(&mut minimum) = low;
        *axis.cell_component_mut(&mut extent) = high - low + 1;
    }
    let [minimum_x, minimum_y, minimum_z] = minimum;
    let [width, height, depth] = extent.map(|length| {
        u32::try_from(length).map_err(|_| SemanticRayOracleError::ArithmeticOverflow)
    });
    let (width, height, depth) = (width?, height?, depth?);
    let region_origin = voxel_coordinate(minimum)?;
    let value_count = usize::try_from(width * height * depth)
        .map_err(|_| SemanticRayOracleError::ArithmeticOverflow)?;
    values.clear();
    values.resize(value_count, VoxelValue::Empty);
    view.read_region_into(
        volume.identity(),
        VoxelRegion::new(region_origin, VoxelExtent::new(width, height, depth)),
        values,
    )?;

    let mut nearest_distance: Option<f64> = None;
    let offsets =
        (0..depth).flat_map(|z| (0..height).flat_map(move |y| (0..width).map(move |x| (x, y, z))));
    for (value, (offset_x, offset_y, offset_z)) in values.iter().zip(offsets) {
        let coordinate = [
            minimum_x + i64::from(offset_x),
            minimum_y + i64::from(offset_y),
            minimum_z + i64::from(offset_z),
        ];
        let [coordinate_x, coordinate_y, coordinate_z] = coordinate;
        let key = [coordinate_z, coordinate_y, coordinate_x];
        if evaluated.contains_key(&key) {
            continue;
        }
        let contact = match value {
            VoxelValue::Empty => None,
            VoxelValue::Occupied(material_identity) => {
                let coordinate = voxel_coordinate(coordinate)?;
                intersect_cell(ray, volume, coordinate).map(|intersection| SemanticRayContact {
                    volume_identity: volume.identity().clone(),
                    coordinate,
                    material_identity: material_identity.clone(),
                    distance: intersection.distance,
                    classification: intersection.classification,
                })
            }
        };
        if let Some(contact) = &contact {
            nearest_distance = Some(
                nearest_distance.map_or(contact.distance, |nearest| nearest.min(contact.distance)),
            );
        }
        evaluated.insert(key, contact);
    }
    Ok(nearest_distance)
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

    fn cell_component(self, cell: [i64; 3]) -> i64 {
        let [x, y, z] = cell;
        match self {
            Self::X => x,
            Self::Y => y,
            Self::Z => z,
        }
    }

    fn cell_component_mut(self, cell: &mut [i64; 3]) -> &mut i64 {
        let [x, y, z] = cell;
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
