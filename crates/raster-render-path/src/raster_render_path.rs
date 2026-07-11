use std::fmt;
use std::mem::size_of;

use thiserror::Error;
use voxel_frontend::{
    VoxelCoordinate, VoxelExtent, VoxelFrontendError, VoxelMaterialId, VoxelRegion,
    VoxelSceneRevision, VoxelSceneView, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AxisNormal {
    NegativeX,
    PositiveX,
    NegativeY,
    PositiveY,
    NegativeZ,
    PositiveZ,
}

impl AxisNormal {
    pub fn vector(self) -> [f32; 3] {
        match self {
            Self::NegativeX => [-1.0, 0.0, 0.0],
            Self::PositiveX => [1.0, 0.0, 0.0],
            Self::NegativeY => [0.0, -1.0, 0.0],
            Self::PositiveY => [0.0, 1.0, 0.0],
            Self::NegativeZ => [0.0, 0.0, -1.0],
            Self::PositiveZ => [0.0, 0.0, 1.0],
        }
    }

    fn offset(self) -> [i32; 3] {
        match self {
            Self::NegativeX => [-1, 0, 0],
            Self::PositiveX => [1, 0, 0],
            Self::NegativeY => [0, -1, 0],
            Self::PositiveY => [0, 1, 0],
            Self::NegativeZ => [0, 0, -1],
            Self::PositiveZ => [0, 0, 1],
        }
    }
}

const AXIS_NORMALS: [AxisNormal; 6] = [
    AxisNormal::NegativeX,
    AxisNormal::PositiveX,
    AxisNormal::NegativeY,
    AxisNormal::PositiveY,
    AxisNormal::NegativeZ,
    AxisNormal::PositiveZ,
];

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SemanticFace {
    volume_identity: VoxelVolumeId,
    occupied_coordinate: VoxelCoordinate,
    outward_normal: AxisNormal,
    material_identity: VoxelMaterialId,
}

impl SemanticFace {
    pub fn new(
        volume_identity: VoxelVolumeId,
        occupied_coordinate: VoxelCoordinate,
        outward_normal: AxisNormal,
        material_identity: VoxelMaterialId,
    ) -> Self {
        Self {
            volume_identity,
            occupied_coordinate,
            outward_normal,
            material_identity,
        }
    }

    pub fn volume_identity(&self) -> &VoxelVolumeId {
        &self.volume_identity
    }

    pub fn occupied_coordinate(&self) -> VoxelCoordinate {
        self.occupied_coordinate
    }

    pub fn outward_normal(&self) -> AxisNormal {
        self.outward_normal
    }

    pub fn material_identity(&self) -> &VoxelMaterialId {
        &self.material_identity
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterVertex {
    position: [f32; 3],
    normal: [f32; 3],
    linear_base_color: [f32; 4],
}

impl RasterVertex {
    pub fn position(&self) -> [f32; 3] {
        self.position
    }

    pub fn normal(&self) -> [f32; 3] {
        self.normal
    }

    pub fn linear_base_color(&self) -> [f32; 4] {
        self.linear_base_color
    }
}

#[derive(Clone, Debug)]
pub struct RasterArtifact {
    source_revision: VoxelSceneRevision,
    volume_identity: VoxelVolumeId,
    vertices: Vec<RasterVertex>,
    indices: Vec<u32>,
    semantic_faces: Vec<SemanticFace>,
    vertex_byte_size: usize,
    index_byte_size: usize,
}

impl RasterArtifact {
    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }

    pub fn volume_identity(&self) -> &VoxelVolumeId {
        &self.volume_identity
    }

    pub fn vertices(&self) -> &[RasterVertex] {
        &self.vertices
    }

    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    pub fn semantic_faces(&self) -> &[SemanticFace] {
        &self.semantic_faces
    }

    pub fn quad_vertices(&self, face: &SemanticFace) -> Option<&[RasterVertex]> {
        let face_index = self
            .semantic_faces
            .iter()
            .position(|candidate| candidate == face)?;
        let start = face_index.checked_mul(4)?;
        let end = start.checked_add(4)?;
        self.vertices.get(start..end)
    }

    pub fn vertex_byte_size(&self) -> usize {
        self.vertex_byte_size
    }

    pub fn index_byte_size(&self) -> usize {
        self.index_byte_size
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterArtifactBuildPhase {
    Metadata,
    MaterialResolution,
    VoxelRead,
    FaceExtraction,
    Geometry,
}

impl fmt::Display for RasterArtifactBuildPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Metadata => "metadata",
            Self::MaterialResolution => "material resolution",
            Self::VoxelRead => "Voxel Region read",
            Self::FaceExtraction => "face extraction",
            Self::Geometry => "geometry",
        };
        formatter.write_str(name)
    }
}

#[derive(Debug, Error)]
pub enum RasterArtifactBuildCause {
    #[error("unknown Voxel Volume identity {0:?}")]
    UnknownVolume(VoxelVolumeId),
    #[error("Voxel Volume dimensions cannot be represented as logical coordinates")]
    UnrepresentableVolumeDimensions,
    #[error("count or byte-size arithmetic overflow")]
    ArithmeticOverflow,
    #[error("memory allocation failed")]
    AllocationFailed,
    #[error("logical Voxel Region read failed: {0}")]
    VoxelRead(#[source] VoxelFrontendError),
    #[error("logical Voxel Region read returned an invalid or incomplete volume")]
    InvalidVoxelRead,
    #[error("occupied coordinate references unknown Voxel Material {0:?}")]
    UnknownMaterial(VoxelMaterialId),
    #[error("scene-space coordinate transform produced a non-finite value")]
    InvalidSceneTransform,
    #[error("geometry needs an index that cannot be represented as u32")]
    IndexOverflow,
}

#[derive(Debug, Error)]
#[error("raster artifact {phase} failed for Voxel Scene Revision {source_revision:?}: {cause}")]
pub struct RasterArtifactBuildError {
    phase: RasterArtifactBuildPhase,
    source_revision: VoxelSceneRevision,
    #[source]
    cause: RasterArtifactBuildCause,
}

impl RasterArtifactBuildError {
    pub fn phase(&self) -> RasterArtifactBuildPhase {
        self.phase
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }

    pub fn cause_detail(&self) -> &RasterArtifactBuildCause {
        &self.cause
    }
}

struct PendingFace {
    coordinate: VoxelCoordinate,
    normal: AxisNormal,
    material_identity: VoxelMaterialId,
    linear_base_color: [f32; 4],
}

pub fn derive_raster_artifact(
    view: &VoxelSceneView,
    volume_identity: &VoxelVolumeId,
) -> Result<RasterArtifact, RasterArtifactBuildError> {
    let source_revision = view.revision();
    let metadata = view
        .volumes()
        .iter()
        .find(|metadata| metadata.identity() == volume_identity)
        .ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::Metadata,
                RasterArtifactBuildCause::UnknownVolume(volume_identity.clone()),
            )
        })?;
    let dimensions = checked_dimensions(metadata.extent()).ok_or_else(|| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::Metadata,
            RasterArtifactBuildCause::UnrepresentableVolumeDimensions,
        )
    })?;
    let value_count = dimensions
        .iter()
        .try_fold(1_usize, |count, dimension| count.checked_mul(*dimension))
        .ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::Metadata,
                RasterArtifactBuildCause::ArithmeticOverflow,
            )
        })?;

    let mut values = Vec::new();
    values.try_reserve_exact(value_count).map_err(|_| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::VoxelRead,
            RasterArtifactBuildCause::AllocationFailed,
        )
    })?;
    values.resize(value_count, None);
    let samples = view
        .read_region(
            volume_identity,
            VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), metadata.extent()),
        )
        .map_err(|error| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::VoxelRead,
                RasterArtifactBuildCause::VoxelRead(error),
            )
        })?;
    if samples.len() != value_count {
        return Err(build_error(
            source_revision,
            RasterArtifactBuildPhase::VoxelRead,
            RasterArtifactBuildCause::InvalidVoxelRead,
        ));
    }
    for sample in samples {
        let index = dense_index(dimensions, sample.coordinate()).ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::VoxelRead,
                RasterArtifactBuildCause::InvalidVoxelRead,
            )
        })?;
        let destination = values.get_mut(index).ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::VoxelRead,
                RasterArtifactBuildCause::InvalidVoxelRead,
            )
        })?;
        if destination.is_some() {
            return Err(build_error(
                source_revision,
                RasterArtifactBuildPhase::VoxelRead,
                RasterArtifactBuildCause::InvalidVoxelRead,
            ));
        }
        *destination = Some(sample.value().clone());
    }
    if values.iter().any(Option::is_none) {
        return Err(build_error(
            source_revision,
            RasterArtifactBuildPhase::VoxelRead,
            RasterArtifactBuildCause::InvalidVoxelRead,
        ));
    }

    let mut face_count = 0_usize;
    for (index, value) in values.iter().enumerate() {
        if !matches!(value, Some(VoxelValue::Occupied(_))) {
            continue;
        }
        let coordinate = coordinate_from_index(dimensions, index).ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::FaceExtraction,
                RasterArtifactBuildCause::ArithmeticOverflow,
            )
        })?;
        for normal in AXIS_NORMALS {
            if face_is_exposed(&values, dimensions, coordinate, normal) {
                face_count = face_count.checked_add(1).ok_or_else(|| {
                    build_error(
                        source_revision,
                        RasterArtifactBuildPhase::FaceExtraction,
                        RasterArtifactBuildCause::ArithmeticOverflow,
                    )
                })?;
            }
        }
    }
    let mut pending_faces = Vec::new();
    pending_faces.try_reserve_exact(face_count).map_err(|_| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::FaceExtraction,
            RasterArtifactBuildCause::AllocationFailed,
        )
    })?;
    for (index, value) in values.iter().enumerate() {
        let Some(VoxelValue::Occupied(material_identity)) = value else {
            continue;
        };
        let coordinate = coordinate_from_index(dimensions, index).ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::FaceExtraction,
                RasterArtifactBuildCause::ArithmeticOverflow,
            )
        })?;
        let linear_base_color = view
            .materials()
            .iter()
            .find(|material| material.identity() == material_identity)
            .map(|material| material.linear_base_color())
            .ok_or_else(|| {
                build_error(
                    source_revision,
                    RasterArtifactBuildPhase::MaterialResolution,
                    RasterArtifactBuildCause::UnknownMaterial(material_identity.clone()),
                )
            })?;
        for normal in AXIS_NORMALS {
            if face_is_exposed(&values, dimensions, coordinate, normal) {
                pending_faces.push(PendingFace {
                    coordinate,
                    normal,
                    material_identity: material_identity.clone(),
                    linear_base_color,
                });
            }
        }
    }
    if pending_faces.len() != face_count {
        return Err(build_error(
            source_revision,
            RasterArtifactBuildPhase::FaceExtraction,
            RasterArtifactBuildCause::ArithmeticOverflow,
        ));
    }

    build_geometry(source_revision, volume_identity, metadata, pending_faces)
}

fn build_geometry(
    source_revision: VoxelSceneRevision,
    volume_identity: &VoxelVolumeId,
    metadata: &VoxelVolumeMetadata,
    pending_faces: Vec<PendingFace>,
) -> Result<RasterArtifact, RasterArtifactBuildError> {
    let face_count = pending_faces.len();
    let vertex_count = face_count
        .checked_mul(4)
        .ok_or_else(|| geometry_overflow(source_revision))?;
    let index_count = face_count
        .checked_mul(6)
        .ok_or_else(|| geometry_overflow(source_revision))?;
    let vertex_byte_size = vertex_count
        .checked_mul(size_of::<RasterVertex>())
        .ok_or_else(|| geometry_overflow(source_revision))?;
    let index_byte_size = index_count
        .checked_mul(size_of::<u32>())
        .ok_or_else(|| geometry_overflow(source_revision))?;
    u32::try_from(vertex_count).map_err(|_| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::Geometry,
            RasterArtifactBuildCause::IndexOverflow,
        )
    })?;

    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(vertex_count)
        .map_err(|_| geometry_allocation(source_revision))?;
    let mut indices = Vec::new();
    indices
        .try_reserve_exact(index_count)
        .map_err(|_| geometry_allocation(source_revision))?;
    let mut semantic_faces = Vec::new();
    semantic_faces
        .try_reserve_exact(face_count)
        .map_err(|_| geometry_allocation(source_revision))?;

    for pending_face in pending_faces {
        let first_vertex = u32::try_from(vertices.len()).map_err(|_| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::Geometry,
                RasterArtifactBuildCause::IndexOverflow,
            )
        })?;
        let positions = face_positions(metadata, pending_face.coordinate, pending_face.normal)
            .ok_or_else(|| {
                build_error(
                    source_revision,
                    RasterArtifactBuildPhase::Geometry,
                    RasterArtifactBuildCause::InvalidSceneTransform,
                )
            })?;
        for position in positions {
            vertices.push(RasterVertex {
                position,
                normal: pending_face.normal.vector(),
                linear_base_color: pending_face.linear_base_color,
            });
        }
        for local_index in [0_u32, 1, 2, 0, 2, 3] {
            indices.push(first_vertex.checked_add(local_index).ok_or_else(|| {
                build_error(
                    source_revision,
                    RasterArtifactBuildPhase::Geometry,
                    RasterArtifactBuildCause::IndexOverflow,
                )
            })?);
        }
        semantic_faces.push(SemanticFace::new(
            volume_identity.clone(),
            pending_face.coordinate,
            pending_face.normal,
            pending_face.material_identity,
        ));
    }

    Ok(RasterArtifact {
        source_revision,
        volume_identity: volume_identity.clone(),
        vertices,
        indices,
        semantic_faces,
        vertex_byte_size,
        index_byte_size,
    })
}

fn checked_dimensions(extent: VoxelExtent) -> Option<[usize; 3]> {
    let [width, height, depth] = extent.dimensions();
    if width > i32::MAX as u32 || height > i32::MAX as u32 || depth > i32::MAX as u32 {
        return None;
    }
    Some([
        usize::try_from(width).ok()?,
        usize::try_from(height).ok()?,
        usize::try_from(depth).ok()?,
    ])
}

fn dense_index(dimensions: [usize; 3], coordinate: VoxelCoordinate) -> Option<usize> {
    let [coordinate_x, coordinate_y, coordinate_z] = coordinate.components();
    let x = usize::try_from(coordinate_x).ok()?;
    let y = usize::try_from(coordinate_y).ok()?;
    let z = usize::try_from(coordinate_z).ok()?;
    let [width, height, depth] = dimensions;
    if x >= width || y >= height || z >= depth {
        return None;
    }
    z.checked_mul(height)?
        .checked_add(y)?
        .checked_mul(width)?
        .checked_add(x)
}

fn coordinate_from_index(dimensions: [usize; 3], index: usize) -> Option<VoxelCoordinate> {
    let [width, height, depth] = dimensions;
    let plane_size = width.checked_mul(height)?;
    if plane_size == 0 || index >= plane_size.checked_mul(depth)? {
        return None;
    }
    let z = index / plane_size;
    let within_plane = index % plane_size;
    let y = within_plane / width;
    let x = within_plane % width;
    Some(VoxelCoordinate::new(
        i32::try_from(x).ok()?,
        i32::try_from(y).ok()?,
        i32::try_from(z).ok()?,
    ))
}

fn offset_coordinate(coordinate: VoxelCoordinate, normal: AxisNormal) -> Option<VoxelCoordinate> {
    let [x, y, z] = coordinate.components();
    let [offset_x, offset_y, offset_z] = normal.offset();
    Some(VoxelCoordinate::new(
        x.checked_add(offset_x)?,
        y.checked_add(offset_y)?,
        z.checked_add(offset_z)?,
    ))
}

fn face_is_exposed(
    values: &[Option<VoxelValue>],
    dimensions: [usize; 3],
    coordinate: VoxelCoordinate,
    normal: AxisNormal,
) -> bool {
    !offset_coordinate(coordinate, normal)
        .and_then(|neighbor| dense_index(dimensions, neighbor))
        .and_then(|neighbor_index| values.get(neighbor_index))
        .is_some_and(|value| matches!(value, Some(VoxelValue::Occupied(_))))
}

fn face_positions(
    metadata: &VoxelVolumeMetadata,
    coordinate: VoxelCoordinate,
    normal: AxisNormal,
) -> Option<[[f32; 3]; 4]> {
    let [x, y, z] = coordinate.components();
    let [origin_x, origin_y, origin_z] = metadata.scene_origin();
    let x0 = scene_component(origin_x, metadata.voxel_size(), x)?;
    let y0 = scene_component(origin_y, metadata.voxel_size(), y)?;
    let z0 = scene_component(origin_z, metadata.voxel_size(), z)?;
    let x1 = scene_component(origin_x, metadata.voxel_size(), x.checked_add(1)?)?;
    let y1 = scene_component(origin_y, metadata.voxel_size(), y.checked_add(1)?)?;
    let z1 = scene_component(origin_z, metadata.voxel_size(), z.checked_add(1)?)?;
    Some(match normal {
        AxisNormal::NegativeX => [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
        AxisNormal::PositiveX => [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
        AxisNormal::NegativeY => [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
        AxisNormal::PositiveY => [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
        AxisNormal::NegativeZ => [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
        AxisNormal::PositiveZ => [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
    })
}

fn scene_component(origin: f32, voxel_size: f32, coordinate: i32) -> Option<f32> {
    let value = origin + coordinate as f32 * voxel_size;
    value.is_finite().then_some(value)
}

fn build_error(
    source_revision: VoxelSceneRevision,
    phase: RasterArtifactBuildPhase,
    cause: RasterArtifactBuildCause,
) -> RasterArtifactBuildError {
    RasterArtifactBuildError {
        phase,
        source_revision,
        cause,
    }
}

fn geometry_overflow(source_revision: VoxelSceneRevision) -> RasterArtifactBuildError {
    build_error(
        source_revision,
        RasterArtifactBuildPhase::Geometry,
        RasterArtifactBuildCause::ArithmeticOverflow,
    )
}

fn geometry_allocation(source_revision: VoxelSceneRevision) -> RasterArtifactBuildError {
    build_error(
        source_revision,
        RasterArtifactBuildPhase::Geometry,
        RasterArtifactBuildCause::AllocationFailed,
    )
}
