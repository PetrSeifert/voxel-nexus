use thiserror::Error;
use voxel_frontend::{VoxelSceneView, VoxelVolumeId};

#[derive(Debug, Error)]
pub enum BrickmapValidationError {
    #[error(
        "brickmap {limit} is {available} bytes, but the packed coarse grid, pool and metadata require {required} bytes"
    )]
    BufferLimit {
        limit: &'static str,
        required: u64,
        available: u64,
    },
    #[error("brickmap Voxel Volume {volume:?} exceeds the spatial envelope: {reason}")]
    Envelope {
        volume: VoxelVolumeId,
        reason: &'static str,
    },
}

pub(crate) fn validate_buffer_sizes(
    required: u64,
    budget: u64,
    storage: u64,
    buffer: u64,
) -> Result<(), BrickmapValidationError> {
    for (limit, available) in [
        ("configured budget", budget),
        ("maxStorageBufferRange", storage),
        ("maxBufferSize", buffer),
    ] {
        if required > available {
            return Err(BrickmapValidationError::BufferLimit {
                limit,
                required,
                available,
            });
        }
    }
    Ok(())
}

pub(crate) fn validate_view(
    view: &VoxelSceneView,
    budget_bytes: u64,
) -> Result<(), BrickmapValidationError> {
    let mut minimum_bytes = 20u64;
    for volume in view.volumes() {
        let reject = |reason| BrickmapValidationError::Envelope {
            volume: volume.identity().clone(),
            reason,
        };
        let size = volume.voxel_size();
        if !(0.125..=16.0).contains(&size) {
            return Err(reject("voxel size must be in [0.125, 16]"));
        }
        if volume
            .extent()
            .dimensions()
            .into_iter()
            .any(|extent| extent > 65536)
        {
            return Err(reject("each local extent must be at most 65536 voxels"));
        }
        let coarse_bytes = volume
            .extent()
            .dimensions()
            .into_iter()
            .map(|extent| u64::from(extent.div_ceil(8)))
            .product::<u64>()
            * 4;
        minimum_bytes = minimum_bytes
            .saturating_add(32)
            .saturating_add(coarse_bytes);
        validate_buffer_sizes(minimum_bytes, budget_bytes, u64::MAX, u64::MAX)?;
        for (origin, extent) in volume
            .scene_origin()
            .into_iter()
            .zip(volume.extent().dimensions())
        {
            if origin.abs() > 65536.0 {
                return Err(reject("absolute scene origin must be at most 65536"));
            }
            let maximum = f64::from(origin) + f64::from(extent) * f64::from(size);
            if maximum.abs() > 131072.0 {
                return Err(reject(
                    "final scene-space bounds must lie in [-131072, 131072]",
                ));
            }
        }
    }
    Ok(())
}
