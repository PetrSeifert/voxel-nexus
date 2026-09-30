use super::streamed_fixture_recipe as recipe;
use std::{collections::HashMap, sync::Arc};
use voxel_frontend::{VoxelSceneId, VoxelSceneRevision, VoxelSceneView, VoxelVolumeId};

#[derive(Clone)]
pub struct Oracle {
    pub revision: u64,
    pub side: u32,
    edits: HashMap<VoxelVolumeId, bool>,
}
impl Oracle {
    pub fn new(side: u32) -> Self {
        Self {
            revision: 1,
            side,
            edits: HashMap::new(),
        }
    }
    pub fn edit(&mut self, coordinate: (u32, u32), restore: bool) {
        self.revision += 1;
        self.edits.insert(
            recipe::volume_identity(coordinate.0, coordinate.1),
            !restore,
        );
    }
    pub fn view(&self) -> Result<VoxelSceneView, String> {
        let state = self.edits.clone();
        VoxelSceneView::qualification_source(
            VoxelSceneId::new("streamed-qualification-v1"),
            VoxelSceneRevision::new(self.revision),
            recipe::materials(),
            recipe::catalog(self.side),
            Arc::new(move |identity, coordinate| {
                let [x, y, z] = coordinate.components();
                let changed = state.get(identity).copied().unwrap_or(false);
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
