use std::{
    mem,
    ops::DerefMut,
    cmp::Ordering,
};

use futures::{
    channel::{
        mpsc,
    },
    StreamExt,
};

use crate::{
    kv,
    storage,
    core::{
        KeyValueRef,
    },
};

pub struct KeyValuesIter {
    key_values_rx: mpsc::Receiver<KeyValueRef>,
    iter_state: IterState,
    advance_next_idx: Option<usize>,
}

impl KeyValuesIter {
    pub fn new(key_values_rx: mpsc::Receiver<KeyValueRef>) -> KeyValuesIter {
        KeyValuesIter {
            key_values_rx,
            iter_state: IterState::NotReady,
            advance_next_idx: None,
        }
    }
}

pub struct ItersMerger<V> {
    iters: V,
    advance_head_idx: Option<usize>,
}

enum IterState {
    NotReady,
    FrontItem(kv::KeyValuePair<storage::OwnedValueBlockRef>),
}

#[derive(Debug)]
pub enum Error {
    BackendIterPeerLost,
}

impl<V> ItersMerger<V> {
    pub fn new(iters: V) -> ItersMerger<V> {
        ItersMerger {
            iters,
            advance_head_idx: None,
        }
    }
}

impl<V> ItersMerger<V> where V: DerefMut<Target = Vec<KeyValuesIter>> {
    pub async fn next(&mut self) -> Result<Option<kv::KeyValuePair<storage::OwnedValueBlockRef>>, Error> {
        self.next_with_deprecated(|_| ()).await
    }

    pub async fn next_with_deprecated<F>(
        &mut self,
        mut deprecated: F,
    )
        -> Result<Option<kv::KeyValuePair<storage::OwnedValueBlockRef>>, Error>
    where F: FnMut(kv::KeyValuePair<storage::OwnedValueBlockRef>)
    {
        assert!(self.advance_head_idx.is_none());
        let mut cursor_idx = 0;
        while cursor_idx < self.iters.len() {
            match self.iters[cursor_idx].iter_state {
                IterState::NotReady => {
                    let current_iter = &mut self.iters[cursor_idx];
                    loop {
                        match current_iter.key_values_rx.next().await {
                            None =>
                                return Err(Error::BackendIterPeerLost),
                            Some(KeyValueRef::NoMore) => {
                                self.iters.swap_remove(cursor_idx);
                            },
                            Some(KeyValueRef::BlockFinish(..)) =>
                                continue,
                            Some(KeyValueRef::Item { key, value_cell, }) => {
                                current_iter.iter_state = IterState::FrontItem(kv::KeyValuePair { key, value_cell, });
                            },
                        }
                        break;
                    }
                    continue;
                },
                IterState::FrontItem(ref front_item) => {
                    match self.advance_head_idx {
                        None => {
                            self.iters[cursor_idx].advance_next_idx = None;
                            self.advance_head_idx = Some(cursor_idx);
                        },
                        Some(advance_head_idx) =>
                            match (&self.iters[advance_head_idx], front_item) {
                                (
                                    KeyValuesIter { iter_state: IterState::FrontItem(kv::KeyValuePair { key: key_min, .. }), .. },
                                    kv::KeyValuePair { key: key_cur, .. },
                                ) =>
                                    match key_cur.key_bytes.cmp(&key_min.key_bytes) {
                                        Ordering::Less => {
                                            self.iters[cursor_idx].advance_next_idx = None;
                                            self.advance_head_idx = Some(cursor_idx);
                                        },
                                        Ordering::Equal => {
                                            self.iters[cursor_idx].advance_next_idx = Some(advance_head_idx);
                                            self.advance_head_idx = Some(cursor_idx);
                                        },
                                        Ordering::Greater =>
                                            (),
                                    },
                                (KeyValuesIter { iter_state: IterState::NotReady, .. }, _) =>
                                    unreachable!(),
                            },
                    }
                    cursor_idx += 1;
                },
            }
        }

        let mut best_item = None;
        while let Some(advance_head_idx) = self.advance_head_idx {
            let current_iter = &mut self.iters[advance_head_idx];
            match mem::replace(&mut current_iter.iter_state, IterState::NotReady) {
                IterState::NotReady =>
                    unreachable!(),
                IterState::FrontItem(front_item) =>
                    match best_item {
                        None =>
                            best_item = Some(front_item),
                        Some(prev_best) =>
                            if prev_best.value_cell.version < front_item.value_cell.version {
                                deprecated(prev_best);
                                best_item = Some(front_item);
                            } else {
                                deprecated(front_item);
                                best_item = Some(prev_best);
                            },
                    },
            }
            self.advance_head_idx = current_iter.advance_next_idx;
        }

        Ok(best_item)
    }
}
