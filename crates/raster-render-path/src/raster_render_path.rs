use ash::vk;
use render_backend::{
    PresentationConfigurationId, RenderPath, RenderPathAttachmentIdentity, RenderPathDeviceContext,
    RenderPathFrameContext, RenderPathResult, RenderPathTarget,
};
use std::fmt;
use std::io::Cursor;
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

const _: () = assert!(size_of::<RasterVertex>() == 10 * size_of::<f32>());

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterArtifactInstallationPhase {
    Upload,
    PresentationConfiguration,
    Record,
}

impl fmt::Display for RasterArtifactInstallationPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Upload => "upload",
            Self::PresentationConfiguration => "presentation configuration",
            Self::Record => "record",
        })
    }
}

#[derive(Debug, Error)]
#[error("raster artifact {phase} failed for Voxel Scene Revision {source_revision}: {source}")]
pub struct RasterArtifactInstallationError {
    phase: RasterArtifactInstallationPhase,
    source_revision: VoxelSceneRevision,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl RasterArtifactInstallationError {
    pub fn new(
        phase: RasterArtifactInstallationPhase,
        source_revision: VoxelSceneRevision,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self {
            phase,
            source_revision,
            source,
        }
    }

    pub fn phase(&self) -> RasterArtifactInstallationPhase {
        self.phase
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }
}

pub struct RasterRenderPath {
    artifact: Option<RasterArtifact>,
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    index_buffer: vk::Buffer,
    index_memory: vk::DeviceMemory,
    depth_image: vk::Image,
    depth_memory: vk::DeviceMemory,
    depth_view: vk::ImageView,
    render_pass: vk::RenderPass,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    framebuffers: Vec<vk::Framebuffer>,
    configured_attachments: Vec<RenderPathAttachmentIdentity>,
    configuration_id: Option<PresentationConfigurationId>,
    camera_constants: [f32; 16],
    index_count: u32,
}

impl Default for RasterRenderPath {
    fn default() -> Self {
        Self {
            artifact: None,
            vertex_buffer: vk::Buffer::null(),
            vertex_memory: vk::DeviceMemory::null(),
            index_buffer: vk::Buffer::null(),
            index_memory: vk::DeviceMemory::null(),
            depth_image: vk::Image::null(),
            depth_memory: vk::DeviceMemory::null(),
            depth_view: vk::ImageView::null(),
            render_pass: vk::RenderPass::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
            framebuffers: Vec::new(),
            configured_attachments: Vec::new(),
            configuration_id: None,
            camera_constants: [0.0; 16],
            index_count: 0,
        }
    }
}

impl RasterRenderPath {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn install_artifact(&mut self, artifact: RasterArtifact) {
        self.artifact = Some(artifact);
    }

    pub fn installed_source_revision(&self) -> Option<VoxelSceneRevision> {
        self.artifact.as_ref().map(RasterArtifact::source_revision)
    }
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

    let mut pending_faces = Vec::new();
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
                pending_faces.try_reserve(1).map_err(|_| {
                    build_error(
                        source_revision,
                        RasterArtifactBuildPhase::FaceExtraction,
                        RasterArtifactBuildCause::AllocationFailed,
                    )
                })?;
                pending_faces.push(PendingFace {
                    coordinate,
                    normal,
                    material_identity: material_identity.clone(),
                    linear_base_color,
                });
            }
        }
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
    let coordinate_x = usize::try_from(coordinate_x).ok()?;
    let coordinate_y = usize::try_from(coordinate_y).ok()?;
    let coordinate_z = usize::try_from(coordinate_z).ok()?;
    let [width, height, depth] = dimensions;
    if coordinate_x >= width || coordinate_y >= height || coordinate_z >= depth {
        return None;
    }
    coordinate_z
        .checked_mul(height)?
        .checked_add(coordinate_y)?
        .checked_mul(width)?
        .checked_add(coordinate_x)
}

fn coordinate_from_index(dimensions: [usize; 3], index: usize) -> Option<VoxelCoordinate> {
    let [width, height, depth] = dimensions;
    let plane_size = width.checked_mul(height)?;
    if plane_size == 0 || index >= plane_size.checked_mul(depth)? {
        return None;
    }
    let coordinate_z = index / plane_size;
    let within_plane = index % plane_size;
    let coordinate_y = within_plane / width;
    let coordinate_x = within_plane % width;
    Some(VoxelCoordinate::new(
        i32::try_from(coordinate_x).ok()?,
        i32::try_from(coordinate_y).ok()?,
        i32::try_from(coordinate_z).ok()?,
    ))
}

fn offset_coordinate(coordinate: VoxelCoordinate, normal: AxisNormal) -> Option<VoxelCoordinate> {
    let [coordinate_x, coordinate_y, coordinate_z] = coordinate.components();
    let [offset_x, offset_y, offset_z] = normal.offset();
    Some(VoxelCoordinate::new(
        coordinate_x.checked_add(offset_x)?,
        coordinate_y.checked_add(offset_y)?,
        coordinate_z.checked_add(offset_z)?,
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
    let [coordinate_x, coordinate_y, coordinate_z] = coordinate.components();
    let [origin_x, origin_y, origin_z] = metadata.scene_origin();
    let minimum_x = scene_component(origin_x, metadata.voxel_size(), coordinate_x)?;
    let minimum_y = scene_component(origin_y, metadata.voxel_size(), coordinate_y)?;
    let minimum_z = scene_component(origin_z, metadata.voxel_size(), coordinate_z)?;
    let maximum_x = scene_component(
        origin_x,
        metadata.voxel_size(),
        coordinate_x.checked_add(1)?,
    )?;
    let maximum_y = scene_component(
        origin_y,
        metadata.voxel_size(),
        coordinate_y.checked_add(1)?,
    )?;
    let maximum_z = scene_component(
        origin_z,
        metadata.voxel_size(),
        coordinate_z.checked_add(1)?,
    )?;
    Some(match normal {
        AxisNormal::NegativeX => [
            [minimum_x, minimum_y, minimum_z],
            [minimum_x, minimum_y, maximum_z],
            [minimum_x, maximum_y, maximum_z],
            [minimum_x, maximum_y, minimum_z],
        ],
        AxisNormal::PositiveX => [
            [maximum_x, minimum_y, minimum_z],
            [maximum_x, maximum_y, minimum_z],
            [maximum_x, maximum_y, maximum_z],
            [maximum_x, minimum_y, maximum_z],
        ],
        AxisNormal::NegativeY => [
            [minimum_x, minimum_y, minimum_z],
            [maximum_x, minimum_y, minimum_z],
            [maximum_x, minimum_y, maximum_z],
            [minimum_x, minimum_y, maximum_z],
        ],
        AxisNormal::PositiveY => [
            [minimum_x, maximum_y, minimum_z],
            [minimum_x, maximum_y, maximum_z],
            [maximum_x, maximum_y, maximum_z],
            [maximum_x, maximum_y, minimum_z],
        ],
        AxisNormal::NegativeZ => [
            [minimum_x, minimum_y, minimum_z],
            [minimum_x, maximum_y, minimum_z],
            [maximum_x, maximum_y, minimum_z],
            [maximum_x, minimum_y, minimum_z],
        ],
        AxisNormal::PositiveZ => [
            [minimum_x, minimum_y, maximum_z],
            [maximum_x, minimum_y, maximum_z],
            [maximum_x, maximum_y, maximum_z],
            [minimum_x, maximum_y, maximum_z],
        ],
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

#[derive(Debug, Error)]
enum RasterResourceError {
    #[error("no complete raster artifact is installed")]
    MissingArtifact,
    #[error("the raster artifact index count cannot be represented for indexed drawing")]
    IndexCount,
    #[error("the Raster Vertex stride cannot be represented for graphics state")]
    VertexStride,
    #[error("the static {0} buffer byte length cannot be represented by Vulkan")]
    BufferSize(&'static str),
    #[error("could not create the static {kind} buffer: {source}")]
    CreateBuffer {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("no host-visible coherent memory type can hold the static {0} buffer")]
    MissingBufferMemory(&'static str),
    #[error("could not allocate the static {kind} buffer memory: {source}")]
    AllocateBufferMemory {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("could not bind the static {kind} buffer memory: {source}")]
    BindBufferMemory {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("could not upload the static {kind} data: {source}")]
    UploadBuffer {
        kind: &'static str,
        source: vk::Result,
    },
    #[error("could not create the depth image: {0}")]
    CreateDepthImage(vk::Result),
    #[error("no device-local memory type can hold the depth image")]
    MissingDepthMemory,
    #[error("could not allocate the depth image memory: {0}")]
    AllocateDepthMemory(vk::Result),
    #[error("could not bind the depth image memory: {0}")]
    BindDepthMemory(vk::Result),
    #[error("could not create the depth image view: {0}")]
    CreateDepthView(vk::Result),
    #[error("could not create the raster render pass: {0}")]
    CreateRenderPass(vk::Result),
    #[error("could not read a reproducibly built raster shader artifact: {0}")]
    ReadShaderArtifact(#[from] std::io::Error),
    #[error("could not create a raster shader module: {0}")]
    CreateShaderModule(vk::Result),
    #[error("could not create the raster graphics pipeline layout: {0}")]
    CreatePipelineLayout(vk::Result),
    #[error("could not create the raster graphics pipeline: {0}")]
    CreateGraphicsPipeline(vk::Result),
    #[error("could not create a raster framebuffer: {0}")]
    CreateFramebuffer(vk::Result),
    #[error("the frame target does not belong to the configured raster presentation target")]
    StaleFrameTarget,
    #[error("the configured raster presentation target has no framebuffer for the acquired image")]
    MissingFramebuffer,
}

impl RasterResourceError {
    fn installation_phase(&self) -> RasterArtifactInstallationPhase {
        match self {
            Self::CreateBuffer { .. }
            | Self::MissingBufferMemory(_)
            | Self::AllocateBufferMemory { .. }
            | Self::BindBufferMemory { .. }
            | Self::UploadBuffer { .. }
            | Self::IndexCount
            | Self::VertexStride
            | Self::BufferSize(_) => RasterArtifactInstallationPhase::Upload,
            _ => RasterArtifactInstallationPhase::PresentationConfiguration,
        }
    }
}

impl RenderPath for RasterRenderPath {
    fn release(&mut self, device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        self.release_resources(&device);
        Ok(())
    }

    fn configure(
        &mut self,
        device: RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        let source_revision = self
            .artifact
            .as_ref()
            .map(RasterArtifact::source_revision)
            .ok_or_else(|| {
                Box::new(RasterResourceError::MissingArtifact)
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        let result = self.configure_resources(&device, target);
        if let Err(error) = result {
            let phase = error.installation_phase();
            self.release_resources(&device);
            return Err(Box::new(RasterArtifactInstallationError::new(
                phase,
                source_revision,
                Box::new(error),
            )));
        }
        Ok(())
    }

    fn record(&mut self, frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        let source_revision = self
            .artifact
            .as_ref()
            .map(RasterArtifact::source_revision)
            .ok_or(RasterResourceError::MissingArtifact)?;
        let result = self.record_frame(&frame);
        result.map_err(|error| {
            Box::new(RasterArtifactInstallationError::new(
                RasterArtifactInstallationPhase::Record,
                source_revision,
                Box::new(error),
            )) as Box<dyn std::error::Error + Send + Sync>
        })
    }
}

impl RasterRenderPath {
    fn configure_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> Result<(), RasterResourceError> {
        let artifact = self
            .artifact
            .as_ref()
            .ok_or(RasterResourceError::MissingArtifact)?;
        self.index_count =
            u32::try_from(artifact.indices().len()).map_err(|_| RasterResourceError::IndexCount)?;
        if !artifact.vertices().is_empty() {
            let vertex = create_static_buffer(
                device,
                raster_vertex_bytes(artifact.vertices()),
                vk::BufferUsageFlags::VERTEX_BUFFER,
                "vertex",
            )?;
            self.vertex_buffer = vertex.buffer;
            self.vertex_memory = vertex.memory;
        }
        if !artifact.indices().is_empty() {
            let index = create_static_buffer(
                device,
                u32_bytes(artifact.indices()),
                vk::BufferUsageFlags::INDEX_BUFFER,
                "index",
            )?;
            self.index_buffer = index.buffer;
            self.index_memory = index.memory;
        }
        self.create_depth_resources(device, target.extent())?;
        self.create_render_pass(device, target.format())?;
        self.create_graphics_pipeline(device, target.extent())?;
        for attachment in target.attachments() {
            let framebuffer = unsafe {
                device.create_framebuffer(
                    self.render_pass,
                    &attachment,
                    self.depth_view,
                    target.extent(),
                )
            }
            .map_err(RasterResourceError::CreateFramebuffer)?;
            self.framebuffers.push(framebuffer);
            self.configured_attachments.push(attachment.identity());
        }
        self.configuration_id = Some(target.configuration_id());
        self.camera_constants = camera_view_projection(target.extent());
        Ok(())
    }

    fn create_depth_resources(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), RasterResourceError> {
        let create_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::D32_SFLOAT)
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        self.depth_image = unsafe { device.create_image(&create_info) }
            .map_err(RasterResourceError::CreateDepthImage)?;
        let requirements = unsafe { device.image_memory_requirements(self.depth_image) };
        let memory_type_index = device
            .memory_type_index(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or(RasterResourceError::MissingDepthMemory)?;
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        self.depth_memory = unsafe { device.allocate_memory(&allocate_info) }
            .map_err(RasterResourceError::AllocateDepthMemory)?;
        unsafe { device.bind_image_memory(self.depth_image, self.depth_memory) }
            .map_err(RasterResourceError::BindDepthMemory)?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(self.depth_image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::D32_SFLOAT)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        self.depth_view = unsafe { device.create_image_view(&view_info) }
            .map_err(RasterResourceError::CreateDepthView)?;
        Ok(())
    }

    fn create_render_pass(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        color_format: vk::Format,
    ) -> Result<(), RasterResourceError> {
        let color = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let depth = vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let color_reference = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let depth_reference = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let color_references = [color_reference];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_references)
            .depth_stencil_attachment(&depth_reference);
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            );
        let attachments = [color, depth];
        let subpasses = [subpass];
        let dependencies = [dependency];
        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        self.render_pass = unsafe { device.create_render_pass(&create_info) }
            .map_err(RasterResourceError::CreateRenderPass)?;
        Ok(())
    }

    fn create_graphics_pipeline(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
    ) -> Result<(), RasterResourceError> {
        let vertex_code = ash::util::read_spv(&mut Cursor::new(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/raster.vert.spv"
        ))))?;
        let fragment_code = ash::util::read_spv(&mut Cursor::new(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/raster.frag.spv"
        ))))?;
        let vertex_module = create_shader_module(device, &vertex_code)?;
        let fragment_module = match create_shader_module(device, &fragment_code) {
            Ok(module) => module,
            Err(error) => {
                unsafe { device.destroy_shader_module(vertex_module) };
                return Err(error);
            }
        };
        let result =
            self.create_pipeline_with_modules(device, extent, vertex_module, fragment_module);
        unsafe {
            device.destroy_shader_module(fragment_module);
            device.destroy_shader_module(vertex_module);
        }
        result
    }

    fn create_pipeline_with_modules(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        extent: vk::Extent2D,
        vertex_module: vk::ShaderModule,
        fragment_module: vk::ShaderModule,
    ) -> Result<(), RasterResourceError> {
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vertex_module)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fragment_module)
                .name(c"main"),
        ];
        let binding = [vk::VertexInputBindingDescription {
            binding: 0,
            stride: u32::try_from(size_of::<RasterVertex>())
                .map_err(|_| RasterResourceError::VertexStride)?,
            input_rate: vk::VertexInputRate::VERTEX,
        }];
        let attributes = [
            vk::VertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                location: 1,
                binding: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 12,
            },
            vk::VertexInputAttributeDescription {
                location: 2,
                binding: 0,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 24,
            },
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding)
            .vertex_attribute_descriptions(&attributes);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewports = [vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        }];
        let scissors = [vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent,
        }];
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewports(&viewports)
            .scissors(&scissors);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS);
        let color_attachment = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_attachment);
        let push_constant_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(64)];
        let layout_info =
            vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_constant_range);
        self.pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info) }
            .map_err(RasterResourceError::CreatePipelineLayout)?;
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend)
            .layout(self.pipeline_layout)
            .render_pass(self.render_pass)
            .subpass(0);
        match unsafe {
            device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info])
        } {
            Ok(mut pipelines) => {
                self.pipeline =
                    pipelines
                        .pop()
                        .ok_or(RasterResourceError::CreateGraphicsPipeline(
                            vk::Result::ERROR_UNKNOWN,
                        ))?;
                Ok(())
            }
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    unsafe { device.destroy_pipeline(pipeline) };
                }
                Err(RasterResourceError::CreateGraphicsPipeline(error))
            }
        }
    }

    fn record_frame(&self, frame: &RenderPathFrameContext<'_>) -> Result<(), RasterResourceError> {
        let target = frame.target();
        if self.configuration_id != Some(target.configuration_id()) {
            return Err(RasterResourceError::StaleFrameTarget);
        }
        let framebuffer_index = self
            .configured_attachments
            .iter()
            .position(|identity| *identity == target.attachment().identity())
            .ok_or(RasterResourceError::MissingFramebuffer)?;
        let framebuffer = self
            .framebuffers
            .get(framebuffer_index)
            .copied()
            .ok_or(RasterResourceError::MissingFramebuffer)?;
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.025, 0.035, 0.06, 1.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        let render_pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: target.extent(),
            })
            .clear_values(&clear_values);
        unsafe {
            frame.begin_render_pass(&render_pass_info, vk::SubpassContents::INLINE);
            frame.bind_pipeline(vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            if self.index_count > 0 {
                frame.bind_vertex_buffer(self.vertex_buffer);
                frame.bind_index_buffer(self.index_buffer);
                frame
                    .push_vertex_constants(self.pipeline_layout, f32_bytes(&self.camera_constants));
                frame.draw_indexed(self.index_count);
            }
            frame.end_render_pass();
        }
        Ok(())
    }

    fn release_resources(&mut self, device: &RenderPathDeviceContext<'_>) {
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                device.destroy_framebuffer(framebuffer);
            }
            if self.pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.pipeline);
                self.pipeline = vk::Pipeline::null();
            }
            if self.pipeline_layout != vk::PipelineLayout::null() {
                device.destroy_pipeline_layout(self.pipeline_layout);
                self.pipeline_layout = vk::PipelineLayout::null();
            }
            if self.render_pass != vk::RenderPass::null() {
                device.destroy_render_pass(self.render_pass);
                self.render_pass = vk::RenderPass::null();
            }
            if self.depth_view != vk::ImageView::null() {
                device.destroy_image_view(self.depth_view);
                self.depth_view = vk::ImageView::null();
            }
            if self.depth_image != vk::Image::null() {
                device.destroy_image(self.depth_image);
                self.depth_image = vk::Image::null();
            }
            if self.depth_memory != vk::DeviceMemory::null() {
                device.free_memory(self.depth_memory);
                self.depth_memory = vk::DeviceMemory::null();
            }
            if self.index_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.index_buffer);
                self.index_buffer = vk::Buffer::null();
            }
            if self.index_memory != vk::DeviceMemory::null() {
                device.free_memory(self.index_memory);
                self.index_memory = vk::DeviceMemory::null();
            }
            if self.vertex_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.vertex_buffer);
                self.vertex_buffer = vk::Buffer::null();
            }
            if self.vertex_memory != vk::DeviceMemory::null() {
                device.free_memory(self.vertex_memory);
                self.vertex_memory = vk::DeviceMemory::null();
            }
        }
        self.configured_attachments.clear();
        self.configuration_id = None;
        self.index_count = 0;
    }
}

struct StaticBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

fn create_static_buffer(
    device: &RenderPathDeviceContext<'_>,
    bytes: &[u8],
    usage: vk::BufferUsageFlags,
    kind: &'static str,
) -> Result<StaticBuffer, RasterResourceError> {
    let size = u64::try_from(bytes.len()).map_err(|_| RasterResourceError::BufferSize(kind))?;
    let create_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&create_info) }
        .map_err(|source| RasterResourceError::CreateBuffer { kind, source })?;
    let requirements = unsafe { device.buffer_memory_requirements(buffer) };
    let memory_type_index = match device.memory_type_index(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    ) {
        Some(index) => index,
        None => {
            unsafe { device.destroy_buffer(buffer) };
            return Err(RasterResourceError::MissingBufferMemory(kind));
        }
    };
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type_index);
    let memory = match unsafe { device.allocate_memory(&allocate_info) } {
        Ok(memory) => memory,
        Err(source) => {
            unsafe { device.destroy_buffer(buffer) };
            return Err(RasterResourceError::AllocateBufferMemory { kind, source });
        }
    };
    if let Err(source) = unsafe { device.bind_buffer_memory(buffer, memory) } {
        unsafe {
            device.free_memory(memory);
            device.destroy_buffer(buffer);
        }
        return Err(RasterResourceError::BindBufferMemory { kind, source });
    }
    if let Err(source) = unsafe { device.write_memory(memory, bytes) } {
        unsafe {
            device.destroy_buffer(buffer);
            device.free_memory(memory);
        }
        return Err(RasterResourceError::UploadBuffer { kind, source });
    }
    Ok(StaticBuffer { buffer, memory })
}

fn create_shader_module(
    device: &RenderPathDeviceContext<'_>,
    code: &[u32],
) -> Result<vk::ShaderModule, RasterResourceError> {
    let create_info = vk::ShaderModuleCreateInfo::default().code(code);
    unsafe { device.create_shader_module(&create_info) }
        .map_err(RasterResourceError::CreateShaderModule)
}

fn raster_vertex_bytes(values: &[RasterVertex]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // RasterVertex is repr(C), contains only f32 arrays, and has no padding at its checked size.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

fn u32_bytes(values: &[u32]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // Every u32 bit pattern is initialized data and valid to read as bytes.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

fn f32_bytes(values: &[f32]) -> &[u8] {
    let byte_length = std::mem::size_of_val(values);
    // Every f32 bit pattern is initialized data and valid to read as bytes.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), byte_length) }
}

fn camera_view_projection(extent: vk::Extent2D) -> [f32; 16] {
    let aspect_ratio = extent.width as f32 / extent.height as f32;
    let projection = perspective(55_f32.to_radians(), aspect_ratio, 0.1, 100.0);
    let view = look_at([5.0, 4.0, 6.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    multiply_matrices(projection, view)
}

fn perspective(field_of_view: f32, aspect_ratio: f32, near: f32, far: f32) -> [f32; 16] {
    let focal_length = 1.0 / (field_of_view * 0.5).tan();
    [
        focal_length / aspect_ratio,
        0.0,
        0.0,
        0.0,
        0.0,
        -focal_length,
        0.0,
        0.0,
        0.0,
        0.0,
        far / (near - far),
        -1.0,
        0.0,
        0.0,
        near * far / (near - far),
        0.0,
    ]
}

fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let forward = normalize(subtract(center, eye));
    let side = normalize(cross(forward, up));
    let upward = cross(side, forward);
    let [side_x, side_y, side_z] = side;
    let [upward_x, upward_y, upward_z] = upward;
    let [forward_x, forward_y, forward_z] = forward;
    [
        side_x,
        upward_x,
        -forward_x,
        0.0,
        side_y,
        upward_y,
        -forward_y,
        0.0,
        side_z,
        upward_z,
        -forward_z,
        0.0,
        -dot(side, eye),
        -dot(upward, eye),
        dot(forward, eye),
        1.0,
    ]
}

fn multiply_matrices(left: [f32; 16], right: [f32; 16]) -> [f32; 16] {
    let mut result = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            let Some(destination) = result.get_mut(column * 4 + row) else {
                continue;
            };
            *destination = (0..4)
                .filter_map(|inner| {
                    let left_value = left.get(inner * 4 + row)?;
                    let right_value = right.get(column * 4 + inner)?;
                    Some(left_value * right_value)
                })
                .sum();
        }
    }
    result
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    let [left_x, left_y, left_z] = left;
    let [right_x, right_y, right_z] = right;
    [left_x - right_x, left_y - right_y, left_z - right_z]
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
