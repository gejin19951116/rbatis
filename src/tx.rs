use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use dashmap::mapref::one::RefMut;

use crate::core::db::DBPool;
use crate::core::db::DBTx;
use crate::plugin::log::LogPlugin;
use crate::rbatis::Rbatis;

///the Transaction manager，It manages the life cycle of transactions and provides access across threads
///every tx_check_interval check tx is out of time(tx_lock_wait_timeout).if out, rollback tx.
///if tx manager will be drop, manager will rollback all of tx.
#[derive(Debug)]
pub struct TxManager {
    pub tx_prefix: String,
    pub tx_context: DashMap<String, (DBTx, TxState)>,
    pub tx_lock_wait_timeout: Duration,
    pub tx_check_interval: Duration,
    pub alive: AtomicBool,
    pub log_plugin: Option<Arc<Box<dyn LogPlugin>>>,
}

impl Drop for TxManager {
    fn drop(&mut self) {
        self.set_alive(false);
    }
}

#[derive(Debug)]
pub enum TxState {
    StateBegin(Instant),
    StateFinish(Instant),
}

impl TxManager {
    pub fn new_arc(
        tx_prefix: &str,
        plugin: Arc<Box<dyn LogPlugin>>,
        tx_lock_wait_timeout: Duration,
        tx_check_interval: Duration,
    ) -> Arc<Self> {
        let s = Self {
            tx_prefix: tx_prefix.to_string(),
            tx_context: DashMap::new(),
            tx_lock_wait_timeout,
            tx_check_interval,
            alive: AtomicBool::new(true),
            log_plugin: Some(plugin),
        };
        let arc = Arc::new(s);
        TxManager::polling_check(arc.clone());
        arc
    }

    pub fn set_alive(&self, alive: bool) {
        self.alive
            .compare_exchange(!alive, alive, Ordering::Relaxed, Ordering::Relaxed);
    }

    pub fn get_alive(&self) -> bool {
        self.alive.fetch_or(false, Ordering::Relaxed)
    }

    pub fn close(&self) {
        if self.get_alive().eq(&true) {
            self.set_alive(false);
        }
    }

    fn is_enable_log(&self) -> bool {
        self.log_plugin.is_some() && self.log_plugin.as_ref().unwrap().is_enable()
    }

    fn do_log(&self, context_id: &str, arg: &str) {
        if self.is_enable_log() {
            match &self.log_plugin {
                Some(v) => {
                    v.info(context_id, arg);
                }
                _ => {}
            }
        }
    }

    ///polling check tx alive
    fn polling_check(manager: Arc<Self>) {
        crate::core::runtime::task::spawn(async move {
            loop {
                if manager.get_alive().eq(&false) {
                    //rollback all
                    let mut rollback_ids = vec![];
                    manager.tx_context.iter().for_each(|a| {
                        rollback_ids.push(a.key().to_string());
                    });
                    for context_id in &rollback_ids {
                        if manager.is_enable_log() {
                            manager.do_log(
                                context_id,
                                &format!(
                                    "[rbatis] rollback context_id:{},Because the manager exits",
                                    context_id
                                ),
                            );
                        }
                        manager.rollback(context_id).await;
                    }
                    break;
                }
                let mut need_rollback = None;
                manager.tx_context.iter().for_each(|a| {
                    let k = a.key();
                    let (tx, state) = a.value();
                    match state {
                        TxState::StateBegin(instant) => {
                            let out_time = instant.elapsed();
                            if out_time > manager.tx_lock_wait_timeout {
                                if need_rollback == None {
                                    need_rollback = Some(vec![]);
                                }
                                match &mut need_rollback {
                                    Some(v) => {
                                        v.push(k.to_string());
                                    }
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                });
                match &mut need_rollback {
                    Some(v) => {
                        for context_id in v {
                            if manager.is_enable_log() {
                                manager.do_log(
                                    context_id,
                                    &format!(
                                        "[rbatis] rollback context_id:{},out of time:{:?}",
                                        context_id, &manager.tx_lock_wait_timeout
                                    ),
                                );
                            }
                            manager.rollback(context_id).await;
                        }
                        //shrink_to_fit
                        manager.tx_context.shrink_to_fit();
                    }
                    _ => {}
                }
                crate::core::runtime::task::sleep(manager.tx_check_interval).await;
            }
            #[cfg(feature = "debug_mode")]
                {
                    match &manager.log_plugin {
                        Some(m) => {
                            m.info("", "[rbatis] TxManager exit!");
                        }
                        _ => {
                            log::info!("[rbatis] TxManager exit!");
                        }
                    }
                }
        });
    }

    pub async fn get_mut<'a>(
        &'a self,
        context_id: &str,
    ) -> Option<RefMut<'a, String, (DBTx, TxState)>> {
        let m = self.tx_context.get_mut(context_id);
        match m {
            Some(v) => {
                return Some(v);
            }
            None => {
                return None;
            }
        }
    }

    /// begin tx,for new conn
    pub async fn begin(
        &self,
        new_context_id: &str,
        pool: &DBPool,
    ) -> Result<String, crate::core::Error> {
        if new_context_id.is_empty() {
            return Err(crate::core::Error::from(
                "[rbatis] context_id can not be empty",
            ));
        }
        let conn: DBTx = pool.begin().await?;
        //send tx to context
        self.tx_context
            .insert(
                new_context_id.to_string(),
                (conn, TxState::StateBegin(Instant::now())),
            );
        if self.is_enable_log() {
            self.do_log(
                new_context_id,
                &format!("[rbatis] [{}] Begin", new_context_id),
            );
        }
        return Ok(new_context_id.to_string());
    }

    /// commit tx,and return conn
    pub async fn commit(&self, context_id: &str) -> Result<String, crate::core::Error> {
        let tx_op = self.tx_context.remove(context_id);
        if tx_op.is_none() {
            return Err(crate::core::Error::from(format!(
                "[rbatis] tx:{} not exist！",
                context_id
            )));
        }
        let (mut tx, state): (DBTx, TxState) = tx_op.unwrap().1;
        let result = tx.commit().await?;
        if self.is_enable_log() {
            self.do_log(context_id, &format!("[rbatis] [{}] Commit", context_id));
        }
        return Ok(context_id.to_string());
    }

    /// rollback tx,and return conn
    pub async fn rollback(&self, context_id: &str) -> Result<String, crate::core::Error> {
        let tx_op = self.tx_context.remove(context_id);
        if tx_op.is_none() {
            return Err(crate::core::Error::from(format!(
                "[rbatis] tx:{} not exist！",
                context_id
            )));
        }
        let (tx, state): (DBTx, TxState) = tx_op.unwrap().1;
        let result = tx.rollback().await?;
        if self.is_enable_log() {
            self.do_log(context_id, &format!("[rbatis] [{}] Rollback", context_id));
        }
        return Ok(context_id.to_string());
    }

    /// context_id is 'tx:' prifix ?
    pub fn is_tx_prifix_id(&self, context_id: &str) -> bool {
        return context_id.starts_with(&self.tx_prefix);
    }
}

/// the TxGuard just like an  Lock Guard,
/// if TxGuard Drop, this tx will be commit or rollback
#[derive(Debug)]
pub struct TxGuard {
    pub tx_id: String,
    pub is_drop_commit: bool,
    pub manager: Option<Arc<TxManager>>,
}

impl TxGuard {
    pub fn new(tx_id: &str, is_drop_commit: bool, manager: Arc<TxManager>) -> Self {
        Self {
            tx_id: tx_id.to_string(),
            is_drop_commit,
            manager: Some(manager),
        }
    }

    pub async fn try_commit(&mut self) -> Result<String, crate::core::Error> {
        match &mut self.manager {
            Some(m) => {
                let result = m.commit(&self.tx_id).await?;
                self.manager = None;
                return Ok(result);
            }
            _ => {}
        }
        return Result::Ok(self.tx_id.clone());
    }

    pub async fn try_rollback(&mut self) -> Result<String, crate::core::Error> {
        match &mut self.manager {
            Some(m) => {
                let result = m.rollback(&self.tx_id).await?;
                self.manager = None;
                return Ok(result);
            }
            _ => {}
        }
        return Result::Ok(self.tx_id.clone());
    }
}

impl Drop for TxGuard {
    fn drop(&mut self) {
        if self.manager.is_none() {
            return;
        }
        let tx_id = self.tx_id.clone();
        let is_drop_commit = self.is_drop_commit;
        let manager = self.manager.take().unwrap();
        crate::core::runtime::task::spawn(async move {
            if is_drop_commit {
                manager.commit(&tx_id).await;
            } else {
                manager.rollback(&tx_id).await;
            }
            drop(manager);
        });
    }
}
