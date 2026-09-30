use super::*;
use crate::cell_enumeration::{CellGrid, CellSource, ScanningCellSource};

type Source = dyn Fn(&VoxelVolumeId, VoxelCoordinate) -> VoxelValue + Send + Sync;

impl VoxelSceneView {
    /// The source must be immutable, total inside each volume, and return declared materials.
    /// This view is an independent whole-scene oracle input, never renderer residency storage.
    pub fn qualification_source(
        identity: VoxelSceneId,
        revision: VoxelSceneRevision,
        materials: Vec<VoxelMaterial>,
        metadata: Vec<VoxelVolumeMetadata>,
        source: Arc<Source>,
    ) -> Result<Self, VoxelFrontendError> {
        let mut published = PublishedScene::new(
            identity,
            revision,
            materials,
            Vec::<DenseVoxelVolume>::new(),
        )?;
        for volume in &metadata {
            validate_volume_metadata(volume)?;
            let storage = RecipeStorage {
                identity: volume.identity().clone(),
                extent: volume.extent(),
                source: source.clone(),
                materials: published.material_indices.clone(),
                changes: HashMap::new(),
            };
            if published
                .volumes
                .insert(volume.identity().clone(), Arc::new(storage))
                .is_some()
            {
                return Err(VoxelFrontendError::DuplicateVolumeIdentity {
                    identity: volume.identity().clone(),
                });
            }
            published
                .volume_content_versions
                .insert(volume.identity().clone(), revision);
        }
        published.volume_metadata = metadata.into();
        Ok(Self {
            published: Arc::new(published),
        })
    }
}

#[derive(Clone)]
struct RecipeStorage {
    identity: VoxelVolumeId,
    extent: VoxelExtent,
    source: Arc<Source>,
    materials: Arc<HashMap<VoxelMaterialId, MaterialIndex>>,
    changes: HashMap<VoxelCoordinate, MaterialIndex>,
}

impl Storage for RecipeStorage {
    fn extent(&self) -> VoxelExtent {
        self.extent
    }
    fn value(&self, coordinate: VoxelCoordinate) -> MaterialIndex {
        if dense_index(self.extent, coordinate).is_none() {
            return MaterialIndex::EMPTY;
        }
        if let Some(value) = self.changes.get(&coordinate) {
            return *value;
        }
        match (self.source)(&self.identity, coordinate) {
            VoxelValue::Empty => MaterialIndex::EMPTY,
            VoxelValue::Occupied(material) => *self
                .materials
                .get(&material)
                .expect("qualification sources return only their declared materials"),
        }
    }
    fn successor(&self, changes: &[(VoxelCoordinate, MaterialIndex)]) -> Arc<dyn Storage> {
        let mut successor = self.clone();
        successor.changes.extend(changes.iter().copied());
        Arc::new(successor)
    }
    fn storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn uniform_region(&self, bounds: &RegionBounds) -> Option<MaterialIndex> {
        let mut coordinates = bounds.coordinates();
        let value = self.value(coordinates.next()?);
        coordinates
            .all(|coordinate| self.value(coordinate) == value)
            .then_some(value)
    }
    fn cell_source(self: Arc<Self>, grid: CellGrid) -> Box<dyn CellSource> {
        Box::new(ScanningCellSource::new(self, grid))
    }
    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_view_keeps_full_membership_without_materialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let stone = VoxelMaterialId::new("stone");
        let source_stone = stone.clone();
        let metadata = (0..256)
            .map(|index| {
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new(format!("volume-{index}")),
                    VoxelExtent::new(64, 64, 64),
                    [0.0; 3],
                    1.0,
                )
            })
            .collect();
        let source = VoxelSceneView::qualification_source(
            VoxelSceneId::new("qualification"),
            VoxelSceneRevision::new(1),
            vec![VoxelMaterial::new(stone.clone(), [1.0; 4])],
            metadata,
            Arc::new(move |_, coordinate| {
                if coordinate.components()[1] < 24 {
                    VoxelValue::Occupied(source_stone.clone())
                } else {
                    VoxelValue::Empty
                }
            }),
        )?;
        assert_eq!(source.volumes().len(), 256);
        let identity = VoxelVolumeId::new("volume-255");
        assert_eq!(
            source.region_content(
                &identity,
                VoxelRegion::new(VoxelCoordinate::new(1, 23, 1), VoxelExtent::new(1, 1, 1))
            )?,
            VoxelRegionContent::Uniform(VoxelValue::Occupied(stone))
        );
        assert_eq!(
            source.region_content(
                &identity,
                VoxelRegion::new(VoxelCoordinate::new(1, 24, 1), VoxelExtent::new(1, 1, 1))
            )?,
            VoxelRegionContent::Uniform(VoxelValue::Empty)
        );
        assert_eq!(
            source.region_content(
                &identity,
                VoxelRegion::new(VoxelCoordinate::new(-1, 1, 1), VoxelExtent::new(1, 1, 1))
            )?,
            VoxelRegionContent::Uniform(VoxelValue::Empty)
        );
        assert!(
            source
                .published
                .volumes
                .values()
                .all(|storage| storage.storage_bytes() == size_of::<RecipeStorage>())
        );
        Ok(())
    }
}
