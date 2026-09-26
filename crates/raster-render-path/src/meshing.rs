use super::preparation::raster_region_origin;
use super::semantic_faces::{AXIS_NORMALS, AxisNormal, SemanticFace};
use std::collections::HashSet;
use std::fmt;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
use voxel_frontend::{
    VoxelChangeSet, VoxelCoordinate, VoxelExtent, VoxelFrontendError, VoxelMaterialId, VoxelRegion,
    VoxelSceneId, VoxelSceneRevision, VoxelSceneView, VoxelValue, VoxelVolumeId,
    VoxelVolumeMetadata,
};

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
    pub(super) scene_identity: VoxelSceneId,
    source_revision: VoxelSceneRevision,
    pub(super) region_extent: Option<VoxelExtent>,
    volume_identity: Option<VoxelVolumeId>,
    vertex_byte_size: usize,
    index_byte_size: usize,
    regions: Vec<RasterRegionResult>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RasterRegionIdentity {
    pub(super) volume_identity: VoxelVolumeId,
    pub(super) core_origin: VoxelCoordinate,
}

impl RasterRegionIdentity {
    pub fn volume_identity(&self) -> &VoxelVolumeId {
        &self.volume_identity
    }

    pub fn core_origin(&self) -> VoxelCoordinate {
        self.core_origin
    }
}

#[derive(Clone, Debug)]
pub struct RasterRegionResult {
    identity: RasterRegionIdentity,
    core: VoxelRegion,
    source_revision: VoxelSceneRevision,
    pub(super) geometry: Arc<RasterGeometry>,
}

#[derive(Debug)]
pub struct RasterGeometry {
    vertices: Vec<RasterVertex>,
    indices: Vec<u32>,
    semantic_faces: Vec<SemanticFace>,
}

impl RasterGeometry {
    pub fn vertices(&self) -> &[RasterVertex] {
        &self.vertices
    }

    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    pub fn semantic_faces(&self) -> &[SemanticFace] {
        &self.semantic_faces
    }
}

impl RasterRegionResult {
    pub fn identity(&self) -> &RasterRegionIdentity {
        &self.identity
    }

    pub fn core(&self) -> VoxelRegion {
        self.core
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }

    pub fn vertices(&self) -> &[RasterVertex] {
        &self.geometry.vertices
    }

    pub fn indices(&self) -> &[u32] {
        &self.geometry.indices
    }

    pub fn semantic_faces(&self) -> &[SemanticFace] {
        &self.geometry.semantic_faces
    }

    pub fn is_empty(&self) -> bool {
        self.geometry.semantic_faces.is_empty()
    }
}

impl RasterArtifact {
    pub fn scene_identity(&self) -> &VoxelSceneId {
        &self.scene_identity
    }

    pub fn source_revision(&self) -> VoxelSceneRevision {
        self.source_revision
    }

    pub fn volume_identity(&self) -> Option<&VoxelVolumeId> {
        self.volume_identity.as_ref()
    }

    pub fn vertex_count(&self) -> usize {
        self.vertex_byte_size / size_of::<RasterVertex>()
    }

    pub fn index_count(&self) -> usize {
        self.index_byte_size / size_of::<u32>()
    }

    pub fn semantic_face_count(&self) -> usize {
        self.vertex_count() / 4
    }

    pub fn semantic_faces(&self) -> impl Iterator<Item = &SemanticFace> {
        self.regions
            .iter()
            .flat_map(|region| region.semantic_faces())
    }

    pub fn quad_vertices(&self, face: &SemanticFace) -> Option<&[RasterVertex]> {
        self.regions.iter().find_map(|region| {
            let face_index = region
                .semantic_faces()
                .iter()
                .position(|candidate| candidate == face)?;
            let start = face_index.checked_mul(4)?;
            let end = start.checked_add(4)?;
            region.vertices().get(start..end)
        })
    }

    pub fn flatten_geometry(&self) -> Result<RasterGeometry, RasterArtifactBuildError> {
        let source_revision = self.source_revision;
        u32::try_from(self.vertex_count()).map_err(|_| geometry_overflow(source_revision))?;
        let mut geometry = RasterGeometry {
            vertices: Vec::new(),
            indices: Vec::new(),
            semantic_faces: Vec::new(),
        };
        geometry
            .vertices
            .try_reserve_exact(self.vertex_count())
            .map_err(|_| geometry_allocation(source_revision))?;
        geometry
            .indices
            .try_reserve_exact(self.index_count())
            .map_err(|_| geometry_allocation(source_revision))?;
        geometry
            .semantic_faces
            .try_reserve_exact(self.semantic_face_count())
            .map_err(|_| geometry_allocation(source_revision))?;
        for region in &self.regions {
            let first_vertex = u32::try_from(geometry.vertices.len())
                .map_err(|_| geometry_overflow(source_revision))?;
            geometry.vertices.extend_from_slice(region.vertices());
            for index in region.indices() {
                geometry.indices.push(
                    first_vertex
                        .checked_add(*index)
                        .ok_or_else(|| geometry_overflow(source_revision))?,
                );
            }
            geometry
                .semantic_faces
                .extend_from_slice(region.semantic_faces());
        }
        Ok(geometry)
    }

    pub fn vertex_byte_size(&self) -> usize {
        self.vertex_byte_size
    }

    pub fn index_byte_size(&self) -> usize {
        self.index_byte_size
    }

    pub fn regions(&self) -> &[RasterRegionResult] {
        &self.regions
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
    #[error("occupied coordinate references unknown Voxel Material {0:?}")]
    UnknownMaterial(VoxelMaterialId),
    #[error("scene-space coordinate transform produced a non-finite value")]
    InvalidSceneTransform,
    #[error("geometry needs an index that cannot be represented as u32")]
    IndexOverflow,
    #[error("Raster Region extent must be non-empty")]
    EmptyRasterRegionExtent,
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

pub(super) struct PendingFace {
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
    values.resize(value_count, VoxelValue::Empty);
    view.read_region_into(
        volume_identity,
        VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), metadata.extent()),
        &mut values,
    )
    .map_err(|error| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::VoxelRead,
            RasterArtifactBuildCause::VoxelRead(error),
        )
    })?;

    let mut pending_faces = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let VoxelValue::Occupied(material_identity) = value else {
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
            .material(material_identity)
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

    build_geometry(
        view.scene_id(),
        source_revision,
        volume_identity,
        metadata,
        pending_faces,
    )
}

pub fn derive_raster_regions(
    view: &VoxelSceneView,
    region_extent: VoxelExtent,
) -> Result<RasterArtifact, RasterArtifactBuildError> {
    let mut regions = Vec::new();
    visit_raster_region_cores(view, region_extent, |metadata, core| {
        regions.push(derive_raster_region(view, metadata, core)?);
        Ok(true)
    })?;
    assemble_derived_raster_regions(view, region_extent, regions)
}

fn assemble_derived_raster_regions(
    view: &VoxelSceneView,
    region_extent: VoxelExtent,
    regions: Vec<RasterRegionResult>,
) -> Result<RasterArtifact, RasterArtifactBuildError> {
    assemble_raster_artifact(
        view.scene_id().clone(),
        view.revision(),
        region_extent,
        regions,
    )
}

pub(super) fn derive_raster_regions_until_cancelled(
    view: &VoxelSceneView,
    region_extent: VoxelExtent,
    cancellation: &AtomicBool,
) -> Result<Option<RasterArtifact>, RasterArtifactBuildError> {
    let mut regions = Vec::new();
    let completed = visit_raster_region_cores(view, region_extent, |metadata, core| {
        if cancellation.load(Ordering::Acquire) {
            return Ok(false);
        }
        regions.push(derive_raster_region(view, metadata, core)?);
        Ok(!cancellation.load(Ordering::Acquire))
    })?;
    if !completed {
        return Ok(None);
    }
    assemble_derived_raster_regions(view, region_extent, regions).map(Some)
}

pub(super) fn visit_raster_region_cores(
    view: &VoxelSceneView,
    region_extent: VoxelExtent,
    mut visit: impl FnMut(&VoxelVolumeMetadata, VoxelRegion) -> Result<bool, RasterArtifactBuildError>,
) -> Result<bool, RasterArtifactBuildError> {
    let source_revision = view.revision();
    let [region_width, region_height, region_depth] = region_extent.dimensions();
    if region_width == 0 || region_height == 0 || region_depth == 0 {
        return Err(build_error(
            source_revision,
            RasterArtifactBuildPhase::Metadata,
            RasterArtifactBuildCause::EmptyRasterRegionExtent,
        ));
    }
    for metadata in view.volumes() {
        let [volume_width, volume_height, volume_depth] = metadata.extent().dimensions();
        checked_dimensions(metadata.extent()).ok_or_else(|| {
            build_error(
                source_revision,
                RasterArtifactBuildPhase::Metadata,
                RasterArtifactBuildCause::UnrepresentableVolumeDimensions,
            )
        })?;
        let mut origin_z = 0_u32;
        while origin_z < volume_depth {
            let mut origin_y = 0_u32;
            while origin_y < volume_height {
                let mut origin_x = 0_u32;
                while origin_x < volume_width {
                    let core = VoxelRegion::new(
                        raster_region_origin(source_revision, origin_x, origin_y, origin_z)?,
                        VoxelExtent::new(
                            region_width.min(volume_width - origin_x),
                            region_height.min(volume_height - origin_y),
                            region_depth.min(volume_depth - origin_z),
                        ),
                    );
                    if !visit(metadata, core)? {
                        return Ok(false);
                    }
                    origin_x = origin_x
                        .checked_add(region_width)
                        .ok_or_else(|| metadata_dimensions_error(source_revision))?;
                }
                origin_y = origin_y
                    .checked_add(region_height)
                    .ok_or_else(|| metadata_dimensions_error(source_revision))?;
            }
            origin_z = origin_z
                .checked_add(region_depth)
                .ok_or_else(|| metadata_dimensions_error(source_revision))?;
        }
    }
    Ok(true)
}

pub(super) fn affected_raster_region_identities(
    view: &VoxelSceneView,
    change_set: &VoxelChangeSet,
    region_extent: VoxelExtent,
) -> Result<HashSet<RasterRegionIdentity>, RasterArtifactBuildError> {
    let source_revision = view.revision();
    let [region_width, region_height, region_depth] = region_extent.dimensions();
    if region_width == 0 || region_height == 0 || region_depth == 0 {
        return Err(build_error(
            source_revision,
            RasterArtifactBuildPhase::Metadata,
            RasterArtifactBuildCause::EmptyRasterRegionExtent,
        ));
    }
    let region_dimensions = [region_width, region_height, region_depth];
    let mut affected = HashSet::new();
    for changed_region in change_set.changed_regions() {
        let metadata = view
            .volumes()
            .iter()
            .find(|metadata| metadata.identity() == changed_region.volume_identity())
            .ok_or_else(|| {
                build_error(
                    source_revision,
                    RasterArtifactBuildPhase::Metadata,
                    RasterArtifactBuildCause::UnknownVolume(
                        changed_region.volume_identity().clone(),
                    ),
                )
            })?;
        let [origin_x, origin_y, origin_z] = changed_region.region().origin().components();
        let [width, height, depth] = changed_region.region().extent().dimensions();
        for z_offset in 0..depth {
            for y_offset in 0..height {
                for x_offset in 0..width {
                    let coordinate = VoxelCoordinate::new(
                        origin_x
                            .checked_add(
                                i32::try_from(x_offset)
                                    .map_err(|_| metadata_dimensions_error(source_revision))?,
                            )
                            .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                        origin_y
                            .checked_add(
                                i32::try_from(y_offset)
                                    .map_err(|_| metadata_dimensions_error(source_revision))?,
                            )
                            .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                        origin_z
                            .checked_add(
                                i32::try_from(z_offset)
                                    .map_err(|_| metadata_dimensions_error(source_revision))?,
                            )
                            .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                    );
                    for offset in [
                        [0, 0, 0],
                        [-1, 0, 0],
                        [1, 0, 0],
                        [0, -1, 0],
                        [0, 1, 0],
                        [0, 0, -1],
                        [0, 0, 1],
                    ] {
                        let [x, y, z] = coordinate.components();
                        let Some(x) = x.checked_add(offset[0]) else {
                            continue;
                        };
                        let Some(y) = y.checked_add(offset[1]) else {
                            continue;
                        };
                        let Some(z) = z.checked_add(offset[2]) else {
                            continue;
                        };
                        let neighbor = [x, y, z];
                        let [volume_width, volume_height, volume_depth] =
                            metadata.extent().dimensions();
                        let volume_dimensions = [volume_width, volume_height, volume_depth];
                        if neighbor
                            .iter()
                            .zip(volume_dimensions)
                            .any(|(component, dimension)| {
                                *component < 0
                                    || u32::try_from(*component)
                                        .map_or(true, |component| component >= dimension)
                            })
                        {
                            continue;
                        }
                        let core_components = neighbor
                            .into_iter()
                            .zip(region_dimensions)
                            .map(|(component, dimension)| {
                                u32::try_from(component)
                                    .ok()
                                    .and_then(|component| {
                                        component
                                            .checked_div(dimension)
                                            .and_then(|index| index.checked_mul(dimension))
                                    })
                                    .and_then(|origin| i32::try_from(origin).ok())
                            })
                            .collect::<Option<Vec<_>>>();
                        let Some(core_components) = core_components else {
                            return Err(metadata_dimensions_error(source_revision));
                        };
                        let [core_x, core_y, core_z] = core_components.as_slice() else {
                            return Err(metadata_dimensions_error(source_revision));
                        };
                        affected.insert(RasterRegionIdentity {
                            volume_identity: changed_region.volume_identity().clone(),
                            core_origin: VoxelCoordinate::new(*core_x, *core_y, *core_z),
                        });
                    }
                }
            }
        }
    }
    Ok(affected)
}

pub(super) fn derive_raster_region(
    view: &VoxelSceneView,
    metadata: &VoxelVolumeMetadata,
    core: VoxelRegion,
) -> Result<RasterRegionResult, RasterArtifactBuildError> {
    let source_revision = view.revision();
    let [core_x, core_y, core_z] = core.origin().components();
    let [core_width, core_height, core_depth] = core.extent().dimensions();
    let halo_extent = VoxelExtent::new(
        core_width
            .checked_add(2)
            .ok_or_else(|| metadata_dimensions_error(source_revision))?,
        core_height
            .checked_add(2)
            .ok_or_else(|| metadata_dimensions_error(source_revision))?,
        core_depth
            .checked_add(2)
            .ok_or_else(|| metadata_dimensions_error(source_revision))?,
    );
    let dimensions = checked_dimensions(halo_extent)
        .ok_or_else(|| metadata_dimensions_error(source_revision))?;
    let value_count = dimensions
        .iter()
        .try_fold(1_usize, |count, dimension| count.checked_mul(*dimension))
        .ok_or_else(|| metadata_dimensions_error(source_revision))?;
    let mut values = Vec::new();
    values.try_reserve_exact(value_count).map_err(|_| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::VoxelRead,
            RasterArtifactBuildCause::AllocationFailed,
        )
    })?;
    values.resize(value_count, VoxelValue::Empty);
    view.read_region_into(
        metadata.identity(),
        VoxelRegion::new(
            VoxelCoordinate::new(core_x - 1, core_y - 1, core_z - 1),
            halo_extent,
        ),
        &mut values,
    )
    .map_err(|error| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::VoxelRead,
            RasterArtifactBuildCause::VoxelRead(error),
        )
    })?;
    let mut pending_faces = Vec::new();
    for z_offset in 0..core_depth {
        for y_offset in 0..core_height {
            for x_offset in 0..core_width {
                let coordinate = VoxelCoordinate::new(
                    core_x
                        .checked_add(
                            i32::try_from(x_offset)
                                .map_err(|_| metadata_dimensions_error(source_revision))?,
                        )
                        .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                    core_y
                        .checked_add(
                            i32::try_from(y_offset)
                                .map_err(|_| metadata_dimensions_error(source_revision))?,
                        )
                        .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                    core_z
                        .checked_add(
                            i32::try_from(z_offset)
                                .map_err(|_| metadata_dimensions_error(source_revision))?,
                        )
                        .ok_or_else(|| metadata_dimensions_error(source_revision))?,
                );
                let local_coordinate = VoxelCoordinate::new(
                    i32::try_from(x_offset + 1)
                        .map_err(|_| metadata_dimensions_error(source_revision))?,
                    i32::try_from(y_offset + 1)
                        .map_err(|_| metadata_dimensions_error(source_revision))?,
                    i32::try_from(z_offset + 1)
                        .map_err(|_| metadata_dimensions_error(source_revision))?,
                );
                let index = dense_index(dimensions, local_coordinate)
                    .ok_or_else(|| metadata_dimensions_error(source_revision))?;
                let Some(VoxelValue::Occupied(material_identity)) = values.get(index) else {
                    continue;
                };
                let linear_base_color = view
                    .material(material_identity)
                    .map(|material| material.linear_base_color())
                    .ok_or_else(|| {
                        build_error(
                            source_revision,
                            RasterArtifactBuildPhase::MaterialResolution,
                            RasterArtifactBuildCause::UnknownMaterial(material_identity.clone()),
                        )
                    })?;
                for normal in AXIS_NORMALS {
                    if face_is_exposed(&values, dimensions, local_coordinate, normal) {
                        pending_faces.push(PendingFace {
                            coordinate,
                            normal,
                            material_identity: material_identity.clone(),
                            linear_base_color,
                        });
                    }
                }
            }
        }
    }
    let artifact = build_geometry(
        view.scene_id(),
        source_revision,
        metadata.identity(),
        metadata,
        pending_faces,
    )?;
    let mut region = artifact.regions.into_iter().next().ok_or_else(|| {
        build_error(
            source_revision,
            RasterArtifactBuildPhase::Geometry,
            RasterArtifactBuildCause::ArithmeticOverflow,
        )
    })?;
    region.identity = RasterRegionIdentity {
        volume_identity: metadata.identity().clone(),
        core_origin: core.origin(),
    };
    region.core = core;
    Ok(region)
}

pub(super) fn assemble_raster_artifact(
    scene_identity: VoxelSceneId,
    source_revision: VoxelSceneRevision,
    region_extent: VoxelExtent,
    regions: Vec<RasterRegionResult>,
) -> Result<RasterArtifact, RasterArtifactBuildError> {
    let (vertex_count, index_count) =
        regions
            .iter()
            .try_fold((0_usize, 0_usize), |(vertices, indices), region| {
                Ok::<_, RasterArtifactBuildError>((
                    vertices
                        .checked_add(region.vertices().len())
                        .ok_or_else(|| geometry_overflow(source_revision))?,
                    indices
                        .checked_add(region.indices().len())
                        .ok_or_else(|| geometry_overflow(source_revision))?,
                ))
            })?;
    let vertex_byte_size = vertex_count
        .checked_mul(size_of::<RasterVertex>())
        .ok_or_else(|| geometry_overflow(source_revision))?;
    let index_byte_size = index_count
        .checked_mul(size_of::<u32>())
        .ok_or_else(|| geometry_overflow(source_revision))?;
    Ok(RasterArtifact {
        scene_identity,
        source_revision,
        region_extent: Some(region_extent),
        volume_identity: None,
        vertex_byte_size,
        index_byte_size,
        regions,
    })
}

pub(super) fn metadata_dimensions_error(
    source_revision: VoxelSceneRevision,
) -> RasterArtifactBuildError {
    build_error(
        source_revision,
        RasterArtifactBuildPhase::Metadata,
        RasterArtifactBuildCause::UnrepresentableVolumeDimensions,
    )
}

fn build_geometry(
    scene_identity: &VoxelSceneId,
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

    let region = RasterRegionResult {
        identity: RasterRegionIdentity {
            volume_identity: volume_identity.clone(),
            core_origin: VoxelCoordinate::new(0, 0, 0),
        },
        core: VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), metadata.extent()),
        source_revision,
        geometry: Arc::new(RasterGeometry {
            vertices,
            indices,
            semantic_faces,
        }),
    };
    Ok(RasterArtifact {
        scene_identity: scene_identity.clone(),
        source_revision,
        region_extent: None,
        volume_identity: Some(volume_identity.clone()),
        vertex_byte_size,
        index_byte_size,
        regions: vec![region],
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
    values: &[VoxelValue],
    dimensions: [usize; 3],
    coordinate: VoxelCoordinate,
    normal: AxisNormal,
) -> bool {
    !offset_coordinate(coordinate, normal)
        .and_then(|neighbor| dense_index(dimensions, neighbor))
        .and_then(|neighbor_index| values.get(neighbor_index))
        .is_some_and(|value| matches!(value, VoxelValue::Occupied(_)))
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

pub(super) fn build_error(
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
