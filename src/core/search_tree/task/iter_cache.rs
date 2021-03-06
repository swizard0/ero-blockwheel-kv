use std::sync::Arc;

use futures::{
    stream,
    SinkExt,
};

use alloc_pool::{
    pool,
    Unique,
};

use crate::{
    kv,
    job,
    storage,
    core::{
        search_tree::{
            KeyValueRef,
            task::{
                IterRequestData,
            },
        },
        MemCache,
        SearchRangeBounds,
    },
};

pub struct Args<J> where J: edeltraud::Job {
    pub cache: Arc<MemCache>,
    pub thread_pool: edeltraud::Edeltraud<J>,
    pub iter_cache_entries_pool: pool::Pool<Vec<kv::KeyValuePair<storage::OwnedValueBlockRef>>>,
    pub iter_request_data: IterRequestData,
}

pub struct Done;

#[derive(Debug)]
pub enum Error {
    ThreadPoolGone,
}

pub type JobOutput = Result<JobDone, Error>;

pub struct JobArgs {
    cache: Arc<MemCache>,
    iter_cache_entries_pool: pool::Pool<Vec<kv::KeyValuePair<storage::OwnedValueBlockRef>>>,
    range: SearchRangeBounds,
}

pub struct JobDone {
    items: Unique<Vec<kv::KeyValuePair<storage::OwnedValueBlockRef>>>,
}

pub fn job(JobArgs { cache, iter_cache_entries_pool, range, }: JobArgs) -> JobOutput {
    let mut items = iter_cache_entries_pool.lend(Vec::new);
    items.clear();
    items.reserve(cache.len());
    for kv_pair in cache.range(range) {
        items.push(kv::KeyValuePair {
            key: kv_pair.key,
            value_cell: kv_pair.value_cell.into(),
        });
    }

    items.shrink_to_fit();
    Ok(JobDone { items, })
}

pub async fn run<J>(
    Args {
        cache,
        thread_pool,
        iter_cache_entries_pool,
        iter_request_data: IterRequestData {
            range,
            mut iter_items_tx,
            repay_iter_items_tx,
        },
    }: Args<J>,
)
    -> Result<Done, Error>
where J: edeltraud::Job + From<job::Job>,
      J::Output: From<job::JobOutput>,
      job::JobOutput: From<J::Output>,
{
    let job_output = thread_pool.spawn(job::Job::SearchTreeIterCache(JobArgs { cache, iter_cache_entries_pool, range, })).await
        .map_err(|edeltraud::SpawnError::ThreadPoolGone| Error::ThreadPoolGone)?;
    let job_output: job::JobOutput = job_output.into();
    let job::SearchTreeIterCacheDone(job_result) = job_output.into();
    let JobDone { mut items, } = job_result?;

    let key_value_refs = items.drain(..)
        .map(|key_value| Ok(KeyValueRef::Item {
            key: key_value.key,
            value_cell: key_value.value_cell,
        }));
    let mut key_value_refs_stream = stream::iter(key_value_refs);
    if let Err(_send_error) = iter_items_tx.items_tx.send_all(&mut key_value_refs_stream).await {
        log::warn!("client canceled iter request");
        return Ok(Done);
    }

    repay_iter_items_tx.send(iter_items_tx).ok();
    Ok(Done)
}
