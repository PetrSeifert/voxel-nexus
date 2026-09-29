use render_backend::CameraState;

pub const DURATION: f64 = 32.0 + 64.0 * std::f64::consts::SQRT_2 + 32.0;
pub const CROSSINGS: [f64; 8] = [
    8.0,
    24.0,
    32.0 + 8.0 * std::f64::consts::SQRT_2,
    32.0 + 24.0 * std::f64::consts::SQRT_2,
    32.0 + 40.0 * std::f64::consts::SQRT_2,
    32.0 + 56.0 * std::f64::consts::SQRT_2,
    32.0 + 64.0 * std::f64::consts::SQRT_2 + 8.0,
    32.0 + 64.0 * std::f64::consts::SQRT_2 + 24.0,
];
pub fn camera(seconds: f64) -> Result<CameraState, String> {
    let points = [
        [224.0, 224.0],
        [352.0, 224.0],
        [480.0, 352.0],
        [352.0, 224.0],
        [224.0, 224.0],
    ];
    let mut time = seconds.min(DURATION);
    for segment in points.windows(2) {
        let from = segment
            .first()
            .expect("windows(2) contains its first route point");
        let to = segment
            .get(1)
            .expect("windows(2) contains its second route point");
        let dx: f64 = to[0] - from[0];
        let dz: f64 = to[1] - from[1];
        let length = dx.hypot(dz);
        let duration = length / 4.0;
        if time <= duration {
            let x = from[0] + dx * time / duration;
            let z = from[1] + dz * time / duration;
            return CameraState::new(
                [x as f32, 48.0, z as f32],
                [
                    (x + 16.0 * dx / length) as f32,
                    24.0,
                    (z + 16.0 * dz / length) as f32,
                ],
                [0.0, 1.0, 0.0],
                60.0,
                0.1,
                34.0,
            )
            .map_err(|error| error.to_string());
        }
        time -= duration;
    }
    Err("route time outside frozen route".into())
}
pub fn center(camera: CameraState, side: u32) -> (u32, u32) {
    let eye = camera.eye();
    (
        (eye[0] / 64.0).floor().max(0.0).min((side - 1) as f32) as u32,
        (eye[2] / 64.0).floor().max(0.0).min((side - 1) as f32) as u32,
    )
}
pub fn covered(
    camera: CameraState,
    selection: &[super::streamed_residency_source::Key],
    side: u32,
) -> bool {
    let Some(minimum_x) = selection.iter().map(|key| key.coordinate.0).min() else {
        return false;
    };
    let Some(minimum_z) = selection.iter().map(|key| key.coordinate.1).min() else {
        return false;
    };
    let maximum_x = selection
        .iter()
        .map(|key| key.coordinate.0)
        .max()
        .unwrap_or(minimum_x);
    let maximum_z = selection
        .iter()
        .map(|key| key.coordinate.1)
        .max()
        .unwrap_or(minimum_z);
    let subtract = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let normalize = |v: [f32; 3]| {
        let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        v.map(|component| component / length)
    };
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let eye = camera.eye();
    let forward = normalize(subtract(camera.target(), eye));
    let right = normalize(cross(forward, camera.up()));
    let upward = cross(right, forward);
    let tangent = (camera.field_of_view_degrees().to_radians() * 0.5).tan();
    let mut low = eye;
    let mut high = eye;
    for distance in [camera.near_plane(), camera.far_plane()] {
        for horizontal in [-1.0, 1.0] {
            for vertical in [-1.0, 1.0] {
                for axis in 0..3 {
                    let point = eye[axis]
                        + distance
                            * (forward[axis]
                                + right[axis] * horizontal * tangent * (1920.0 / 1080.0)
                                + upward[axis] * vertical * tangent);
                    low[axis] = low[axis].min(point);
                    high[axis] = high[axis].max(point);
                }
            }
        }
    }
    // The frustum's convex hull is inside its eight-corner AABB; clip that bound to finite scene space.
    let end = side as f32 * 64.0;
    low[0].max(0.0) >= minimum_x as f32 * 64.0
        && high[0].min(end) <= (maximum_x + 1) as f32 * 64.0
        && low[2].max(0.0) >= minimum_z as f32 * 64.0
        && high[2].min(end) <= (maximum_z + 1) as f32 * 64.0
}
