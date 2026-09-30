use super::*;
use crate::cell_enumeration::{CellGrid, CellSource, ScanningCellSource};

type Source = dyn Fn(&VoxelVolumeId, VoxelCoordinate) -> VoxelValue + Send + Sync;

#[derive(Debug, Error)]
pub enum QualificationViewError {
    #[error("qualification assembly requires at least one materialized view")]
    Empty,
    #[error("qualification views must belong to one scene with identical material palettes")]
    Palette,
    #[error("qualification assembly requires unstreamed materialized views")]
    StreamedAssembly,
    #[error(transparent)]
    Frontend(#[from] VoxelFrontendError),
}

impl VoxelSceneView {
    /// Qualification-only assembly shares each supplied materialized allocation.
    pub fn qualification_assemble(
        revision: VoxelSceneRevision,
        views: &[Self],
    ) -> Result<Self, QualificationViewError> {
        let first = views.first().ok_or(QualificationViewError::Empty)?;
        let mut published = (*first.published).clone();
        published.revision = revision;
        published.volumes.clear();
        published.volume_content_versions.clear();
        let mut metadata = Vec::new();
        for view in views {
            if view.published.streamed.is_some() {
                return Err(QualificationViewError::StreamedAssembly);
            }
            if view.scene_id() != first.scene_id()
                || view.published.palette_values != first.published.palette_values
                || view.materials() != first.materials()
            {
                return Err(QualificationViewError::Palette);
            }
            for volume in view.volumes() {
                let storage = view
                    .published
                    .volumes
                    .get(volume.identity())
                    .expect("published metadata identifies its storage");
                if published
                    .volumes
                    .insert(volume.identity().clone(), storage.clone())
                    .is_some()
                {
                    return Err(VoxelFrontendError::DuplicateVolumeIdentity {
                        identity: volume.identity().clone(),
                    }
                    .into());
                }
                metadata.push(volume.clone());
                published.volume_content_versions.insert(
                    volume.identity().clone(),
                    view.volume_content_version(volume.identity())?,
                );
            }
        }
        published.volume_metadata = metadata.into();
        Ok(Self {
            published: Arc::new(published),
        })
    }

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

    fn volume(identity: &str) -> Result<VoxelSceneView, VoxelFrontendError> {
        VoxelFrontend::new().publish(DenseVoxelScene::new(
            VoxelSceneId::new("qualification"),
            VoxelSceneRevision::new(1),
            vec![],
            vec![DenseVoxelVolume::new(
                VoxelVolumeMetadata::new(
                    VoxelVolumeId::new(identity),
                    VoxelExtent::new(2, 1, 1),
                    [0.0; 3],
                    1.0,
                ),
                vec![DenseVoxelBatch::new(
                    VoxelRegion::new(VoxelCoordinate::new(0, 0, 0), VoxelExtent::new(2, 1, 1)),
                    vec![VoxelValue::Empty; 2],
                )],
            )],
        ))
    }

    #[test]
    fn assembly_shares_storage_and_rejects_duplicate_ownership()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = volume("first")?;
        let second = volume("second")?;
        let assembled = VoxelSceneView::qualification_assemble(
            VoxelSceneRevision::new(7),
            &[first.clone(), second.clone()],
        )?;
        assert_eq!(assembled.revision(), VoxelSceneRevision::new(7));
        assert_eq!(assembled.volumes().len(), 2);
        for original in [&first, &second] {
            let identity = original
                .volumes()
                .first()
                .ok_or("missing metadata")?
                .identity();
            assert!(Arc::ptr_eq(
                original
                    .published
                    .volumes
                    .get(identity)
                    .ok_or("missing original storage")?,
                assembled
                    .published
                    .volumes
                    .get(identity)
                    .ok_or("missing assembled storage")?
            ));
        }
        assert!(matches!(
            VoxelSceneView::qualification_assemble(
                VoxelSceneRevision::new(7),
                &[first.clone(), first]
            ),
            Err(QualificationViewError::Frontend(
                VoxelFrontendError::DuplicateVolumeIdentity { .. }
            ))
        ));
        Ok(())
    }

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
