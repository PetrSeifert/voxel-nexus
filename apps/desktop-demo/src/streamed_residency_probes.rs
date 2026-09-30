use ash::vk;
use compute_ray_render_path::{ComputeSemanticRayController, camera_semantic_ray};
use raster_render_path::RasterSemanticFaceController;
use render_backend::CameraState;
use semantic_ray_oracle::{
    SemanticRayDistanceTolerance, SemanticRayProbe, SemanticRayProbeObservation,
    observe_probe_along_ray,
};
pub enum Probes {
    Raster(RasterSemanticFaceController),
    Brickmap(ComputeSemanticRayController),
}
impl Probes {
    pub fn request_probes(
        &self,
        source: &super::streamed_qualification_oracle::Oracle,
        camera: CameraState,
    ) -> Result<Vec<SemanticRayProbeObservation>, String> {
        let oracle = source.view()?;
        let mut observations = Vec::new();
        let mut probes = Vec::new();
        for pixel in [[960, 540], [480, 810], [1440, 810], [960, 1000]] {
            let ray = camera_semantic_ray(
                camera,
                vk::Extent2D {
                    width: 1920,
                    height: 1080,
                },
                pixel,
            )
            .map_err(|error| error.to_string())?;
            let probe = SemanticRayProbe::new(format!("pixel-{}-{}", pixel[0], pixel[1]), ray)
                .map_err(|error| error.to_string())?;
            let observation =
                observe_probe_along_ray(&oracle, &probe).map_err(|error| error.to_string())?;
            observations.push(observation);
            probes.push(probe);
        }
        match self {
            Probes::Raster(controller) => controller
                .request(observations.clone())
                .map_err(|error| error.to_string())?,
            Probes::Brickmap(controller) => controller
                .request(probes)
                .map_err(|error| error.to_string())?,
        }
        Ok(observations)
    }
    pub fn verify_probes(&self, expected: &[SemanticRayProbeObservation]) -> Result<usize, String> {
        match self {
            Probes::Raster(controller) => {
                let actual = controller.drain().map_err(|error| error.to_string())?;
                if !actual.is_empty()
                    && (actual.len() != expected.len()
                        || actual
                            .iter()
                            .map(|entry| entry.probe_identity())
                            .collect::<std::collections::HashSet<_>>()
                            .len()
                            != expected.len())
                {
                    return Err("incomplete or duplicated rendered probe batch".into());
                }
                if actual.iter().any(|entry| {
                    !entry.passed()
                        || !expected.iter().any(|expected| {
                            expected.probe_identity() == entry.probe_identity()
                                && expected.observation().revision() == entry.revision()
                        })
                }) {
                    return Err(
                        "presented raster faces disagree with the whole-scene oracle".into(),
                    );
                }
                Ok(actual.len())
            }
            Probes::Brickmap(controller) => {
                let actual = controller.drain().map_err(|error| error.to_string())?;
                let tolerance =
                    SemanticRayDistanceTolerance::new(0.002).map_err(|error| error.to_string())?;
                if !actual.is_empty()
                    && (actual.len() != expected.len()
                        || actual
                            .iter()
                            .map(|entry| entry.probe_identity())
                            .collect::<std::collections::HashSet<_>>()
                            .len()
                            != expected.len())
                {
                    return Err("incomplete or duplicated rendered probe batch".into());
                }
                if actual.iter().any(|entry| {
                    !expected.iter().any(|expected| {
                        expected.probe_identity() == entry.probe_identity()
                            && entry
                                .observation()
                                .agrees_with(expected.observation(), tolerance)
                    })
                }) {
                    return Err(
                        "rendered Brickmap probes disagree with the whole-scene oracle".into(),
                    );
                }
                Ok(actual.len())
            }
        }
    }
}
