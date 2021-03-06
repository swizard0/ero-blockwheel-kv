#[forbid(unsafe_code)]

use std::{
    ops::{
        AddAssign,
        RangeBounds,
    },
};

use futures::{
    channel::{
        mpsc,
    },
};

use ero::{
    supervisor::SupervisorPid,
};

use alloc_pool::bytes::BytesPool;

use ero_blockwheel_fs as blockwheel;

pub mod kv;
pub mod job;
pub mod wheels;
pub mod version;

mod core;
mod storage;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct Params {
    pub tree_block_size: usize,
    pub butcher_task_restart_sec: usize,
    pub manager_task_restart_sec: usize,
    pub search_tree_task_restart_sec: usize,
    pub search_tree_remove_tasks_limit: usize,
    pub search_tree_iter_send_buffer: usize,
    pub search_tree_values_inline_size_limit: usize,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            tree_block_size: 32,
            butcher_task_restart_sec: 1,
            manager_task_restart_sec: 1,
            search_tree_task_restart_sec: 1,
            search_tree_remove_tasks_limit: 64,
            search_tree_iter_send_buffer: 4,
            search_tree_values_inline_size_limit: 128,
        }
    }
}

pub struct GenServer {
    manager_gen_server: core::manager::GenServer,
    manager_pid: core::manager::Pid,
}

#[derive(Clone)]
pub struct Pid {
    manager_pid: core::manager::Pid,
}

impl GenServer {
    pub fn new() -> GenServer {
        let manager_gen_server = core::manager::GenServer::new();
        let manager_pid = manager_gen_server.pid();
        GenServer {
            manager_gen_server,
            manager_pid,
        }
    }

    pub fn pid(&self) -> Pid {
        Pid {
            manager_pid: self.manager_pid.clone(),
        }
    }

    pub async fn run<J>(
        self,
        mut parent_supervisor: SupervisorPid,
        thread_pool: edeltraud::Edeltraud<J>,
        blocks_pool: BytesPool,
        version_provider: version::Provider,
        wheels_pid: wheels::Pid,
        params: Params,
    )
    where J: edeltraud::Job + From<job::Job>,
          J::Output: From<job::JobOutput>,
          job::JobOutput: From<J::Output>,
    {
        let butcher_gen_server = core::butcher::GenServer::new();
        let butcher_pid = butcher_gen_server.pid();
        let butcher_params = core::butcher::Params {
            tree_block_size: params.tree_block_size,
            task_restart_sec: params.butcher_task_restart_sec,
        };

        let manager_params = core::manager::Params {
            task_restart_sec: params.manager_task_restart_sec,
            search_tree_params: core::search_tree::Params {
                task_restart_sec: params.search_tree_task_restart_sec,
                tree_block_size: params.tree_block_size,
                remove_tasks_limit: params.search_tree_remove_tasks_limit,
                iter_send_buffer: params.search_tree_iter_send_buffer,
                values_inline_size_limit: params.search_tree_values_inline_size_limit,
            },
        };

        let child_supervisor_gen_server = parent_supervisor.child_supervisor();
        let child_supervisor_pid = child_supervisor_gen_server.pid();
        parent_supervisor.spawn_link_permanent(
            child_supervisor_gen_server.run(),
        );
        parent_supervisor.spawn_link_permanent(
            butcher_gen_server.run(
                version_provider.clone(),
                self.manager_pid.clone(),
                butcher_params,
            ),
        );

        let manager_task = self.manager_gen_server.run(
            child_supervisor_pid.clone(),
            thread_pool,
            blocks_pool,
            butcher_pid,
            wheels_pid,
            manager_params,
        );
        manager_task.await
    }
}

#[derive(Debug)]
pub enum InsertError {
    GenServer(ero::NoProcError),
}

#[derive(Debug)]
pub enum LookupError {
    GenServer(ero::NoProcError),
}

#[derive(Debug)]
pub enum LookupRangeError {
    GenServer(ero::NoProcError),
}

#[derive(Debug)]
pub enum RemoveError {
    GenServer(ero::NoProcError),
}

#[derive(Debug)]
pub enum FlushError {
    GenServer(ero::NoProcError),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Inserted {
    pub version: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Removed {
    pub version: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Flushed;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct Info {
    pub alive_cells_count: usize,
    pub tombstones_count: usize,
}

pub struct LookupRange {
    pub key_values_rx: mpsc::Receiver<KeyValueStreamItem>,
}

#[derive(Clone)]
pub enum KeyValueStreamItem {
    KeyValue(kv::KeyValuePair<kv::Value>),
    NoMore,
}

impl Pid {
    pub async fn info(&mut self) -> Result<Info, ero::NoProcError> {
        self.manager_pid.info().await
    }

    pub async fn insert(&mut self, key: kv::Key, value: kv::Value) -> Result<Inserted, InsertError> {
        self.manager_pid.insert(key, value).await
            .map_err(|core::manager::InsertError::GenServer(ero::NoProcError)| InsertError::GenServer(ero::NoProcError))
    }

    pub async fn lookup(&mut self, key: kv::Key) -> Result<Option<kv::ValueCell<kv::Value>>, LookupError> {
        self.manager_pid.lookup(key).await
            .map_err(|core::manager::LookupError::GenServer(ero::NoProcError)| LookupError::GenServer(ero::NoProcError))
    }

    pub async fn lookup_range<R>(&mut self, range: R) -> Result<LookupRange, LookupRangeError> where R: RangeBounds<kv::Key> {
        self.manager_pid.lookup_range(range).await
            .map_err(|core::manager::LookupRangeError::GenServer(ero::NoProcError)| LookupRangeError::GenServer(ero::NoProcError))
    }

    pub async fn remove(&mut self, key: kv::Key) -> Result<Removed, RemoveError> {
        self.manager_pid.remove(key).await
            .map_err(|core::manager::RemoveError::GenServer(ero::NoProcError)| RemoveError::GenServer(ero::NoProcError))
    }

    pub async fn flush(&mut self) -> Result<Flushed, FlushError> {
        self.manager_pid.flush_all().await
            .map_err(|core::manager::FlushError::GenServer(ero::NoProcError)| FlushError::GenServer(ero::NoProcError))
    }
}

impl AddAssign for Info {
    fn add_assign(&mut self, rhs: Info) {
        self.alive_cells_count += rhs.alive_cells_count;
        self.tombstones_count += rhs.tombstones_count;
    }
}

impl Info {
    pub fn reset(&mut self) {
        self.alive_cells_count = 0;
        self.tombstones_count = 0;
    }
}
