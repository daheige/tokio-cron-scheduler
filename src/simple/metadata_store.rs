use crate::job_data::{JobAndNextTick, JobStoredData};
use crate::store::{DataStore, InitStore, MetaDataStorage};
use crate::JobSchedulerError;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

pub struct SimpleMetadataStore {
    pub data: Arc<RwLock<HashMap<Uuid, JobStoredData>>>,
    pub inited: bool,
}

impl DataStore<JobStoredData> for SimpleMetadataStore {
    fn get(
        &mut self,
        id: Uuid,
    ) -> Box<dyn Future<Output = Result<Option<JobStoredData>, JobSchedulerError>>> {
        let data = self.data.clone();
        Box::new(async move {
            let r = data.write().await;
            let val = r.get(&id).cloned();
            Ok(val)
        })
    }

    fn add_or_update(
        &mut self,
        data: JobStoredData,
    ) -> Box<dyn Future<Output = Result<(), JobSchedulerError>>> {
        let id: Uuid = data.id.as_ref().unwrap().into();
        let job_data = self.data.clone();
        Box::new(async move {
            let mut w = job_data.write().await;
            w.insert(id, data);
            Ok(())
        })
    }

    fn delete(&mut self, guid: Uuid) -> Box<dyn Future<Output = Result<(), JobSchedulerError>>> {
        let job_data = self.data.clone();
        Box::new(async move {
            let mut w = job_data.write().await;
            w.remove(&guid);
            Ok(())
        })
    }
}

impl InitStore for SimpleMetadataStore {
    fn init(&mut self) -> Box<dyn Future<Output = Result<(), JobSchedulerError>>> {
        self.inited = true;
        Box::new(std::future::ready(Ok(())))
    }

    fn inited(&mut self) -> Box<dyn Future<Output = Result<bool, JobSchedulerError>>> {
        let val = self.inited;
        Box::new(std::future::ready(Ok(val)))
    }
}

impl MetaDataStorage for SimpleMetadataStore {
    fn list_next_ticks(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<JobAndNextTick>, JobSchedulerError>> + Send>> {
        let data = self.data.clone();
        Box::pin(async move {
            let r = data.read().await;
            let ret = r
                .iter()
                .map(|(_, v)| (v.id.clone(), v.next_tick, v.last_tick))
                .map(|(id, next_tick, last_tick)| JobAndNextTick {
                    id,
                    next_tick,
                    last_tick,
                })
                .collect::<Vec<_>>();
            Ok(ret)
        })
    }

    fn set_next_tick(
        &mut self,
        guid: Uuid,
        next_tick: DateTime<Utc>,
    ) -> Box<dyn Future<Output = Result<(), JobSchedulerError>>> {
        let data = self.data.clone();
        Box::new(async move {
            let mut w = data.write().await;
            let val = w.get_mut(&guid);
            match val {
                Some(mut val) => {
                    val.next_tick = next_tick.timestamp() as u64;
                    Ok(())
                }
                None => Err(JobSchedulerError::UpdateJobData),
            }
        })
    }
}
