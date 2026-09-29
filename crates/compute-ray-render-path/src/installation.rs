use super::compute_convergence::{ComputeConvergenceControlError, ComputeConvergenceFailurePhase};
use super::compute_scene::ComputeSceneBundle;
use super::gpu_resources::{
    ComputeHiddenSceneGpuResources, ComputeRayRenderPath, ComputeSceneGpuResources,
    create_scene_gpu_resources, release_scene_gpu_resources, scene_storage_byte_size, u32_bytes,
};
use super::observation::{
    ComputeOwnedResourceCounts, ComputeResourceObservationPoint, ComputeTimingPhase,
};
use super::resource_error::ComputeRenderPathError;
use ash::vk;
use render_backend::{RenderPathDeviceContext, RenderPathTarget};
use std::time::Instant;
use voxel_frontend::VoxelSceneRevision;

impl ComputeRayRenderPath {
    pub(super) fn advance_convergence_at_frame_boundary(
        &mut self,
        device: &RenderPathDeviceContext<'_>,
        target: RenderPathTarget<'_>,
    ) -> Result<(), ComputeRenderPathError> {
        if self.presentation_stopped {
            return Err(ComputeRenderPathError::PresentationStopped);
        }
        if self.configuration_id != Some(target.configuration_id())
            || self.output_extent != target.extent()
        {
            return Err(ComputeRenderPathError::StaleFrameTarget);
        }
        self.convergence.retain_ready_candidate();
        let hidden_stamp = self.convergence.status().hidden();
        let retained_gpu_stamp = self
            .hidden_scene_gpu_resources
            .as_ref()
            .map(|candidate| candidate.stamp);
        if retained_gpu_stamp.is_some() && retained_gpu_stamp != hidden_stamp {
            self.release_hidden_scene_gpu_resources_with(|resources| {
                release_scene_gpu_resources(device, resources);
            })?;
        }
        let Some(candidate_revision) = self
            .convergence
            .hidden_bundle()
            .map(ComputeSceneBundle::revision)
        else {
            return Ok(());
        };
        let candidate_stamp = hidden_stamp.ok_or(ComputeRenderPathError::MissingHiddenCandidate)?;
        if self.hidden_scene_gpu_resources.is_none() {
            #[cfg(any(test, feature = "qualification"))]
            if self
                .convergence
                .fail_hidden_if_injected(ComputeConvergenceFailurePhase::Upload)?
            {
                return Err(ComputeRenderPathError::InjectedConvergenceFailure(
                    ComputeConvergenceFailurePhase::Upload,
                ));
            }
            let candidate_bundle = self
                .convergence
                .hidden_bundle()
                .ok_or(ComputeRenderPathError::MissingHiddenCandidate)?;
            let candidate_range = match scene_storage_byte_size(device, candidate_bundle) {
                Ok(range) => range,
                Err(error) => {
                    self.convergence
                        .fail_hidden(ComputeConvergenceFailurePhase::Upload, error.to_string());
                    return Err(error);
                }
            };
            let upload_started_at = Instant::now();
            let incremental = candidate_bundle.predecessor() == Some(self.scene_gpu_revision);
            let candidate_resources = match if incremental {
                Ok(None)
            } else {
                let candidate_bundle = candidate_bundle.clone();
                create_scene_gpu_resources(
                    device,
                    &candidate_bundle,
                    |allocation_bytes, staging_bytes| {
                        self.convergence.prepare_hidden_allocation(
                            self.scene_allocation_bytes,
                            allocation_bytes,
                            staging_bytes,
                        )
                    },
                )
                .map(Some)
            } {
                Ok(resources) => resources,
                Err(error) => {
                    self.convergence
                        .fail_hidden(ComputeConvergenceFailurePhase::Upload, error.to_string());
                    if matches!(
                        error,
                        ComputeRenderPathError::SceneBuild(
                            crate::ComputeSceneBuildError::GrowthBudgetExceeded { .. }
                        )
                    ) {
                        return Ok(());
                    }
                    return Err(error);
                }
            };
            if let Some(resources) = &candidate_resources {
                self.convergence.record_growth_allocation(
                    self.scene_allocation_bytes + resources.allocation_bytes + candidate_range,
                );
            }
            self.hidden_scene_gpu_resources = Some(ComputeHiddenSceneGpuResources {
                stamp: candidate_stamp,
                resources: candidate_resources,
                range: candidate_range,
            });
            if !incremental {
                self.record_timing_with_bytes(
                    ComputeTimingPhase::Upload,
                    candidate_stamp,
                    upload_started_at,
                    candidate_range,
                )?;
            }
            self.convergence.mark_hidden_uploaded();
        }
        if let Some(control) = &self.convergence_control
            && control.hold_post_upload(candidate_revision)?
        {
            return Ok(());
        }
        let installation_started_at = Instant::now();
        #[cfg(any(test, feature = "qualification"))]
        if self
            .convergence
            .fail_hidden_if_injected(ComputeConvergenceFailurePhase::Installation)?
        {
            self.release_hidden_scene_gpu_resources_with(|resources| {
                release_scene_gpu_resources(device, resources);
            })?;
            return Err(ComputeRenderPathError::InjectedConvergenceFailure(
                ComputeConvergenceFailurePhase::Installation,
            ));
        }
        self.convergence.validate_hidden_base()?;
        let inject_partial_write = self
            .convergence_control
            .as_ref()
            .map(|control| control.take_partial_write_failure())
            .transpose()?
            .unwrap_or(false);
        let candidate_bundle = self
            .convergence
            .hidden_bundle()
            .ok_or(ComputeRenderPathError::MissingHiddenCandidate)?;
        if candidate_bundle.predecessor() == Some(self.scene_gpu_revision) {
            let upload_started_at = Instant::now();
            let uploaded_bytes = candidate_bundle.patches().len() as u64 * 4;
            let ranges = candidate_bundle
                .patches()
                .iter()
                .map(|(index, word)| (index * 4, u32_bytes(std::slice::from_ref(word))))
                .collect::<Vec<_>>();
            if ranges.iter().any(|(offset, bytes)| {
                offset
                    .checked_add(bytes.len())
                    .is_none_or(|end| end as u64 > self.scene_allocation_bytes)
            }) {
                return Err(ComputeRenderPathError::WriteSceneMemory(
                    vk::Result::ERROR_OUT_OF_HOST_MEMORY,
                ));
            }
            // SAFETY: The backend has waited for preceding readers. The coherent
            // allocation bounds and patch base are validated before any mutation.
            let write_result = unsafe {
                if inject_partial_write && !ranges.is_empty() {
                    device
                        .write_memory_ranges(
                            self.scene_memory,
                            self.scene_allocation_bytes,
                            &ranges[..1],
                        )
                        .and(Err(vk::Result::ERROR_UNKNOWN))
                } else {
                    device.write_memory_ranges(
                        self.scene_memory,
                        self.scene_allocation_bytes,
                        &ranges,
                    )
                }
            };
            if let Err(error) = write_result {
                self.presentation_stopped = true;
                self.convergence
                    .fail_hidden(ComputeConvergenceFailurePhase::Upload, error.to_string());
                self.release_hidden_scene_gpu_resources_with(|resources| {
                    release_scene_gpu_resources(device, resources)
                })?;
                return Err(ComputeRenderPathError::WriteSceneMemory(error));
            }
            // Timing reporting must not interrupt the CPU/GPU revision commit.
            let elapsed = upload_started_at;
            self.install_incremental_candidate(candidate_revision)?;
            self.record_timing_with_bytes(
                ComputeTimingPhase::Upload,
                candidate_stamp,
                elapsed,
                uploaded_bytes,
            )?;
            self.record_timing(
                ComputeTimingPhase::Installation,
                candidate_stamp,
                installation_started_at,
            )?;
            return Ok(());
        }
        let retired_bundle = match self.convergence.install_hidden(self.scene_gpu_revision) {
            Ok(Some(bundle)) => bundle,
            Ok(None) => {
                self.release_hidden_scene_gpu_resources_with(|resources| {
                    release_scene_gpu_resources(device, resources);
                })?;
                return Ok(());
            }
            Err(error) => {
                self.release_hidden_scene_gpu_resources_with(|resources| {
                    release_scene_gpu_resources(device, resources);
                })?;
                self.convergence.fail_hidden(
                    ComputeConvergenceFailurePhase::Installation,
                    error.to_string(),
                );
                return Err(error.into());
            }
        };
        let candidate = self
            .hidden_scene_gpu_resources
            .take()
            .ok_or(ComputeRenderPathError::MissingHiddenCandidate)?;
        let resources = candidate
            .resources
            .ok_or(ComputeRenderPathError::MissingHiddenCandidate)?;
        let scene_buffer = [vk::DescriptorBufferInfo::default()
            .buffer(resources.buffer)
            .offset(0)
            .range(candidate.range)];
        let writes = [vk::WriteDescriptorSet::default()
            .dst_set(self.descriptor_set)
            .dst_binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&scene_buffer)];
        unsafe { device.update_descriptor_sets(&writes) };
        let retired_resources = ComputeSceneGpuResources {
            buffer: std::mem::replace(&mut self.scene_buffer, resources.buffer),
            memory: std::mem::replace(&mut self.scene_memory, resources.memory),
            allocation_bytes: std::mem::replace(
                &mut self.scene_allocation_bytes,
                resources.allocation_bytes,
            ),
        };
        self.scene_gpu_revision = candidate_revision;
        if let Some(control) = &self.convergence_control {
            control.clear_post_upload_revision(candidate_revision)?;
        }
        release_scene_gpu_resources(device, retired_resources);
        drop(retired_bundle);
        self.record_timing(
            ComputeTimingPhase::Installation,
            candidate_stamp,
            installation_started_at,
        )?;
        Ok(())
    }

    fn install_incremental_candidate(
        &mut self,
        revision: VoxelSceneRevision,
    ) -> Result<(), ComputeRenderPathError> {
        self.convergence
            .install_hidden(self.scene_gpu_revision)?
            .ok_or(ComputeRenderPathError::MissingHiddenCandidate)?;
        self.scene_gpu_revision = revision;
        self.hidden_scene_gpu_resources = None;
        if let Some(control) = &self.convergence_control {
            control.clear_post_upload_revision(revision)?;
        }
        Ok(())
    }

    pub(super) fn release_hidden_scene_gpu_resources_with(
        &mut self,
        mut release: impl FnMut(ComputeSceneGpuResources),
    ) -> Result<(), ComputeConvergenceControlError> {
        let Some(candidate) = self.hidden_scene_gpu_resources.take() else {
            return Ok(());
        };
        let control_result = self
            .convergence_control
            .as_ref()
            .map(|control| control.clear_post_upload_revision(candidate.stamp.revision()))
            .transpose();
        if let Some(resources) = candidate.resources {
            release(resources);
        }
        control_result?;
        Ok(())
    }

    pub(super) fn owned_resource_counts(
        &self,
    ) -> Result<ComputeOwnedResourceCounts, ComputeRenderPathError> {
        let hidden_resources = self
            .hidden_scene_gpu_resources
            .as_ref()
            .and_then(|candidate| candidate.resources.as_ref());
        let objects = [
            self.output_image != vk::Image::null(),
            self.output_view != vk::ImageView::null(),
            self.sampler != vk::Sampler::null(),
            self.scene_buffer != vk::Buffer::null(),
            hidden_resources.is_some_and(|resources| resources.buffer != vk::Buffer::null()),
            self.camera_buffer != vk::Buffer::null(),
            self.semantic_ray_buffer != vk::Buffer::null(),
            self.descriptor_set_layout != vk::DescriptorSetLayout::null(),
            self.descriptor_pool != vk::DescriptorPool::null(),
            self.descriptor_set != vk::DescriptorSet::null(),
            self.compute_pipeline_layout != vk::PipelineLayout::null(),
            self.compute_pipeline != vk::Pipeline::null(),
            self.render_pass != vk::RenderPass::null(),
            self.composite_pipeline_layout != vk::PipelineLayout::null(),
            self.composite_pipeline != vk::Pipeline::null(),
        ]
        .into_iter()
        .filter(|owned| *owned)
        .count()
            + self.framebuffers.len();
        let allocations = [
            self.output_memory != vk::DeviceMemory::null(),
            self.scene_memory != vk::DeviceMemory::null(),
            hidden_resources.is_some_and(|resources| resources.memory != vk::DeviceMemory::null()),
            self.camera_memory != vk::DeviceMemory::null(),
            self.semantic_ray_memory != vk::DeviceMemory::null(),
        ]
        .into_iter()
        .filter(|owned| *owned)
        .count();
        let hidden_allocation_bytes = hidden_resources
            .map(|resources| resources.allocation_bytes)
            .unwrap_or(0);
        let bytes = self
            .output_allocation_bytes
            .checked_add(self.scene_allocation_bytes)
            .and_then(|bytes| bytes.checked_add(hidden_allocation_bytes))
            .and_then(|bytes| bytes.checked_add(self.camera_allocation_bytes))
            .and_then(|bytes| bytes.checked_add(self.semantic_ray_allocation_bytes))
            .ok_or(ComputeRenderPathError::ResourceAccountingOverflow)?;
        Ok(ComputeOwnedResourceCounts {
            bytes,
            objects,
            allocations,
            workers: self.convergence.status().worker_count(),
            views: self.convergence.owned_view_count(),
        })
    }

    pub(super) fn record_resource_observation(
        &self,
        point: ComputeResourceObservationPoint,
    ) -> Result<(), ComputeRenderPathError> {
        let Some(controller) = &self.lifecycle_controller else {
            return Ok(());
        };
        controller.record_resources(
            point,
            self.convergence.installed_bundle().scene_identity().clone(),
            self.convergence.status(),
            self.owned_resource_counts()?,
        )?;
        Ok(())
    }
}
