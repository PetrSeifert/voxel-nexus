use std::sync::Arc;

const PAGE_WORDS: usize = 1024;

// Path copying keeps an edit from cloning the scene-sized page directory.
#[derive(Clone, Debug)]
pub(crate) enum SceneWords {
    Leaf(Arc<[u32]>),
    Branch {
        left: Arc<Self>,
        right: Arc<Self>,
        length: usize,
    },
}

impl SceneWords {
    pub(crate) fn new(words: &[u32]) -> Self {
        if words.len() <= PAGE_WORDS {
            Self::Leaf(Arc::from(words))
        } else {
            let middle = words.len() / 2;
            let (left, right) = words.split_at(middle);
            Self::Branch {
                left: Arc::new(Self::new(left)),
                right: Arc::new(Self::new(right)),
                length: words.len(),
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Leaf(words) => words.len(),
            Self::Branch { length, .. } => *length,
        }
    }

    pub(crate) fn get(&self, index: usize) -> Option<u32> {
        match self {
            Self::Leaf(words) => words.get(index).copied(),
            Self::Branch { left, right, .. } => {
                if index < left.len() {
                    left.get(index)
                } else {
                    right.get(index.checked_sub(left.len())?)
                }
            }
        }
    }

    pub(crate) fn set(&mut self, index: usize, value: u32) -> Option<()> {
        match self {
            Self::Leaf(words) => {
                *Arc::make_mut(words).get_mut(index)? = value;
                Some(())
            }
            Self::Branch { left, right, .. } => {
                if index < left.len() {
                    Arc::make_mut(left).set(index, value)
                } else {
                    let index = index.checked_sub(left.len())?;
                    Arc::make_mut(right).set(index, value)
                }
            }
        }
    }

    pub(crate) fn flatten(&self) -> Vec<u32> {
        let mut words = Vec::with_capacity(self.len());
        self.append(&mut words);
        words
    }

    fn append(&self, words: &mut Vec<u32>) {
        match self {
            Self::Leaf(page) => words.extend_from_slice(page),
            Self::Branch { left, right, .. } => {
                left.append(words);
                right.append(words);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_copies_only_the_branch_containing_its_page() -> Result<(), &'static str> {
        let original = SceneWords::new(&vec![0; PAGE_WORDS * 16]);
        let mut successor = original.clone();
        successor.set(0, 7).ok_or("missing word")?;
        assert_eq!(original.get(0), Some(0));
        assert_eq!(successor.get(0), Some(7));
        let (
            SceneWords::Branch {
                right: original_right,
                ..
            },
            SceneWords::Branch {
                right: successor_right,
                ..
            },
        ) = (&original, &successor)
        else {
            return Err("missing branches");
        };
        assert!(Arc::ptr_eq(original_right, successor_right));
        assert_eq!(successor.get(successor.len()), None);
        Ok(())
    }
}
