use crate::storage_counters::record_copied_node;
use std::collections::BTreeMap;
use std::sync::Arc;

// Each copied node has at most 64 entries, regardless of the volume's size.
const LEVEL_BITS: u32 = 6;
const LEVEL_MASK: usize = (1 << LEVEL_BITS) - 1;

#[derive(Clone)]
enum Node<T> {
    Branch(BTreeMap<usize, Arc<Node<T>>>),
    Leaf(BTreeMap<usize, T>),
}

#[derive(Clone)]
pub(super) struct PageTable<T> {
    root: Arc<Node<T>>,
    shift: u32,
}

impl<T: Clone> PageTable<T> {
    pub(super) fn new(page_count: usize) -> Self {
        let bits = usize::BITS - page_count.saturating_sub(1).leading_zeros();
        let shift = bits.saturating_sub(1) / LEVEL_BITS * LEVEL_BITS;
        Self {
            root: Arc::new(Node::empty(shift)),
            shift,
        }
    }

    pub(super) fn get(&self, index: &usize) -> Option<&T> {
        self.root.get(*index, self.shift)
    }

    pub(super) fn get_mut(&mut self, index: &usize) -> Option<&mut T> {
        make_node_mut(&mut self.root).get_mut(*index, self.shift)
    }

    pub(super) fn insert(&mut self, index: usize, value: T) {
        make_node_mut(&mut self.root).insert(index, self.shift, value);
    }

    pub(super) fn remove(&mut self, index: &usize) {
        make_node_mut(&mut self.root).remove(*index, self.shift);
    }

    #[cfg(test)]
    pub(super) fn depth(&self) -> usize {
        (self.shift / LEVEL_BITS) as usize + 1
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (usize, &T)> {
        self.root.iter(0, self.shift)
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &T> {
        self.iter().map(|(_, value)| value)
    }

    pub(super) fn storage_bytes(&self) -> usize {
        size_of::<Self>() + self.root.storage_bytes()
    }
}

fn make_node_mut<T: Clone>(node: &mut Arc<Node<T>>) -> &mut Node<T> {
    if Arc::get_mut(node).is_none() {
        record_copied_node();
    }
    Arc::make_mut(node)
}

impl<T: Clone> Node<T> {
    fn empty(shift: u32) -> Self {
        if shift == 0 {
            Self::Leaf(BTreeMap::new())
        } else {
            Self::Branch(BTreeMap::new())
        }
    }

    fn get(&self, index: usize, shift: u32) -> Option<&T> {
        let slot = (index >> shift) & LEVEL_MASK;
        match self {
            Self::Leaf(values) => values.get(&slot),
            Self::Branch(children) => children.get(&slot)?.get(index, shift - LEVEL_BITS),
        }
    }

    fn get_mut(&mut self, index: usize, shift: u32) -> Option<&mut T> {
        let slot = (index >> shift) & LEVEL_MASK;
        match self {
            Self::Leaf(values) => values.get_mut(&slot),
            Self::Branch(children) => {
                make_node_mut(children.get_mut(&slot)?).get_mut(index, shift - LEVEL_BITS)
            }
        }
    }

    fn insert(&mut self, index: usize, shift: u32, value: T) {
        let slot = (index >> shift) & LEVEL_MASK;
        match self {
            Self::Leaf(values) => {
                values.insert(slot, value);
            }
            Self::Branch(children) => {
                let child = children
                    .entry(slot)
                    .or_insert_with(|| Arc::new(Self::empty(shift - LEVEL_BITS)));
                make_node_mut(child).insert(index, shift - LEVEL_BITS, value);
            }
        }
    }

    fn remove(&mut self, index: usize, shift: u32) -> bool {
        let slot = (index >> shift) & LEVEL_MASK;
        match self {
            Self::Leaf(values) => {
                values.remove(&slot);
                values.is_empty()
            }
            Self::Branch(children) => {
                if let Some(child) = children.get_mut(&slot)
                    && make_node_mut(child).remove(index, shift - LEVEL_BITS)
                {
                    children.remove(&slot);
                }
                children.is_empty()
            }
        }
    }

    fn iter(&self, prefix: usize, shift: u32) -> Box<dyn Iterator<Item = (usize, &T)> + '_> {
        match self {
            Self::Leaf(values) => Box::new(
                values
                    .iter()
                    .map(move |(slot, value)| (prefix | slot, value)),
            ),
            Self::Branch(children) => Box::new(children.iter().flat_map(move |(slot, child)| {
                child.iter(prefix | (slot << shift), shift - LEVEL_BITS)
            })),
        }
    }

    fn storage_bytes(&self) -> usize {
        // BTreeMap capacity is private; estimate three pointers of overhead per entry.
        size_of::<Self>()
            + 2 * size_of::<usize>()
            + match self {
                Self::Leaf(values) => values.len() * (size_of::<T>() + 4 * size_of::<usize>()),
                Self::Branch(children) => {
                    children.len() * 5 * size_of::<usize>()
                        + children
                            .values()
                            .map(|child| child.storage_bytes())
                            .sum::<usize>()
                }
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copied_entries<T>(before: &Arc<Node<T>>, after: &Arc<Node<T>>) -> usize {
        if Arc::ptr_eq(before, after) {
            return 0;
        }
        match (before.as_ref(), after.as_ref()) {
            (Node::Leaf(values), Node::Leaf(_)) => values.len(),
            (Node::Branch(previous), Node::Branch(next)) => {
                previous.len()
                    + previous
                        .iter()
                        .map(|(slot, child)| {
                            next.get(slot).map_or(0, |next| copied_entries(child, next))
                        })
                        .sum::<usize>()
            }
            _ => 0,
        }
    }

    #[test]
    fn one_edit_copies_only_bounded_nodes_on_its_path() -> Result<(), Box<dyn std::error::Error>> {
        for count in [64, 4096, 32768] {
            let mut original = PageTable::new(count);
            for index in 0..count {
                original.insert(index, index);
            }
            let mut successor = original.clone();
            *successor.get_mut(&(count - 1)).ok_or("missing page")? = usize::MAX;
            assert_eq!(original.get(&(count - 1)), Some(&(count - 1)));
            assert_eq!(successor.get(&(count - 1)), Some(&usize::MAX));
            let copied = copied_entries(&original.root, &successor.root);
            assert!(copied <= 64 * (successor.shift as usize / LEVEL_BITS as usize + 1));
            if count == 32768 {
                assert_eq!(copied, 136);
            }
            for index in [0, 63, count - 1] {
                successor.remove(&index);
                assert!(successor.get(&index).is_none());
                assert_eq!(original.get(&index), Some(&index));
            }
        }
        Ok(())
    }
}
