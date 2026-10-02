//! PROTOTYPE (issue #139): the frozen #137 route, selection and whole-world scenes shared by the
//! re-profile binaries.
use super::streamed_fixture_recipe::{EDGE, materials, scene, volume_contents};
use super::whole_world_detail_levels::{coarse_volume, downsample};
use ash::vk;
use render_backend::{CameraState, PresentationStyle};
use std::time::Duration;
use voxel_frontend::{
    SparseVoxelScene, VoxelFrontend, VoxelSceneId, VoxelSceneRevision, VoxelValue,
};

pub type RunResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub const GRID: i32 = 16;
pub const DEADBAND: f32 = 16.0;
pub const TELEPORT: f32 = 64.0;
pub const SPEED: f32 = 16.0;
pub const EYE_HEIGHT: f32 = 48.0;
pub const WORLD_CENTRE: [f32; 3] = [512.0, 24.0, 512.0];
pub const WARMUP_HOLD: Duration = Duration::from_secs(5);
pub const DIAGONAL_LEG: usize = 5;
pub const FINAL_LEG: usize = 6;
pub const EXTENT: vk::Extent2D = vk::Extent2D {
    width: 1920,
    height: 1080,
};

pub struct Leg {
    to: [f32; 2],
    teleport: bool,
}

pub fn route() -> (Vec<Leg>, [f32; 2]) {
    let leg = |x, z| Leg {
        to: [x, z],
        teleport: false,
    };
    (
        vec![
            leg(864.0, 160.0),
            leg(864.0, 500.0),
            leg(864.0, 420.0),
            leg(864.0, 864.0),
            Leg {
                to: [160.0, 864.0],
                teleport: true,
            },
            leg(864.0, 160.0),
            leg(160.0, 160.0),
        ],
        [160.0, 160.0],
    )
}

/// Position, travel direction and leg index at route time `seconds`, or None past the lap end.
pub fn route_position(seconds: f32) -> Option<([f32; 2], [f32; 2], usize)> {
    let (legs, mut from) = route();
    let mut remaining = seconds * SPEED;
    for (index, leg) in legs.iter().enumerate() {
        if leg.teleport {
            from = leg.to;
            continue;
        }
        let delta = [leg.to[0] - from[0], leg.to[1] - from[1]];
        let length = delta[0].hypot(delta[1]);
        let direction = [delta[0] / length, delta[1] / length];
        if remaining <= length {
            return Some((
                [
                    from[0] + direction[0] * remaining,
                    from[1] + direction[1] * remaining,
                ],
                direction,
                index,
            ));
        }
        remaining -= length;
        from = leg.to;
    }
    None
}

pub fn nominal(coordinate: f32) -> i32 {
    ((coordinate / EDGE as f32).floor() as i32).clamp(0, GRID - 1)
}

pub fn axis_centre(previous: i32, coordinate: f32) -> i32 {
    let low = previous as f32 * EDGE as f32 - DEADBAND;
    let high = (previous + 1) as f32 * EDGE as f32 + DEADBAND;
    if coordinate >= low && coordinate < high {
        previous
    } else {
        nominal(coordinate)
    }
}

pub fn next_centre(centre: [i32; 2], previous: [f32; 2], position: [f32; 2]) -> [i32; 2] {
    if (position[0] - previous[0]).hypot(position[1] - previous[1]) > TELEPORT {
        [nominal(position[0]), nominal(position[1])]
    } else {
        [
            axis_centre(centre[0], position[0]),
            axis_centre(centre[1], position[1]),
        ]
    }
}

pub fn camera(position: [f32; 2], direction: [f32; 2]) -> RunResult<CameraState> {
    let eye = [position[0], EYE_HEIGHT, position[1]];
    let to_centre = [WORLD_CENTRE[0] - eye[0], WORLD_CENTRE[2] - eye[2]];
    let target = if to_centre[0].hypot(to_centre[1]) < 64.0 {
        [
            eye[0] + direction[0] * 64.0,
            eye[1] - 16.0,
            eye[2] + direction[1] * 64.0,
        ]
    } else {
        WORLD_CENTRE
    };
    let world = GRID as f32 * EDGE as f32;
    let mut farthest: f32 = 0.0;
    for corner_x in [0.0, world] {
        for corner_y in [0.0, EDGE as f32] {
            for corner_z in [0.0, world] {
                let offset = [corner_x - eye[0], corner_y - eye[1], corner_z - eye[2]];
                farthest = farthest.max(
                    (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt(),
                );
            }
        }
    }
    Ok(
        CameraState::new(eye, target, [0.0, 1.0, 0.0], 60.0, 0.1, farthest + 1.0)?
            .with_radial_far_clip()
            .with_presentation_style(PresentationStyle::SCENIC),
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathKind {
    Raster,
    Brickmap,
}

impl PathKind {
    pub fn other(self) -> Self {
        match self {
            Self::Raster => Self::Brickmap,
            Self::Brickmap => Self::Raster,
        }
    }
}

pub fn levels() -> RunResult<Vec<(u32, Vec<VoxelValue>)>> {
    let frontend = VoxelFrontend::new();
    let view = frontend.publish_sparse(scene(&[(3, 3)]))?;
    Ok([32, 16, 8]
        .into_iter()
        .zip(downsample(&view, 3, 3)?)
        .collect())
}

/// Chebyshev reaches, in volumes from the centre, of the 64³, 32³ and 16³ bands.
pub const DEMO_BANDS: [i32; 3] = [3, 6, 12];
pub const QUALIFICATION_BANDS: [i32; 3] = [1, 2, 4];

pub fn selected_edge(bands: [i32; 3], centre: [i32; 2], x: i32, z: i32) -> u32 {
    let distance = (x - centre[0]).abs().max((z - centre[1]).abs());
    match bands.iter().position(|reach| distance <= *reach) {
        Some(0) => 64,
        Some(1) => 32,
        Some(2) => 16,
        _ => 8,
    }
}

pub fn build_scene(levels: &[(u32, Vec<VoxelValue>)], centre: [i32; 2]) -> SparseVoxelScene {
    build_scene_with_bands(levels, centre, DEMO_BANDS)
}

pub fn build_scene_with_bands(
    levels: &[(u32, Vec<VoxelValue>)],
    centre: [i32; 2],
    bands: [i32; 3],
) -> SparseVoxelScene {
    let mut volumes = Vec::new();
    for z in 0..GRID {
        for x in 0..GRID {
            let edge = selected_edge(bands, centre, x, z);
            if edge == 64 {
                volumes.push(volume_contents(x as u32, z as u32, false));
            } else {
                let values = levels
                    .iter()
                    .find(|(level, _)| *level == edge)
                    .map(|(_, values)| values.clone())
                    .expect("every coarse band has a level");
                volumes.push(coarse_volume(x as u32, z as u32, edge, values));
            }
        }
    }
    // An in-place installation keeps one scene identity, as the production Render Backend does.
    SparseVoxelScene::new(
        VoxelSceneId::new("whole-world-detail"),
        VoxelSceneRevision::new(1),
        materials(),
        volumes,
    )
}

/// Every distinct scene the route visits, with a camera from inside that segment.
pub fn route_scenes() -> Vec<([i32; 2], [f32; 2], [f32; 2])> {
    let (start, start_direction, _) = route_position(0.0).expect("the route is not empty");
    let mut centre = [nominal(start[0]), nominal(start[1])];
    let mut previous = start;
    let mut scenes = vec![(centre, start, start_direction)];
    let mut seconds = 0.0;
    while let Some((position, direction, _)) = route_position(seconds) {
        let moved = next_centre(centre, previous, position);
        previous = position;
        if moved != centre {
            centre = moved;
            scenes.push((centre, position, direction));
        }
        seconds += 0.05;
    }
    scenes
}
