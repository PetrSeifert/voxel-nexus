use super::*;

impl ScenarioExecution<'_> {
    pub(super) fn select_camera(
        &mut self,
        event_loop: &ActiveEventLoop,
        selection: DesktopCameraSelection,
    ) {
        if let Err(error) = selection
            .pose()
            .and_then(|pose| self.desktop.publish_camera_state(pose))
        {
            self.desktop.fail(event_loop, error);
            return;
        }
        self.state.camera.pending_camera_report = Some(selection.report_identity());
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
    }

    pub(super) fn advance_camera_move(&mut self, event_loop: &ActiveEventLoop) {
        let Some(step) = self.state.camera.camera_move_step else {
            return;
        };
        let movement = match overview_to_cavity_camera_move() {
            Ok(movement) => movement,
            Err(error) => {
                self.desktop.fail(event_loop, error);
                return;
            }
        };
        if step >= movement.total_steps() {
            self.state.camera.camera_move_step = None;
            println!(
                "Deterministic camera move completed: steps={}",
                movement.total_steps()
            );
            self.desktop.set_status("camera-move-complete");
            return;
        }
        let next_step = match step.checked_add(1) {
            Some(step) => step,
            None => {
                self.desktop
                    .fail(event_loop, "the deterministic camera move step overflowed");
                return;
            }
        };
        let pose = match movement.pose_at_step(next_step) {
            Ok(pose) => pose,
            Err(error) => {
                self.desktop.fail(event_loop, error);
                return;
            }
        };
        if let Err(error) = self.desktop.publish_camera_state(pose) {
            self.desktop.fail(event_loop, error);
            return;
        }
        self.state.camera.camera_move_step = Some(next_step);
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
    }
}
