use semantic_ray_oracle::{
    AxisNormal, SemanticRay, SemanticRayContact, SemanticRayContactClassification,
    SemanticRayProbe, SemanticRayResult,
};
use voxel_frontend::{VoxelCoordinate, VoxelMaterialId, VoxelVolumeId};
fn contact(
    coordinate: [i32; 3],
    material: &str,
    distance: f64,
    classification: SemanticRayContactClassification,
) -> SemanticRayResult {
    let [x, y, z] = coordinate;
    SemanticRayResult::Contact(SemanticRayContact::new(
        VoxelVolumeId::new("large-terrain"),
        VoxelCoordinate::new(x, y, z),
        VoxelMaterialId::new(material),
        distance,
        classification,
    ))
}

pub fn probes(
    phase: usize,
) -> Result<Vec<(SemanticRayProbe, SemanticRayResult)>, Box<dyn std::error::Error>> {
    use SemanticRayContactClassification::{Entered, StartedInside};
    let roof = if phase == 1 { 65 } else { 64 };
    let floor = if phase == 3 { 32 } else { 31 };
    let mut fixtures = vec![
        (
            "long-empty-miss",
            [2047.5, 255.5, 2047.5],
            [-1.0, 0.0, 0.0],
            4096.0,
            SemanticRayResult::Miss,
        ),
        (
            "long-empty-hit",
            [-2048.0, 80.5, 0.5],
            [1.0, 0.0, 0.0],
            4096.0,
            contact(
                [0, 80, 0],
                "terrain-grass",
                2048.0,
                Entered(AxisNormal::NegativeX),
            ),
        ),
        (
            "fill-interior",
            [8.5, 8.5, 8.5],
            [1.0, 0.0, 0.0],
            100.0,
            contact([8, 8, 8], "terrain-stone", 0.0, StartedInside),
        ),
        (
            "mixed-surface",
            [32.5, 255.5, 32.5],
            [0.0, -1.0, 0.0],
            256.0,
            contact(
                [32, 81, 32],
                "terrain-grass",
                173.5,
                Entered(AxisNormal::PositiveY),
            ),
        ),
        (
            "cavity-wall",
            [800.5, 48.0, 800.5],
            [1.0, 0.0, 0.0],
            1024.0,
            contact(
                [1024, 48, 800],
                "terrain-stone",
                223.5,
                Entered(AxisNormal::NegativeX),
            ),
        ),
        (
            "cavity-miss",
            [800.5, 48.0, 800.5],
            [1.0, 0.0, 0.0],
            100.0,
            SemanticRayResult::Miss,
        ),
        (
            "edited-roof",
            [800.5, 60.0, 800.5],
            [0.0, 1.0, 0.0],
            100.0,
            contact(
                [800, roof, 800],
                "terrain-stone",
                if phase == 1 { 5.0 } else { 4.0 },
                Entered(AxisNormal::NegativeY),
            ),
        ),
        (
            "edited-floor",
            [800.5, 40.0, 800.5],
            [0.0, -1.0, 0.0],
            100.0,
            contact(
                [800, floor, 800],
                "terrain-stone",
                if phase == 3 { 7.0 } else { 8.0 },
                Entered(AxisNormal::PositiveY),
            ),
        ),
    ];
    if phase == 5 {
        fixtures.push((
            "grown-pool",
            [0.5, 201.5, 1024.5],
            [0.0, -1.0, 0.0],
            2.0,
            contact(
                [0, 200, 1024],
                "terrain-stone",
                0.5,
                Entered(AxisNormal::PositiveY),
            ),
        ));
    }
    fixtures
        .into_iter()
        .map(|(name, origin, direction, maximum, expected)| {
            Ok((
                SemanticRayProbe::new(name, SemanticRay::new(origin, direction, 0.0, maximum)?)?,
                expected,
            ))
        })
        .collect()
}
