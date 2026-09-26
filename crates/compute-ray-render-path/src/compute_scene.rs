use semantic_ray_oracle::{
    AxisNormal, SemanticRay, SemanticRayContact, SemanticRayContactClassification,
    SemanticRayObservation, SemanticRayResult,
};
use std::collections::HashMap;
use std::num::TryFromIntError;
use thiserror::Error;
use voxel_frontend::{
    VoxelCoordinate, VoxelExtent, VoxelFrontendError, VoxelMaterialId, VoxelRegion, VoxelSceneId,
    VoxelSceneRevision, VoxelSceneView, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

pub(crate) const SCENE_PREFIX_WORD_COUNT: usize = 4;
pub(crate) const VOLUME_HEADER_WORD_COUNT: usize = 8;
pub(crate) const MATERIAL_WORD_COUNT: usize = 4;
const REGION_READ_EDGE: u32 = 32;

#[derive(Clone, Debug, PartialEq)]
pub struct ComputeVolumeHeader {
    identity: VoxelVolumeId,
    scene_origin: [f32; 3],
    voxel_size: f32,
    extent: VoxelExtent,
    voxel_word_offset: u32,
}

impl ComputeVolumeHeader {
    pub fn identity(&self) -> &VoxelVolumeId {
        &self.identity
    }

    pub fn scene_origin(&self) -> [f32; 3] {
        self.scene_origin
    }

    pub fn voxel_size(&self) -> f32 {
        self.voxel_size
    }

    pub fn extent(&self) -> VoxelExtent {
        self.extent
    }

    pub fn voxel_word_offset(&self) -> u32 {
        self.voxel_word_offset
    }

    fn storage_words(&self) -> [u32; VOLUME_HEADER_WORD_COUNT] {
        let [origin_x, origin_y, origin_z] = self.scene_origin;
        let [width, height, depth] = self.extent.dimensions();
        [
            origin_x.to_bits(),
            origin_y.to_bits(),
            origin_z.to_bits(),
            self.voxel_size.to_bits(),
            width,
            height,
            depth,
            self.voxel_word_offset,
        ]
    }
}

#[derive(Clone, Debug)]
pub struct ComputeSceneBundle {
    scene_identity: VoxelSceneId,
    revision: VoxelSceneRevision,
    volume_headers: Vec<ComputeVolumeHeader>,
    material_identities: Vec<VoxelMaterialId>,
    material_words: Vec<u32>,
    voxel_words: Vec<u32>,
    storage_words: Vec<u32>,
}

impl ComputeSceneBundle {
    pub fn from_view(view: &VoxelSceneView) -> Result<Self, ComputeSceneBuildError> {
        Self::from_view_until_cancelled(view, || false)
    }

    pub(crate) fn from_view_until_cancelled(
        view: &VoxelSceneView,
        mut cancellation_requested: impl FnMut() -> bool,
    ) -> Result<Self, ComputeSceneBuildError> {
        Self::from_view_with_block_completion(view, &mut cancellation_requested, || Ok(()))
    }

    pub(crate) fn from_view_with_block_completion(
        view: &VoxelSceneView,
        mut cancellation_requested: impl FnMut() -> bool,
        mut block_completed: impl FnMut() -> Result<(), ComputeSceneBuildError>,
    ) -> Result<Self, ComputeSceneBuildError> {
        let material_count = u32::try_from(view.materials().len())?;
        if material_count == u32::MAX {
            return Err(ComputeSceneBuildError::TooManyMaterials);
        }

        let mut material_indices = HashMap::new();
        material_indices
            .try_reserve(view.materials().len())
            .map_err(|_| ComputeSceneBuildError::Allocation)?;
        let mut material_identities = Vec::new();
        let mut material_words = Vec::new();
        material_identities
            .try_reserve_exact(view.materials().len())
            .map_err(|_| ComputeSceneBuildError::Allocation)?;
        material_words
            .try_reserve_exact(
                view.materials()
                    .len()
                    .checked_mul(MATERIAL_WORD_COUNT)
                    .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?,
            )
            .map_err(|_| ComputeSceneBuildError::Allocation)?;
        for (index, material) in view.materials().iter().enumerate() {
            let local_index = u32::try_from(index)?;
            if material_indices
                .insert(material.identity().clone(), local_index)
                .is_some()
            {
                return Err(ComputeSceneBuildError::DuplicateMaterial(
                    material.identity().clone(),
                ));
            }
            material_identities.push(material.identity().clone());
            material_words.extend(material.linear_base_color().map(f32::to_bits));
        }

        let mut volumes = view.volumes().iter().collect::<Vec<_>>();
        volumes.sort_by(|left, right| left.identity().cmp(right.identity()));
        let mut volume_headers = Vec::new();
        volume_headers
            .try_reserve_exact(volumes.len())
            .map_err(|_| ComputeSceneBuildError::Allocation)?;
        let total_voxel_count = volumes.iter().try_fold(0_usize, |total, volume| {
            total
                .checked_add(extent_value_count(volume.extent())?)
                .ok_or(ComputeSceneBuildError::ArithmeticOverflow)
        })?;
        let mut voxel_words = Vec::new();
        voxel_words
            .try_reserve_exact(total_voxel_count)
            .map_err(|_| ComputeSceneBuildError::Allocation)?;

        for volume in volumes {
            let voxel_word_offset = u32::try_from(voxel_words.len())?;
            let value_count = extent_value_count(volume.extent())?;
            let new_length = voxel_words
                .len()
                .checked_add(value_count)
                .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
            voxel_words.resize(new_length, 0);
            populate_volume_words(
                view,
                volume,
                voxel_word_offset,
                &material_indices,
                &mut voxel_words,
                &mut cancellation_requested,
                &mut block_completed,
            )?;
            volume_headers.push(ComputeVolumeHeader {
                identity: volume.identity().clone(),
                scene_origin: volume.scene_origin(),
                voxel_size: volume.voxel_size(),
                extent: volume.extent(),
                voxel_word_offset,
            });
        }

        let storage_words = pack_storage_words(&volume_headers, &material_words, &voxel_words)?;
        Ok(Self {
            scene_identity: view.scene_id().clone(),
            revision: view.revision(),
            volume_headers,
            material_identities,
            material_words,
            voxel_words,
            storage_words,
        })
    }

    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn revision(&self) -> VoxelSceneRevision {
        self.revision
    }

    pub fn volume_headers(&self) -> &[ComputeVolumeHeader] {
        &self.volume_headers
    }

    pub fn material_identities(&self) -> &[VoxelMaterialId] {
        &self.material_identities
    }

    pub fn material_words(&self) -> &[u32] {
        &self.material_words
    }

    pub fn voxel_words(&self) -> &[u32] {
        &self.voxel_words
    }

    pub fn storage_words(&self) -> &[u32] {
        &self.storage_words
    }

    pub fn observe(&self, ray: &SemanticRay) -> SemanticRayObservation {
        let mut nearest_contact = None;
        for header in &self.volume_headers {
            let Some(contact) = trace_volume(self, header, ray) else {
                continue;
            };
            if nearest_contact
                .as_ref()
                .is_none_or(|nearest| contact_precedes(&contact, nearest))
            {
                nearest_contact = Some(contact);
            }
        }
        SemanticRayObservation::new(
            self.scene_identity.clone(),
            self.revision,
            nearest_contact.map_or(SemanticRayResult::Miss, SemanticRayResult::Contact),
        )
    }
}

#[derive(Debug, Error)]
pub enum ComputeSceneBuildError {
    #[error("compute-owned Voxel Scene preparation was cancelled")]
    Cancelled,
    #[error("could not allocate the compute-owned Voxel Scene representation")]
    Allocation,
    #[error("compute-owned Voxel Scene representation arithmetic overflowed")]
    ArithmeticOverflow,
    #[error("the compute-owned Voxel Scene representation exceeds 32-bit indexing")]
    IndexOverflow(#[from] TryFromIntError),
    #[error("the Voxel Scene has too many Voxel Materials for zero-reserved 32-bit words")]
    TooManyMaterials,
    #[error("the Voxel Scene contains duplicate Voxel Material identity {0:?}")]
    DuplicateMaterial(VoxelMaterialId),
    #[error("Voxel Volume {volume:?} references unknown Voxel Material {material:?}")]
    UnknownMaterial {
        volume: VoxelVolumeId,
        material: VoxelMaterialId,
    },
    #[error("could not read a bounded Voxel Region")]
    VoxelFrontend(#[from] VoxelFrontendError),
    #[error("the compute convergence preparation barrier is unavailable")]
    PreparationBarrier,
    #[error("the compute convergence control state is unavailable during preparation")]
    PreparationControl,
    #[error("injected preparation failure")]
    InjectedPreparationFailure,
}

fn populate_volume_words(
    view: &VoxelSceneView,
    volume: &VoxelVolumeMetadata,
    voxel_word_offset: u32,
    material_indices: &HashMap<VoxelMaterialId, u32>,
    voxel_words: &mut [u32],
    cancellation_requested: &mut impl FnMut() -> bool,
    block_completed: &mut impl FnMut() -> Result<(), ComputeSceneBuildError>,
) -> Result<(), ComputeSceneBuildError> {
    let [width, height, depth] = volume.extent().dimensions();
    let mut values = Vec::new();
    let maximum_count = usize::try_from(REGION_READ_EDGE)?.pow(3);
    values
        .try_reserve_exact(maximum_count)
        .map_err(|_| ComputeSceneBuildError::Allocation)?;
    for origin_z in (0..depth).step_by(REGION_READ_EDGE as usize) {
        for origin_y in (0..height).step_by(REGION_READ_EDGE as usize) {
            for origin_x in (0..width).step_by(REGION_READ_EDGE as usize) {
                if cancellation_requested() {
                    return Err(ComputeSceneBuildError::Cancelled);
                }
                let region = VoxelRegion::new(
                    VoxelCoordinate::new(
                        i32::try_from(origin_x)?,
                        i32::try_from(origin_y)?,
                        i32::try_from(origin_z)?,
                    ),
                    VoxelExtent::new(
                        REGION_READ_EDGE.min(width - origin_x),
                        REGION_READ_EDGE.min(height - origin_y),
                        REGION_READ_EDGE.min(depth - origin_z),
                    ),
                );
                let [region_width, region_height, region_depth] = region.extent().dimensions();
                let region_width = usize::try_from(region_width)?;
                let region_height = usize::try_from(region_height)?;
                let value_count = region_width * region_height * usize::try_from(region_depth)?;
                values.resize(value_count, VoxelValue::Empty);
                view.read_region_into(volume.identity(), region, &mut values)?;
                for (row_index, row) in values.chunks_exact(region_width).enumerate() {
                    let coordinate = VoxelCoordinate::new(
                        i32::try_from(origin_x)?,
                        i32::try_from(origin_y + u32::try_from(row_index % region_height)?)?,
                        i32::try_from(origin_z + u32::try_from(row_index / region_height)?)?,
                    );
                    let local_index = dense_index(volume.extent(), coordinate)
                        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                    let destination_start = usize::try_from(voxel_word_offset)?
                        .checked_add(local_index)
                        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                    let destination_end = destination_start
                        .checked_add(region_width)
                        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                    let destinations = voxel_words
                        .get_mut(destination_start..destination_end)
                        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
                    for (value, destination) in row.iter().zip(destinations) {
                        *destination = match value {
                            VoxelValue::Empty => 0,
                            VoxelValue::Occupied(material_identity) => material_indices
                                .get(material_identity)
                                .copied()
                                .ok_or_else(|| ComputeSceneBuildError::UnknownMaterial {
                                    volume: volume.identity().clone(),
                                    material: material_identity.clone(),
                                })?
                                .checked_add(1)
                                .ok_or(ComputeSceneBuildError::TooManyMaterials)?,
                        };
                    }
                }
                block_completed()?;
            }
        }
    }
    Ok(())
}

fn pack_storage_words(
    headers: &[ComputeVolumeHeader],
    material_words: &[u32],
    voxel_words: &[u32],
) -> Result<Vec<u32>, ComputeSceneBuildError> {
    let header_words = headers
        .len()
        .checked_mul(VOLUME_HEADER_WORD_COUNT)
        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
    let total_words = SCENE_PREFIX_WORD_COUNT
        .checked_add(header_words)
        .and_then(|count| count.checked_add(material_words.len()))
        .and_then(|count| count.checked_add(voxel_words.len()))
        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)?;
    let mut words = Vec::new();
    words
        .try_reserve_exact(total_words)
        .map_err(|_| ComputeSceneBuildError::Allocation)?;
    words.extend([
        u32::try_from(headers.len())?,
        u32::try_from(material_words.len() / MATERIAL_WORD_COUNT)?,
        u32::try_from(SCENE_PREFIX_WORD_COUNT + header_words)?,
        u32::try_from(SCENE_PREFIX_WORD_COUNT + header_words + material_words.len())?,
    ]);
    for header in headers {
        words.extend(header.storage_words());
    }
    words.extend(material_words);
    words.extend(voxel_words);
    Ok(words)
}

fn trace_volume(
    scene: &ComputeSceneBundle,
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
) -> Option<SemanticRayContact> {
    let clipped_start = point_at_distance(ray, ray.minimum_distance());
    let starts_inside_volume = half_open_volume_contains(header, clipped_start);
    let (mut distance, exit_distance, mut normal) = if starts_inside_volume {
        (
            ray.minimum_distance(),
            volume_exit_distance(header, ray)?,
            AxisNormal::NegativeX,
        )
    } else {
        intersect_volume(header, ray)?
    };
    let mut coordinate = volume_coordinate_at(header, ray, distance)?;

    loop {
        let material_word = voxel_word(scene, header, coordinate)?;
        if material_word != 0 {
            let material_index = usize::try_from(material_word.checked_sub(1)?).ok()?;
            let material_identity = scene.material_identities.get(material_index)?.clone();
            let classification = if starts_inside_volume && distance == ray.minimum_distance() {
                SemanticRayContactClassification::StartedInside
            } else {
                SemanticRayContactClassification::Entered(normal)
            };
            return Some(SemanticRayContact::new(
                header.identity.clone(),
                coordinate,
                material_identity,
                distance,
                classification,
            ));
        }

        let (next_distance, tied_axes) = next_cell_crossing(header, ray, coordinate)?;
        if next_distance > exit_distance || next_distance > ray.maximum_distance() {
            return None;
        }
        let first_axis = *tied_axes.first()?;
        normal = first_axis.entered_normal(ray.direction());
        let [mut coordinate_x, mut coordinate_y, mut coordinate_z] = coordinate.components();
        for axis in tied_axes {
            match axis {
                Axis::X => coordinate_x = coordinate_x.checked_add(axis.step(ray.direction()))?,
                Axis::Y => coordinate_y = coordinate_y.checked_add(axis.step(ray.direction()))?,
                Axis::Z => coordinate_z = coordinate_z.checked_add(axis.step(ray.direction()))?,
            }
        }
        coordinate = VoxelCoordinate::new(coordinate_x, coordinate_y, coordinate_z);
        dense_index(header.extent, coordinate)?;
        distance = next_distance;
    }
}

fn contact_precedes(candidate: &SemanticRayContact, current: &SemanticRayContact) -> bool {
    match candidate.distance().total_cmp(&current.distance()) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Equal => candidate.volume_identity() < current.volume_identity(),
        std::cmp::Ordering::Greater => false,
    }
}

fn intersect_volume(
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
) -> Option<(f64, f64, AxisNormal)> {
    let minimum = header.scene_origin.map(f64::from);
    let [width, height, depth] = header.extent.dimensions();
    let voxel_size = f64::from(header.voxel_size);
    let maximum = [
        minimum[0] + f64::from(width) * voxel_size,
        minimum[1] + f64::from(height) * voxel_size,
        minimum[2] + f64::from(depth) * voxel_size,
    ];
    let mut entry_distance = f64::NEG_INFINITY;
    let mut exit_distance = f64::INFINITY;
    let mut entry_normal = AxisNormal::NegativeX;
    for axis in Axis::ALL {
        let origin = axis.component(ray.origin());
        let direction = axis.component(ray.direction());
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
        || exit_distance < ray.minimum_distance()
        || entry_distance > ray.maximum_distance()
    {
        return None;
    }
    Some((
        entry_distance.max(ray.minimum_distance()),
        exit_distance.min(ray.maximum_distance()),
        entry_normal,
    ))
}

fn volume_exit_distance(header: &ComputeVolumeHeader, ray: &SemanticRay) -> Option<f64> {
    intersect_volume_from_inside(header, ray).map(|(_, exit)| exit.min(ray.maximum_distance()))
}

fn intersect_volume_from_inside(
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
) -> Option<(f64, f64)> {
    let minimum = header.scene_origin.map(f64::from);
    let [width, height, depth] = header.extent.dimensions();
    let voxel_size = f64::from(header.voxel_size);
    let maximum = [
        minimum[0] + f64::from(width) * voxel_size,
        minimum[1] + f64::from(height) * voxel_size,
        minimum[2] + f64::from(depth) * voxel_size,
    ];
    let mut exit_distance = f64::INFINITY;
    for axis in Axis::ALL {
        let direction = axis.component(ray.direction());
        if direction == 0.0 {
            continue;
        }
        let boundary = if direction > 0.0 {
            axis.component(maximum)
        } else {
            axis.component(minimum)
        };
        exit_distance = exit_distance.min((boundary - axis.component(ray.origin())) / direction);
    }
    Some((ray.minimum_distance(), exit_distance))
}

fn volume_coordinate_at(
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
    distance: f64,
) -> Option<VoxelCoordinate> {
    let point = point_at_distance(ray, distance);
    let direction = ray.direction();
    let [width, height, depth] = header.extent.dimensions();
    let mut components = [0_i32; 3];
    for (axis, dimension) in Axis::ALL.into_iter().zip([width, height, depth]) {
        let local = (axis.component(point) - axis.component(header.scene_origin.map(f64::from)))
            / f64::from(header.voxel_size);
        let mut coordinate = local.floor();
        if coordinate == f64::from(dimension) && axis.component(direction) < 0.0 {
            coordinate -= 1.0;
        }
        let destination = components.get_mut(axis.index())?;
        *destination = i32::try_from(coordinate as i64).ok()?;
    }
    Some(VoxelCoordinate::new(
        components[0],
        components[1],
        components[2],
    ))
}

fn next_cell_crossing(
    header: &ComputeVolumeHeader,
    ray: &SemanticRay,
    coordinate: VoxelCoordinate,
) -> Option<(f64, Vec<Axis>)> {
    let coordinate = coordinate.components();
    let mut crossings = [f64::INFINITY; 3];
    for axis in Axis::ALL {
        let direction = axis.component(ray.direction());
        if direction == 0.0 {
            continue;
        }
        let local_coordinate = f64::from(*coordinate.get(axis.index())?);
        let boundary_coordinate = if direction > 0.0 {
            local_coordinate + 1.0
        } else {
            local_coordinate
        };
        let boundary = axis.component(header.scene_origin.map(f64::from))
            + boundary_coordinate * f64::from(header.voxel_size);
        crossings[axis.index()] = (boundary - axis.component(ray.origin())) / direction;
    }
    let next_distance = crossings.into_iter().fold(f64::INFINITY, f64::min);
    if !next_distance.is_finite() {
        return None;
    }
    let tolerance = f64::EPSILON * 16.0 * next_distance.abs().max(1.0);
    let tied_axes = Axis::ALL
        .into_iter()
        .filter(|axis| (crossings[axis.index()] - next_distance).abs() <= tolerance)
        .collect::<Vec<_>>();
    Some((next_distance, tied_axes))
}

fn voxel_word(
    scene: &ComputeSceneBundle,
    header: &ComputeVolumeHeader,
    coordinate: VoxelCoordinate,
) -> Option<u32> {
    let index = usize::try_from(header.voxel_word_offset)
        .ok()?
        .checked_add(dense_index(header.extent, coordinate)?)?;
    scene.voxel_words.get(index).copied()
}

fn dense_index(extent: VoxelExtent, coordinate: VoxelCoordinate) -> Option<usize> {
    let [coordinate_x, coordinate_y, coordinate_z] = coordinate.components();
    let x = usize::try_from(coordinate_x).ok()?;
    let y = usize::try_from(coordinate_y).ok()?;
    let z = usize::try_from(coordinate_z).ok()?;
    let [width, height, depth] = extent.dimensions().map(|value| value as usize);
    if x >= width || y >= height || z >= depth {
        return None;
    }
    z.checked_mul(height)?
        .checked_add(y)?
        .checked_mul(width)?
        .checked_add(x)
}

fn extent_value_count(extent: VoxelExtent) -> Result<usize, ComputeSceneBuildError> {
    let [width, height, depth] = extent.dimensions().map(|value| value as usize);
    width
        .checked_mul(height)
        .and_then(|count| count.checked_mul(depth))
        .ok_or(ComputeSceneBuildError::ArithmeticOverflow)
}

fn point_at_distance(ray: &SemanticRay, distance: f64) -> [f64; 3] {
    let [origin_x, origin_y, origin_z] = ray.origin();
    let [direction_x, direction_y, direction_z] = ray.direction();
    [
        origin_x + direction_x * distance,
        origin_y + direction_y * distance,
        origin_z + direction_z * distance,
    ]
}

fn half_open_volume_contains(header: &ComputeVolumeHeader, point: [f64; 3]) -> bool {
    let minimum = header.scene_origin.map(f64::from);
    let [width, height, depth] = header.extent.dimensions();
    let voxel_size = f64::from(header.voxel_size);
    let maximum = [
        minimum[0] + f64::from(width) * voxel_size,
        minimum[1] + f64::from(height) * voxel_size,
        minimum[2] + f64::from(depth) * voxel_size,
    ];
    (0..3).all(|axis| point[axis] >= minimum[axis] && point[axis] < maximum[axis])
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];

    fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }

    fn component(self, value: [f64; 3]) -> f64 {
        value[self.index()]
    }

    fn step(self, direction: [f64; 3]) -> i32 {
        if self.component(direction) > 0.0 {
            1
        } else {
            -1
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

    fn entered_normal(self, direction: [f64; 3]) -> AxisNormal {
        if self.component(direction) > 0.0 {
            self.negative_normal()
        } else {
            self.positive_normal()
        }
    }
}
