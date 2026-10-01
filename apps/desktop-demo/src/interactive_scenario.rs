use super::*;
use std::collections::HashSet;
use winit::event::MouseButton;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::CursorGrabMode;

pub(super) struct InteractiveState {
    camera: FreeFlyCamera,
    held_keys: HashSet<KeyCode>,
    pending_look_x: f64,
    pending_look_y: f64,
    last_movement_at: Option<Instant>,
    captured: bool,
    pick: Option<SemanticRayObservation>,
    pick_needed: bool,
    selected_material_index: usize,
    pub(super) control_feedback: String,
    last_overlay_report: Option<String>,
    edited_coordinates: HashSet<(VoxelVolumeId, VoxelCoordinate)>,
}

impl InteractiveState {
    pub(super) fn new(camera_state: render_backend::CameraState) -> Self {
        Self {
            camera: FreeFlyCamera::from_camera_state(camera_state),
            held_keys: HashSet::new(),
            pending_look_x: 0.0,
            pending_look_y: 0.0,
            last_movement_at: None,
            captured: false,
            pick: None,
            pick_needed: true,
            selected_material_index: 0,
            control_feedback: "Tab-waiting-for-convergence".to_owned(),
            last_overlay_report: None,
            edited_coordinates: HashSet::new(),
        }
    }

    pub(super) fn camera_state(&self) -> Result<render_backend::CameraState, String> {
        self.camera
            .camera_state()
            .map_err(|error| error.to_string())
    }

    fn movement_input(&self) -> MovementInput {
        let held = |key| self.held_keys.contains(&key);
        MovementInput {
            forward: held(KeyCode::KeyW),
            backward: held(KeyCode::KeyS),
            left: held(KeyCode::KeyA),
            right: held(KeyCode::KeyD),
            up: held(KeyCode::KeyE),
            down: held(KeyCode::KeyQ),
            fast: held(KeyCode::ShiftLeft) || held(KeyCode::ShiftRight),
        }
    }
}

fn material_key_index(key: KeyCode) -> Option<usize> {
    match key {
        KeyCode::Digit1 => Some(0),
        KeyCode::Digit2 => Some(1),
        KeyCode::Digit3 => Some(2),
        KeyCode::Digit4 => Some(3),
        KeyCode::Digit5 => Some(4),
        KeyCode::Digit6 => Some(5),
        KeyCode::Digit7 => Some(6),
        KeyCode::Digit8 => Some(7),
        KeyCode::Digit9 => Some(8),
        _ => None,
    }
}

enum InteractiveEditFailure {
    Rejected(EditRejection),
    Failed(String),
}

impl From<EditRejection> for InteractiveEditFailure {
    fn from(rejection: EditRejection) -> Self {
        Self::Rejected(rejection)
    }
}

impl From<String> for InteractiveEditFailure {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

pub(super) fn format_interactive_overlay(
    diagnostics: &RenderPathSwitchDiagnostics,
    target: &str,
    material: &str,
    control_feedback: &str,
) -> String {
    let presenting = diagnostics.presenting();
    format!(
        "Presenter={} Switch={} Required={} Visible={} RequiredSelection={} InstalledSelection={} Target={target} Material={material} Control={control_feedback}",
        presenting.strategy().identifier(),
        render_path_switch_phase(diagnostics),
        presenting.required_revision(),
        presenting.visible_revision(),
        presenting
            .required_selection()
            .map_or_else(|| "all".to_owned(), |selection| selection.to_string()),
        presenting
            .installed_selection()
            .map_or_else(|| "all".to_owned(), |selection| selection.to_string()),
    )
}

impl ScenarioExecution<'_> {
    /// Routes Render Path switching feedback to whichever mode owns the overlay.
    pub(super) fn set_control_feedback(&mut self, feedback: String) {
        match &mut self.state.interactive {
            Some(interactive) => interactive.control_feedback = feedback,
            None => self.state.compute.render_path_control_feedback = feedback,
        }
    }

    fn selected_material(&self) -> Result<Option<VoxelMaterialId>, String> {
        let Some(interactive) = &self.state.interactive else {
            return Ok(None);
        };
        Ok(self
            .desktop
            .latest_scene_view()?
            .materials()
            .get(interactive.selected_material_index)
            .map(|material| material.identity().clone()))
    }

    pub(super) fn set_interactive_overlay(&mut self) -> Result<(), String> {
        let streamed = self.desktop.streamed_report()?;
        let diagnostics = self.desktop.switch_diagnostics()?;
        let material = match self.selected_material() {
            Ok(Some(material)) => format!("{material:?}"),
            Ok(None) => "none".to_owned(),
            Err(error) => format!("unavailable({error})"),
        };
        let Some(interactive) = &mut self.state.interactive else {
            return Ok(());
        };
        let target = interactive
            .pick
            .as_ref()
            .map(format_pick_target)
            .unwrap_or_else(|| "none".to_owned());
        let mut report = format_interactive_overlay(
            &diagnostics,
            &target,
            &material,
            &interactive.control_feedback,
        );
        if let Some(streamed) = streamed {
            report = format!("{report} {streamed}");
        }
        if interactive.last_overlay_report.as_deref() == Some(&report) {
            return Ok(());
        }
        self.desktop
            .text_overlay
            .as_ref()
            .ok_or_else(|| "the in-client interactive overlay is unavailable".to_owned())?
            .set_text(&report)?;
        self.desktop.set_status(&report);
        interactive.last_overlay_report = Some(report);
        Ok(())
    }

    fn refresh_interactive_overlay(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.set_interactive_overlay() {
            self.desktop.fail(event_loop, error);
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.desktop.window {
            window.request_redraw();
        }
    }

    fn set_cursor_capture(&mut self, captured: bool) -> Result<(), String> {
        let window = self
            .desktop
            .window
            .as_ref()
            .ok_or_else(|| "the desktop window is unavailable for cursor capture".to_owned())?;
        let grab_mode = if captured {
            CursorGrabMode::Confined
        } else {
            CursorGrabMode::None
        };
        window
            .set_cursor_grab(grab_mode)
            .map_err(|error| format!("could not change cursor capture: {error}"))?;
        window.set_cursor_visible(!captured);
        if let Some(interactive) = &mut self.state.interactive {
            interactive.captured = captured;
            if !captured {
                interactive.pending_look_x = 0.0;
                interactive.pending_look_y = 0.0;
            }
        }
        Ok(())
    }

    fn release_interactive_capture(&mut self) {
        let feedback = match self.set_cursor_capture(false) {
            Ok(()) => "Capture-released".to_owned(),
            Err(error) => format!("Capture-failed-{error}"),
        };
        self.set_control_feedback(feedback);
    }

    pub(super) fn interactive_focus_changed(
        &mut self,
        event_loop: &ActiveEventLoop,
        focused: bool,
    ) {
        if focused {
            return;
        }
        let Some(interactive) = &mut self.state.interactive else {
            return;
        };
        interactive.held_keys.clear();
        interactive.last_movement_at = None;
        if interactive.captured {
            self.release_interactive_capture();
        }
        self.refresh_interactive_overlay(event_loop);
    }

    pub(super) fn interactive_keyboard_input(
        &mut self,
        event_loop: &ActiveEventLoop,
        event: &winit::event::KeyEvent,
    ) {
        let Some(interactive) = &mut self.state.interactive else {
            return;
        };
        let PhysicalKey::Code(key) = event.physical_key else {
            return;
        };
        match event.state {
            ElementState::Pressed => {
                interactive.held_keys.insert(key);
            }
            ElementState::Released => {
                interactive.held_keys.remove(&key);
            }
        }
        if interactive.movement_input().is_moving() {
            if interactive.last_movement_at.is_none() {
                interactive.last_movement_at = Some(Instant::now());
            }
            self.request_redraw();
        } else {
            interactive.last_movement_at = None;
        }
        if event.state != ElementState::Pressed || event.repeat {
            return;
        }
        match key {
            KeyCode::Escape => self.release_interactive_capture(),
            KeyCode::Tab => self.request_interactive_mode_switch(),
            KeyCode::KeyR
                if self
                    .desktop
                    .render_configuration
                    .streamed_neighbourhood()
                    .is_some() =>
            {
                let feedback = match self.restore_streamed_edits() {
                    Ok(EditPublication::Changed(revision)) => {
                        format!("Restore-published-{revision}")
                    }
                    Ok(EditPublication::Unchanged) => "Restore-unchanged".to_owned(),
                    Err(InteractiveEditFailure::Rejected(reason)) => {
                        format!("Restore-rejected-{}", reason.overlay_label())
                    }
                    Err(InteractiveEditFailure::Failed(error)) => format!("Restore-failed-{error}"),
                };
                self.set_control_feedback(feedback);
            }
            key => {
                let Some(index) = material_key_index(key) else {
                    return;
                };
                self.select_material(index);
            }
        }
        self.refresh_interactive_overlay(event_loop);
        self.request_redraw();
    }

    fn select_material(&mut self, index: usize) {
        let key_number = index + 1;
        let material_count = match self.desktop.latest_scene_view() {
            Ok(view) => view.materials().len(),
            Err(error) => {
                self.set_control_feedback(format!("Material-{key_number}-failed-{error}"));
                return;
            }
        };
        let Some(interactive) = &mut self.state.interactive else {
            return;
        };
        interactive.control_feedback = if index < material_count {
            interactive.selected_material_index = index;
            format!("Material-{key_number}-selected")
        } else {
            format!("Material-{key_number}-unavailable")
        };
    }

    fn request_interactive_mode_switch(&mut self) {
        let result = self
            .desktop
            .admit_render_path_switch()
            .and_then(|admitted| {
                let accepted = (
                    admitted.source,
                    admitted.replacement,
                    admitted.view().revision(),
                );
                self.desktop
                    .start_render_path_switch(admitted, |_, _| Ok(()))?;
                Ok(accepted)
            });
        let feedback = match result {
            Ok((source, replacement, revision)) => {
                println!(
                    "Tab switch accepted: Presenting={source:?} Replacement={replacement:?} revision={revision}"
                );
                format!("Tab-accepted-{}", replacement.identifier())
            }
            Err(rejection) => {
                println!("Tab switch rejected: {rejection}");
                format!("Tab-rejected-{rejection}")
            }
        };
        self.set_control_feedback(feedback);
    }

    pub(super) fn interactive_mouse_input(
        &mut self,
        event_loop: &ActiveEventLoop,
        state: ElementState,
        button: MouseButton,
    ) {
        let Some(interactive) = &self.state.interactive else {
            return;
        };
        if state != ElementState::Pressed {
            return;
        }
        if !interactive.captured {
            // The capturing click only focuses the view, so it never edits.
            let feedback = match self.set_cursor_capture(true) {
                Ok(()) => "Capture-acquired".to_owned(),
                Err(error) => format!("Capture-failed-{error}"),
            };
            self.set_control_feedback(feedback);
            self.refresh_interactive_overlay(event_loop);
            return;
        }
        let (label, action) = match button {
            MouseButton::Left => ("Break", Ok(Some(EditAction::Break))),
            MouseButton::Right => (
                "Place",
                self.selected_material()
                    .map(|material| material.map(EditAction::Place)),
            ),
            _ => return,
        };
        let publication = match action {
            Ok(Some(action)) => self.publish_interactive_edit(&action),
            Ok(None) => Err(InteractiveEditFailure::Rejected(EditRejection::NoMaterial)),
            Err(error) => Err(InteractiveEditFailure::Failed(error)),
        };
        let feedback = match publication {
            Ok(EditPublication::Changed(revision)) => format!("{label}-published-{revision}"),
            Ok(EditPublication::Unchanged) => format!("{label}-unchanged"),
            Err(InteractiveEditFailure::Rejected(rejection)) => {
                format!("{label}-rejected-{}", rejection.overlay_label())
            }
            Err(InteractiveEditFailure::Failed(error)) => format!("{label}-failed-{error}"),
        };
        println!("Interactive edit: {feedback}");
        self.set_control_feedback(feedback);
        self.refresh_interactive_overlay(event_loop);
        self.request_redraw();
    }

    fn publish_interactive_edit(
        &mut self,
        action: &EditAction,
    ) -> Result<EditPublication, InteractiveEditFailure> {
        let diagnostics = self.desktop.switch_diagnostics()?;
        edit_admission(
            diagnostics.roles().replacement(),
            diagnostics.presenting().readiness(),
        )?;
        let view = self.desktop.latest_scene_view()?;
        let interactive = self
            .state
            .interactive
            .as_mut()
            .ok_or_else(|| "the interactive state is unavailable".to_owned())?;
        // Edits always target the latest revision, even while convergence still shows an
        // earlier one.
        let observation = pick(
            &view,
            &FreeFlyCamera::from_camera_state(self.desktop.camera_state),
        )?;
        let command = edit_command(action, &observation, view.volumes())?;
        let coordinates: Vec<_> = command
            .edits()
            .iter()
            .map(|edit| (edit.volume_identity().clone(), edit.coordinate()))
            .collect();
        interactive.pick = Some(observation);
        let frontend = self
            .desktop
            .frontend
            .as_ref()
            .ok_or_else(|| "the Voxel Frontend is unavailable".to_owned())?;
        let backend = self
            .desktop
            .backend
            .as_mut()
            .ok_or_else(|| "the Render Backend is unavailable".to_owned())?;
        let publication = publish_edit(frontend, command, |outcome| {
            backend
                .submit_edit_outcome(outcome)
                .map_err(|error| error.to_string())
        })?;
        if let EditPublication::Changed(revision) = publication {
            self.desktop.published_revision = Some(revision);
            interactive.pick_needed = true;
            if view.is_streamed() {
                interactive.edited_coordinates.extend(coordinates);
            }
            self.desktop.update_streamed_residency(
                self.desktop
                    .pending_camera
                    .map_or(self.desktop.camera_state, |(pose, _)| pose),
            )?;
        }
        Ok(publication)
    }

    fn restore_streamed_edits(&mut self) -> Result<EditPublication, InteractiveEditFailure> {
        let diagnostics = self.desktop.switch_diagnostics()?;
        edit_admission(
            diagnostics.roles().replacement(),
            diagnostics.presenting().readiness(),
        )?;
        let interactive = self
            .state
            .interactive
            .as_mut()
            .ok_or("the interactive state is unavailable".to_owned())?;
        let command = VoxelEditCommand::from_edits(
            interactive
                .edited_coordinates
                .iter()
                .map(|(identity, coordinate)| {
                    let [x, y, z] = coordinate.components();
                    voxel_frontend::VoxelEdit::new(
                        identity.clone(),
                        *coordinate,
                        super::streamed_fixture_recipe::material(
                            super::streamed_fixture_recipe::generated_code(x, y, z),
                        ),
                    )
                })
                .collect(),
        );
        let frontend = self
            .desktop
            .frontend
            .as_ref()
            .ok_or("the Voxel Frontend is unavailable".to_owned())?;
        let backend = self
            .desktop
            .backend
            .as_mut()
            .ok_or("the Render Backend is unavailable".to_owned())?;
        let publication = publish_edit(frontend, command, |outcome| {
            backend
                .submit_edit_outcome(outcome)
                .map_err(|error| error.to_string())
        })?;
        interactive.edited_coordinates.clear();
        interactive.pick_needed = true;
        if let EditPublication::Changed(revision) = publication {
            self.desktop.published_revision = Some(revision);
            self.desktop.update_streamed_residency(
                self.desktop
                    .pending_camera
                    .map_or(self.desktop.camera_state, |(pose, _)| pose),
            )?;
        }
        Ok(publication)
    }

    pub(super) fn interactive_mouse_motion(&mut self, delta: (f64, f64)) {
        let Some(interactive) = &mut self.state.interactive else {
            return;
        };
        if !interactive.captured {
            return;
        }
        let (delta_x, delta_y) = delta;
        interactive.pending_look_x += delta_x;
        interactive.pending_look_y += delta_y;
        self.request_redraw();
    }

    /// Applies input once per redraw so at most one Camera State is published per frame.
    pub(super) fn before_interactive_draw(&mut self, event_loop: &ActiveEventLoop) {
        let previous_camera = self.desktop.camera_state;
        if let Err(error) = self.desktop.accept_pending_camera() {
            self.desktop.fail(event_loop, error);
            return;
        }
        let Some(interactive) = &mut self.state.interactive else {
            return;
        };
        interactive.pick_needed |= previous_camera != self.desktop.camera_state;
        let look_x = std::mem::take(&mut interactive.pending_look_x);
        let look_y = std::mem::take(&mut interactive.pending_look_y);
        interactive.camera.look(look_x, look_y);
        let now = Instant::now();
        if let Some(last_movement_at) = interactive.last_movement_at {
            if self.desktop.pending_camera.is_none() {
                interactive.camera.advance(
                    interactive.movement_input(),
                    now.saturating_duration_since(last_movement_at),
                );
            }
            interactive.last_movement_at = Some(now);
        }
        let neighbourhood = self.desktop.render_configuration.streamed_neighbourhood();
        let camera_state = match interactive.camera.camera_state().and_then(|camera| {
            if let Some(neighbourhood) = neighbourhood {
                let extent = self.desktop.drawable_extent;
                super::streamed_world::camera_state(
                    camera,
                    [extent.width, extent.height],
                    neighbourhood,
                )
            } else {
                Ok(camera)
            }
        }) {
            Ok(camera_state) => camera_state,
            Err(error) => {
                self.desktop.fail(event_loop, error);
                return;
            }
        };
        if camera_state.far_plane() != interactive.camera.far_plane() {
            interactive.camera = FreeFlyCamera::from_camera_state(camera_state);
        }
        let mut pick_needed = std::mem::take(&mut interactive.pick_needed);
        if camera_state != self.desktop.camera_state
            && self.desktop.pending_camera.map(|(pose, _)| pose) != Some(camera_state)
        {
            if let Err(error) = self.desktop.publish_camera_state(camera_state) {
                self.desktop.fail(event_loop, error);
                return;
            }
            pick_needed = true;
        }
        let camera = FreeFlyCamera::from_camera_state(self.desktop.camera_state);
        if !pick_needed {
            return;
        }
        match self
            .desktop
            .latest_scene_view()
            .and_then(|view| pick(&view, &camera))
        {
            Ok(observation) => {
                if let Some(interactive) = &mut self.state.interactive {
                    interactive.pick = Some(observation);
                }
            }
            Err(error) => self.set_control_feedback(format!("Pick-failed-{error}")),
        }
        self.refresh_interactive_overlay(event_loop);
    }

    pub(super) fn after_interactive_presented(&mut self, event_loop: &ActiveEventLoop) {
        let Some(interactive) = &self.state.interactive else {
            return;
        };
        let moving = interactive.last_movement_at.is_some();
        let converging = match self.desktop.switch_diagnostics() {
            Ok(diagnostics) => {
                let presenting = diagnostics.presenting();
                !presenting.is_fully_converged()
                    || diagnostics.held_camera_state_revision().is_some()
            }
            Err(error) => {
                self.desktop.fail(event_loop, error);
                return;
            }
        };
        if moving || converging {
            self.request_redraw();
        }
        self.refresh_interactive_overlay(event_loop);
    }
}
