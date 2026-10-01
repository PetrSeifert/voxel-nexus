use std::collections::VecDeque;
use std::time::{Duration, Instant};
use voxel_frontend::VoxelResidencySelectionId;

/// Times each neighbourhood crossing from its selection request until the Presenting Render
/// Path installs that selection or a newer one.
#[derive(Debug, Default)]
pub(super) struct StreamedCrossingLatency {
    pending: VecDeque<(VoxelResidencySelectionId, Instant)>,
    last: Option<Duration>,
    worst: Option<Duration>,
}

impl StreamedCrossingLatency {
    pub(super) fn crossed(&mut self, selection: VoxelResidencySelectionId, at: Instant) {
        self.pending.push_back((selection, at));
    }

    /// Returns the latencies of the crossings this installation completed, oldest first.
    pub(super) fn installed(
        &mut self,
        selection: VoxelResidencySelectionId,
        at: Instant,
    ) -> Vec<Duration> {
        let mut completed = Vec::new();
        while let Some((crossing, started_at)) = self.pending.front().copied()
            && crossing <= selection
        {
            self.pending.pop_front();
            let latency = at.saturating_duration_since(started_at);
            self.last = Some(latency);
            self.worst = self.worst.max(Some(latency));
            completed.push(latency);
        }
        completed
    }

    pub(super) fn last(&self) -> Option<Duration> {
        self.last
    }

    pub(super) fn worst(&self) -> Option<Duration> {
        self.worst
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_installation_completes_every_older_crossing_and_keeps_the_worst() {
        let start = Instant::now();
        let at = |milliseconds| start + Duration::from_millis(milliseconds);
        let selection = VoxelResidencySelectionId::new;
        let mut latency = StreamedCrossingLatency::default();
        assert_eq!(latency.installed(selection(1), at(0)), []);
        latency.crossed(selection(2), at(10));
        latency.crossed(selection(3), at(30));
        assert_eq!(latency.installed(selection(1), at(40)), []);
        assert_eq!(
            latency.installed(selection(3), at(110)),
            [Duration::from_millis(100), Duration::from_millis(80)]
        );
        latency.crossed(selection(4), at(200));
        assert_eq!(
            latency.installed(selection(4), at(220)),
            [Duration::from_millis(20)]
        );
        assert_eq!(latency.last(), Some(Duration::from_millis(20)));
        assert_eq!(latency.worst(), Some(Duration::from_millis(100)));
    }
}
