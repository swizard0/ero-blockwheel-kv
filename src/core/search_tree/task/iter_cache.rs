use std::sync::Arc;

use futures::{
    channel::{
        mpsc,
        oneshot,
    },
    SinkExt,
};

use crate::{
    kv,
    core::{
        MemCache,
        search_tree::{
            SearchTreeIterItemsRx,
        },
    },
    KeyValueStreamItem,
};

pub struct Args {
    pub cache: Arc<MemCache>,
    pub reply_tx: oneshot::Sender<SearchTreeIterItemsRx>,
}

pub struct Done;

#[derive(Debug)]
pub enum Error {
}

pub async fn run(Args { cache, reply_tx, }: Args) -> Result<Done, Error> {
    let (mut iter_tx, iter_rx) = mpsc::channel(0);
    let iter = SearchTreeIterItemsRx { items_rx: iter_rx, };
    if let Err(_send_error) = reply_tx.send(iter) {
        log::warn!("client canceled iter request");
    } else {
        for (ord_key, value_cell) in cache.iter() {
            let kv_pair = kv::KeyValuePair {
                key: ord_key.as_ref().clone(),
                value_cell: value_cell.clone(),
            };
            if let Err(_send_error) = iter_tx.send(KeyValueStreamItem::KeyValue(kv_pair)).await {
                return Ok(Done);
            }
        }
        iter_tx.send(KeyValueStreamItem::NoMore).await.ok();
    }
    Ok(Done)
}
