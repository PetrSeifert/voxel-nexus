use super::{
    allocation::{self, Category},
    streamed_fixture_recipe as recipe,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use voxel_frontend::{
    VoxelCoordinate, VoxelExtent, VoxelFrontend, VoxelRegion, VoxelSceneId, VoxelSceneRevision,
    VoxelSceneView, VoxelValue, VoxelVolumeId, VoxelVolumeMetadata,
};

#[derive(Clone)]
struct VolumeState {
    version: u64,
    edited: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_reload_and_restoration_have_frozen_fingerprints() -> Result<(), String> {
        let generated = Snapshot::new(16);
        let edited = generated.edit((2, 2), false)?;
        let restored = edited.edit((2, 2), true)?;
        assert_eq!(generated.version((3, 3)), restored.version((3, 3)));
        assert_ne!(generated.version((2, 2)), restored.version((2, 2)));
        let mut cache = Cache::new();
        for snapshot in [&edited, &generated, &restored] {
            cache.begin_query(snapshot, (2, 2))?;
            cache.verify_query(snapshot)?;
            assert!(cache.begin_query(snapshot, (3, 3)).is_err());
            cache.end_query();
            assert_eq!(cache.copies(), 0);
        }
        Ok(())
    }

    #[test]
    fn nineteen_copy_admission_and_query_transfer_share_one_materialization() -> Result<(), String>
    {
        let source = Snapshot::new(16);
        let mut cache = Cache::new();
        let installed = keys(&source, (3, 3));
        let newest = keys(&source, (12, 12));
        for key in installed.iter().chain(&newest) {
            cache.ensure(&source, key)?;
        }
        cache.begin_query(&source, (15, 15))?;
        assert_eq!(cache.copies(), 19);
        let generated = cache.generated;
        cache.ensure(
            &source,
            &Key {
                coordinate: (15, 15),
                version: 1,
            },
        )?;
        assert_eq!(cache.generated, generated);
        assert_eq!(cache.copies(), 19);
        assert!(
            cache
                .ensure(
                    &source,
                    &Key {
                        coordinate: (14, 15),
                        version: 1
                    }
                )
                .is_err()
        );
        cache.end_query();
        cache.retain(&installed, &installed);
        assert_eq!(cache.copies(), 9);
        assert_eq!(cache.peak_copies, 19);
        let view = cache.assemble(&source, &installed)?;
        assert!(matches!(
            compute_ray_render_path::ComputeSceneBundle::qualification_streamed(
                &view,
                compute_ray_render_path::ComputeRepresentation::Dense
            ),
            Err(compute_ray_render_path::ComputeSceneBuildError::StreamedDense)
        ));
        Ok(())
    }
}
#[derive(Clone)]
struct State {
    revision: u64,
    volumes: HashMap<VoxelVolumeId, VolumeState>,
}
#[derive(Clone)]
pub struct Snapshot {
    state: Arc<State>,
    metadata: Arc<Vec<VoxelVolumeMetadata>>,
    pub side: u32,
}
impl Snapshot {
    pub fn new(side: u32) -> Self {
        Self {
            state: allocation::within(Category::History, || {
                Arc::new(State {
                    revision: 1,
                    volumes: HashMap::new(),
                })
            }),
            metadata: allocation::within(Category::Metadata, || Arc::new(recipe::catalog(side))),
            side,
        }
    }
    pub fn revision(&self) -> u64 {
        self.state.revision
    }
    pub fn version(&self, coordinate: (u32, u32)) -> u64 {
        self.state
            .volumes
            .get(&recipe::volume_identity(coordinate.0, coordinate.1))
            .map_or(1, |state| state.version)
    }
    pub fn edited(&self, coordinate: (u32, u32)) -> bool {
        self.state
            .volumes
            .get(&recipe::volume_identity(coordinate.0, coordinate.1))
            .is_some_and(|state| state.edited)
    }
    pub fn edit(&self, coordinate: (u32, u32), restore: bool) -> Result<Self, String> {
        if coordinate.0 >= self.side || coordinate.1 >= self.side {
            return Err("edit outside the finite scene".into());
        }
        if self.edited(coordinate) == !restore {
            return Ok(self.clone());
        }
        allocation::within(Category::History, || {
            let mut state = (*self.state).clone();
            state.revision = state
                .revision
                .checked_add(1)
                .ok_or("scene revision exhausted")?;
            state.volumes.insert(
                recipe::volume_identity(coordinate.0, coordinate.1),
                VolumeState {
                    version: state.revision,
                    edited: !restore,
                },
            );
            Ok(Self {
                state: Arc::new(state),
                metadata: self.metadata.clone(),
                side: self.side,
            })
        })
    }
    pub fn overlay_coordinates(&self) -> usize {
        self.state
            .volumes
            .values()
            .filter(|volume| volume.edited)
            .count()
            * 3
    }
    pub fn oracle_view(&self) -> Result<VoxelSceneView, String> {
        let state = self.state.clone();
        VoxelSceneView::qualification_source(
            VoxelSceneId::new("streamed-qualification-v1"),
            VoxelSceneRevision::new(self.revision()),
            recipe::materials(),
            (*self.metadata).clone(),
            Arc::new(move |identity, coordinate| {
                let [x, y, z] = coordinate.components();
                let changed = state
                    .volumes
                    .get(identity)
                    .is_some_and(|state| state.edited);
                let code = if changed {
                    recipe::EDIT_COORDINATES
                        .iter()
                        .position(|entry| *entry == [x, y, z])
                        .and_then(|index| [1, 0, 2].get(index).copied())
                        .unwrap_or_else(|| recipe::generated_code(x, y, z))
                } else {
                    recipe::generated_code(x, y, z)
                };
                recipe::material(code)
            }),
        )
        .map_err(|error| error.to_string())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Key {
    pub coordinate: (u32, u32),
    pub version: u64,
}
pub fn keys(snapshot: &Snapshot, center: (u32, u32)) -> Vec<Key> {
    let mut keys = Vec::new();
    for z in center.1.saturating_sub(1)..=(center.1 + 1).min(snapshot.side - 1) {
        for x in center.0.saturating_sub(1)..=(center.0 + 1).min(snapshot.side - 1) {
            keys.push(Key {
                coordinate: (x, z),
                version: snapshot.version((x, z)),
            });
        }
    }
    keys
}
pub struct Cache {
    volumes: HashMap<Key, VoxelSceneView>,
    query: Option<(Key, VoxelSceneView, bool)>,
    pub peak_copies: usize,
    pub generation_peak: usize,
    pub generated: u64,
    pub reused: u64,
    pub discarded: u64,
    limit: usize,
}
impl Cache {
    pub fn new() -> Self {
        Self::with_limit(19)
    }
    pub fn with_limit(limit: usize) -> Self {
        Self {
            volumes: HashMap::new(),
            query: None,
            peak_copies: 0,
            generation_peak: 0,
            generated: 0,
            reused: 0,
            discarded: 0,
            limit,
        }
    }
    pub fn copies(&self) -> usize {
        self.volumes.len() + usize::from(self.query.as_ref().is_some_and(|(_, _, owned)| *owned))
    }
    pub fn query_count(&self) -> usize {
        usize::from(self.query.is_some())
    }
    fn generate(&mut self, snapshot: &Snapshot, key: &Key) -> Result<VoxelSceneView, String> {
        if self.copies() >= self.limit {
            return Err("global materialization admission cap".into());
        }
        if snapshot.version(key.coordinate) != key.version {
            return Err("materialization source/content-version mismatch".into());
        }
        if key.coordinate.0 >= snapshot.side || key.coordinate.1 >= snapshot.side {
            return Err("query outside the finite scene".into());
        }
        self.peak_copies = self.peak_copies.max(self.copies() + 1);
        self.generation_peak = self.generation_peak.max(1);
        let input = allocation::within(Category::Generation, || {
            recipe::scene_contents(&[key.coordinate], snapshot.edited(key.coordinate))
        });
        let frontend = VoxelFrontend::new();
        let view = allocation::within(Category::Materialized, || {
            frontend
                .publish_sparse(input)
                .map_err(|error| error.to_string())
        })?;
        self.generated += 1;
        Ok(view)
    }
    pub fn ensure(&mut self, snapshot: &Snapshot, key: &Key) -> Result<bool, String> {
        if self.volumes.contains_key(key) {
            self.reused += 1;
            return Ok(false);
        }
        if let Some((query_key, view, owned)) = &mut self.query
            && query_key == key
        {
            self.volumes.insert(key.clone(), view.clone());
            *owned = false;
            self.reused += 1;
            return Ok(false);
        }
        let view = self.generate(snapshot, key)?;
        self.volumes.insert(key.clone(), view);
        Ok(true)
    }
    pub fn retain(&mut self, installed: &[Key], newest: &[Key]) {
        let retained = installed.iter().chain(newest).collect::<HashSet<_>>();
        let before = self.volumes.len();
        self.volumes.retain(|key, _| {
            retained.contains(key)
                || self
                    .query
                    .as_ref()
                    .is_some_and(|(query, _, _)| query == key)
        });
        self.discarded += (before - self.volumes.len()) as u64;
    }
    pub fn assemble(
        &self,
        snapshot: &Snapshot,
        selection: &[Key],
    ) -> Result<VoxelSceneView, String> {
        let views = selection
            .iter()
            .map(|key| {
                self.volumes
                    .get(key)
                    .cloned()
                    .ok_or("selection has an unmaterialized volume".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        VoxelSceneView::qualification_assemble(VoxelSceneRevision::new(snapshot.revision()), &views)
            .map_err(|error| error.to_string())
    }
    pub fn begin_query(
        &mut self,
        snapshot: &Snapshot,
        coordinate: (u32, u32),
    ) -> Result<(), String> {
        if self.query.is_some() {
            return Err("the one global query-only copy is already reserved".into());
        }
        let key = Key {
            coordinate,
            version: snapshot.version(coordinate),
        };
        let (view, owned) = match self.volumes.get(&key) {
            Some(view) => (view.clone(), false),
            None => (self.generate(snapshot, &key)?, true),
        };
        self.query = Some((key, view, owned));
        Ok(())
    }
    pub fn end_query(&mut self) {
        self.query = None;
    }
    pub fn verify_query(&self, snapshot: &Snapshot) -> Result<u64, String> {
        let (key, view, _) = self.query.as_ref().ok_or("no query copy")?;
        let identity = recipe::volume_identity(key.coordinate.0, key.coordinate.1);
        let mut buffer = allocation::within(Category::Control, || vec![VoxelValue::Empty; 64 * 64]);
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let stone = voxel_frontend::VoxelMaterialId::new("stone");
        let grass = voxel_frontend::VoxelMaterialId::new("grass");
        // Hash rows in canonical x/y/z order without a second full-volume query-result copy.
        for z in 0..64 {
            view.read_region_into(
                &identity,
                VoxelRegion::new(VoxelCoordinate::new(0, 0, z), VoxelExtent::new(64, 64, 1)),
                &mut buffer,
            )
            .map_err(|error| error.to_string())?;
            for value in &buffer {
                let code = match value {
                    VoxelValue::Empty => 0,
                    VoxelValue::Occupied(identity) if *identity == stone => 1,
                    VoxelValue::Occupied(identity) if *identity == grass => 2,
                    _ => return Err("query material outside frozen palette".into()),
                };
                hash = (hash ^ code).wrapping_mul(0x100_0000_01b3);
            }
        }
        if hash != recipe::fingerprint(snapshot.edited(key.coordinate)) {
            return Err("historical query/reload fingerprint mismatch".into());
        }
        Ok(hash)
    }
}
