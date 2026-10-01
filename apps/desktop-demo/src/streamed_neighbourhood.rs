use std::fmt;

/// An odd-sided square of Voxel Volumes centred on the camera's volume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StreamedNeighbourhood {
    side: u32,
}

impl StreamedNeighbourhood {
    pub(super) const DEFAULT: Self = Self { side: 7 };
    /// The recorded residency qualification ratified its caps for this size.
    // Only the residency qualification binary and tests select it.
    #[allow(dead_code)]
    pub(super) const QUALIFICATION: Self = Self { side: 3 };
    // A one-volume square leaves no reach beyond the camera's own volume at a boundary.
    const MINIMUM_SIDE: u32 = 3;
    // Larger squares would rebuild most of the 16x16 scene on every crossing.
    const MAXIMUM_SIDE: u32 = 15;

    pub(super) fn new(side: u32) -> Result<Self, String> {
        if side.is_multiple_of(2) {
            return Err(format!(
                "StreamedNeighbourhoodEven: neighbourhood size {side} must be odd so the camera's volume stays centred"
            ));
        }
        if !(Self::MINIMUM_SIDE..=Self::MAXIMUM_SIDE).contains(&side) {
            return Err(format!(
                "StreamedNeighbourhoodOutOfRange: neighbourhood size {side} must be between {} and {}",
                Self::MINIMUM_SIDE,
                Self::MAXIMUM_SIDE
            ));
        }
        Ok(Self { side })
    }

    pub(super) fn parse(argument: Option<&str>) -> Result<Self, String> {
        let argument = argument
            .ok_or("StreamedNeighbourhoodMissing: --streamed-neighbourhood requires an odd size")?;
        let side = argument.parse::<u32>().map_err(|error| {
            format!("StreamedNeighbourhoodInvalid: neighbourhood size {argument:?}: {error}")
        })?;
        Self::new(side)
    }

    /// Volumes selected on each side of the camera's volume.
    pub(super) fn reach(self) -> u32 {
        self.side / 2
    }

    pub(super) fn volume_count(self) -> usize {
        let side = self.side as usize;
        side * side
    }
}

impl fmt::Display for StreamedNeighbourhood {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{0}x{0}", self.side)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_odd_sizes_and_names_every_rejection() -> Result<(), String> {
        assert_eq!(StreamedNeighbourhood::DEFAULT.volume_count(), 49);
        assert_eq!(StreamedNeighbourhood::DEFAULT.reach(), 3);
        assert_eq!(StreamedNeighbourhood::DEFAULT.to_string(), "7x7");
        assert_eq!(StreamedNeighbourhood::QUALIFICATION.volume_count(), 9);
        for side in [3, 5, 7, 15] {
            assert_eq!(
                StreamedNeighbourhood::parse(Some(&side.to_string()))?.reach(),
                side / 2
            );
        }
        for (argument, reason) in [
            (Some("4"), "StreamedNeighbourhoodEven"),
            (Some("0"), "StreamedNeighbourhoodEven"),
            (Some("1"), "StreamedNeighbourhoodOutOfRange"),
            (Some("17"), "StreamedNeighbourhoodOutOfRange"),
            (Some("-7"), "StreamedNeighbourhoodInvalid"),
            (Some("seven"), "StreamedNeighbourhoodInvalid"),
            (None, "StreamedNeighbourhoodMissing"),
        ] {
            let error =
                StreamedNeighbourhood::parse(argument).expect_err("the size must be rejected");
            assert!(error.starts_with(reason), "{argument:?}: {error}");
        }
        Ok(())
    }
}
