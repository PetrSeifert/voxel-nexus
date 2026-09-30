use std::cell::Cell;

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum QualificationAllocationCategory {
    Control,
    Materialized,
    Generation,
    Raster,
    Brickmap,
    Metadata,
    History,
}

thread_local! {
    static CATEGORY: Cell<Option<QualificationAllocationCategory>> = const { Cell::new(None) };
}

pub fn qualification_allocation_category() -> Option<QualificationAllocationCategory> {
    CATEGORY.try_with(Cell::get).ok().flatten()
}

pub struct QualificationAllocationScope(Option<QualificationAllocationCategory>);

impl QualificationAllocationScope {
    pub fn enter(category: QualificationAllocationCategory) -> Self {
        Self(CATEGORY.with(|current| current.replace(Some(category))))
    }
}

impl Drop for QualificationAllocationScope {
    fn drop(&mut self) {
        CATEGORY.with(|current| current.set(self.0));
    }
}
