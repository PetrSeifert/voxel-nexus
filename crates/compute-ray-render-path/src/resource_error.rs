use super::capabilities::ComputeRenderPathRejection;
#[cfg(any(test, feature = "qualification"))]
use super::compute_convergence::ComputeConvergenceFailurePhase;
use super::compute_convergence::{ComputeConvergenceControlError, ComputeConvergenceError};
use super::observation::{ComputeLifecycleControlError, ComputeMeasurementControlError};
use super::semantic_rays::ComputeSemanticRayControlError;
use ash::vk;
use thiserror::Error;

#[derive(Debug, Error)]
pub(super) enum ComputeRenderPathError {
    #[error(transparent)]
    Convergence(#[from] ComputeConvergenceError),
    #[error(transparent)]
    ConvergenceControl(#[from] ComputeConvergenceControlError),
    #[error(transparent)]
    SemanticRayControl(#[from] ComputeSemanticRayControlError),
    #[error(transparent)]
    Rejected(#[from] ComputeRenderPathRejection),
    #[error("could not create the compute output image: {0}")]
    CreateOutputImage(vk::Result),
    #[error("no device-local memory type can hold the compute output image")]
    MissingOutputMemory,
    #[error("could not allocate compute output memory: {0}")]
    AllocateOutputMemory(vk::Result),
    #[error("could not bind compute output memory: {0}")]
    BindOutputMemory(vk::Result),
    #[error("could not create the compute output image view: {0}")]
    CreateOutputView(vk::Result),
    #[error("could not create the compute output sampler: {0}")]
    CreateSampler(vk::Result),
    #[error(
        "the compute Voxel Scene needs a {required}-byte storage-buffer binding, but the device limit is {available} bytes"
    )]
    SceneStorageBufferRange { required: u64, available: u32 },
    #[error("could not create the compute Voxel Scene buffer: {0}")]
    CreateSceneBuffer(vk::Result),
    #[error("no host-visible coherent memory type can hold the compute Voxel Scene buffer")]
    MissingSceneMemory,
    #[error("could not allocate compute Voxel Scene buffer memory: {0}")]
    AllocateSceneMemory(vk::Result),
    #[error("could not bind compute Voxel Scene buffer memory: {0}")]
    BindSceneMemory(vk::Result),
    #[error("could not write the compute Voxel Scene buffer: {0}")]
    WriteSceneMemory(vk::Result),
    #[error("could not create the compute Camera State buffer: {0}")]
    CreateCameraBuffer(vk::Result),
    #[error("no host-visible coherent memory type can hold the compute Camera State buffer")]
    MissingCameraMemory,
    #[error("could not allocate compute Camera State buffer memory: {0}")]
    AllocateCameraMemory(vk::Result),
    #[error("could not bind compute Camera State buffer memory: {0}")]
    BindCameraMemory(vk::Result),
    #[error("could not write the compute Camera State buffer: {0}")]
    WriteCameraMemory(vk::Result),
    #[error("could not create the compute Semantic Ray buffer: {0}")]
    CreateSemanticRayBuffer(vk::Result),
    #[error("no host-visible coherent memory type can hold the compute Semantic Ray buffer")]
    MissingSemanticRayMemory,
    #[error("could not allocate compute Semantic Ray buffer memory: {0}")]
    AllocateSemanticRayMemory(vk::Result),
    #[error("could not bind compute Semantic Ray buffer memory: {0}")]
    BindSemanticRayMemory(vk::Result),
    #[error("could not write compute Semantic Ray input: {0}")]
    WriteSemanticRayMemory(vk::Result),
    #[error("could not read compute Semantic Ray output: {0}")]
    ReadSemanticRayMemory(vk::Result),
    #[error("the compute Semantic Ray buffer is unavailable")]
    SemanticRayResourcesUnavailable,
    #[error("compute Semantic Ray probe {probe_identity} returned invalid GPU data: {reason}")]
    InvalidSemanticRayOutput {
        probe_identity: String,
        reason: &'static str,
    },
    #[error("the compute Camera State buffer is unavailable")]
    CameraResourcesUnavailable,
    #[error("could not create the compute descriptor-set layout: {0}")]
    CreateDescriptorSetLayout(vk::Result),
    #[error("could not create the compute descriptor pool: {0}")]
    CreateDescriptorPool(vk::Result),
    #[error("could not allocate the compute descriptor set: {0}")]
    AllocateDescriptorSet(vk::Result),
    #[error("the compute descriptor allocation returned no set")]
    MissingDescriptorSet,
    #[error("could not read compiled shader code: {0}")]
    ReadShader(#[from] std::io::Error),
    #[error("could not create a shader module: {0}")]
    CreateShaderModule(vk::Result),
    #[error("could not create a pipeline layout: {0}")]
    CreatePipelineLayout(vk::Result),
    #[error("could not create the compute pipeline: {0}")]
    CreateComputePipeline(vk::Result),
    #[error("could not create the composite render pass: {0}")]
    CreateRenderPass(vk::Result),
    #[error("could not create the composite graphics pipeline: {0}")]
    CreateGraphicsPipeline(vk::Result),
    #[error("could not create a composite framebuffer: {0}")]
    CreateFramebuffer(vk::Result),
    #[error("the frame target does not match the configured presentation")]
    StaleFrameTarget,
    #[error("the frame target has no configured composite framebuffer")]
    MissingFramebuffer,
    #[error("the compute convergence hidden candidate is unavailable")]
    MissingHiddenCandidate,
    #[cfg(any(test, feature = "qualification"))]
    #[error("injected compute convergence {0:?} failure")]
    InjectedConvergenceFailure(ComputeConvergenceFailurePhase),
    #[error("compute shutdown failed: {0}")]
    Shutdown(String),
    #[error(transparent)]
    Measurement(#[from] ComputeMeasurementControlError),
    #[error(transparent)]
    LifecycleControl(#[from] ComputeLifecycleControlError),
    #[error("compute resource byte accounting overflowed")]
    ResourceAccountingOverflow,
}
