use render_backend::{
    BackendError, RenderPath, RenderPathDeviceContext, RenderPathFrameContext, RenderPathPhase,
    RenderPathResult, RenderPathTarget,
};
use std::error::Error;
use std::fmt;

#[derive(Debug)]
struct ProofPathError(&'static str);

impl fmt::Display for ProofPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for ProofPathError {}

struct ProofPath;

impl RenderPath for ProofPath {
    fn release(&mut self, _device: RenderPathDeviceContext<'_>) -> RenderPathResult<()> {
        Ok(())
    }

    fn configure(
        &mut self,
        _device: RenderPathDeviceContext<'_>,
        _target: RenderPathTarget<'_>,
    ) -> RenderPathResult<()> {
        Ok(())
    }

    fn record(&mut self, _frame: RenderPathFrameContext<'_>) -> RenderPathResult<()> {
        Ok(())
    }
}

#[test]
fn render_path_protocol_accepts_an_independent_image_strategy() {
    fn accepts_render_path(_path: &mut dyn RenderPath) {}

    accepts_render_path(&mut ProofPath);
}

#[test]
fn every_render_path_phase_preserves_phase_and_source_context() {
    for (phase, phase_name) in [
        (RenderPathPhase::Release, "release"),
        (RenderPathPhase::Configure, "configure"),
        (RenderPathPhase::Record, "record"),
    ] {
        let error = BackendError::render_path_failure(phase, ProofPathError("proof failure"));

        assert_eq!(
            error.to_string(),
            format!("Render Path {phase_name} failed: proof failure")
        );
        assert_eq!(
            error.source().map(ToString::to_string),
            Some("proof failure".to_owned())
        );
    }
}
