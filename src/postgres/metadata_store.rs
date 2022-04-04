use crate::job::job_data::{CronJob, JobType, NonCronJob};
use crate::postgres::PostgresStore;
use crate::store::{DataStore, InitStore, MetaDataStorage};
use crate::{JobAndNextTick, JobSchedulerError, JobStoredData, JobUuid};
use chrono::{DateTime, Utc};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_postgres::Row;
use tracing::error;
use uuid::Uuid;

const TABLE: &str = "job_data";

#[derive(Clone)]
pub struct PostgresMetadataStore {
    pub store: Arc<RwLock<PostgresStore>>,
    pub init_tables: bool,
    pub table: String,
}

impl Default for PostgresMetadataStore {
    fn default() -> Self {
        let init_tables = std::env::var("POSTGRES_INIT_METADATA")
            .map(|s| s.to_lowercase() == "true")
            .unwrap_or_default();
        let table =
            std::env::var("POSTGRES_METADATA_TABLE").unwrap_or_else(|_| TABLE.to_lowercase());
        Self {
            init_tables,
            table,
            ..Default::default()
        }
    }
}

impl DataStore<JobStoredData> for PostgresMetadataStore {
    fn get(
        &mut self,
        id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<Option<JobStoredData>, JobSchedulerError>> + Send>>
    {
        let store = self.store.clone();
        let table = self.table.clone();
        Box::pin(async move {
            let store = store.read().await;
            match &*store {
                PostgresStore::Created(_) => Err(JobSchedulerError::GetJobData),
                PostgresStore::Inited(store) => {
                    let store = store.read().await;
                    let sql = "select \
                        id, last_updated, next_tick, job_type, count, \
                        ran, stopped, schedule, repeating, repeating_every, \
                        extra \
                     from $1 where id = $2 limit 1";
                    let row = store.query_one(sql, &[&table, &id]).await;
                    if let Err(e) = row {
                        error!("Error getting value {:?}", e);
                        return Err(JobSchedulerError::GetJobData);
                    }
                    let row = row.unwrap();
                    Ok(Some(row.into()))
                }
            }
        })
    }

    fn add_or_update(
        &mut self,
        data: JobStoredData,
    ) -> Pin<Box<dyn Future<Output = Result<(), JobSchedulerError>> + Send>> {
        let store = self.store.clone();
        let table = self.table.clone();
        Box::pin(async move {
            use crate::job::job_data::job_stored_data::Job::CronJob as CronJobType;
            use crate::job::job_data::job_stored_data::Job::NonCronJob as NonCronJobType;

            let store = store.read().await;
            match &*store {
                PostgresStore::Created(_) => Err(JobSchedulerError::UpdateJobData),
                PostgresStore::Inited(store) => {
                    let uuid: Uuid = data.id.as_ref().unwrap().into();
                    let store = store.read().await;
                    let sql = "INSERT INTO $1 (\
                        id, last_updated, next_tick, job_type, count, \
                        ran, stopped, schedule, repeating, repeated_every, \
                        extra \
                    )\
                    VALUES (\
                        $2, $3, $4, $5,  $6, \
                        $7, $8, $9, $10, $11\
                        $12 \
                    )\
                    ON CONFLICT (id) \
                    DO \
                        UPDATE $1 \
                        SET \
                            last_updated=$3, next_tick=$4, job_type=$5, count=$6, \
                            ran=$7, stopped=$8, schedule=$9, repeating=$10, repeated_every=$11, \
                            extra=$12 \
                        WHERE \
                            id=$2
                    ";
                    let last_updated = data.last_updated.as_ref().map(|i| *i as i64);
                    let next_tick = data.next_tick as i64;
                    let job_type = data.job_type;
                    let count = data.count as i32;
                    let ran = data.ran;
                    let stopped = data.stopped;
                    let schedule = match data.job.as_ref() {
                        Some(CronJobType(ct)) => Some(ct.schedule.clone()),
                        _ => None,
                    };
                    let repeating = match data.job.as_ref() {
                        Some(NonCronJobType(ct)) => Some(ct.repeating),
                        _ => None,
                    };
                    let repeated_every = match data.job.as_ref() {
                        Some(NonCronJobType(ct)) => Some(ct.repeated_every as i64),
                        _ => None,
                    };
                    let extra = data.extra;

                    let val = store
                        .query_one(
                            sql,
                            &[
                                &table,
                                &uuid,
                                &last_updated,
                                &next_tick,
                                &job_type,
                                &count,
                                &ran,
                                &stopped,
                                &schedule,
                                &repeating,
                                &repeated_every,
                                &extra,
                            ],
                        )
                        .await;
                    if let Err(e) = val {
                        error!("Error {:?}", e);
                        Err(JobSchedulerError::CantAdd)
                    } else {
                        Ok(())
                    }
                }
            }
        })
    }

    fn delete(
        &mut self,
        guid: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<(), JobSchedulerError>> + Send>> {
        let store = self.store.clone();
        let table = self.table.clone();

        Box::pin(async move {
            let store = store.read().await;
            match &*store {
                PostgresStore::Created(_) => Err(JobSchedulerError::CantRemove),
                PostgresStore::Inited(store) => {
                    let store = store.read().await;
                    let val = store
                        .query("delete from $1 where id = $2", &[&table, &guid])
                        .await;
                    match val {
                        Ok(_) => Ok(()),
                        Err(e) => {
                            error!("Error deleting job data {:?}", e);
                            Err(JobSchedulerError::CantRemove)
                        }
                    }
                }
            }
        })
    }
}

impl From<Row> for JobStoredData {
    fn from(row: Row) -> Self {
        let id: Uuid = row.get(0);
        let last_updated = row.try_get(1).ok().map(|i: i64| i as u64);
        let last_tick = row.try_get(2).ok().map(|i: i64| i as u64);
        let next_tick = row
            .try_get(3)
            .ok()
            .map(|i: i64| i as u64)
            .unwrap_or_default();
        let job_type: i32 = row.try_get(4).unwrap_or_default();
        let count = row.try_get(5).unwrap_or_default();
        let extra = row.try_get(6).unwrap_or_default();
        let ran = row.try_get(7).unwrap_or_default();
        let stopped = row.try_get(8).unwrap_or_default();
        let job = {
            use crate::job::job_data::job_stored_data::Job::CronJob as CronJobType;
            use crate::job::job_data::job_stored_data::Job::NonCronJob as NonCronJobType;

            let job_type = JobType::from_i32(job_type);
            match job_type {
                Some(JobType::Cron) => match row.try_get(8) {
                    Ok(schedule) => Some(CronJobType(CronJob { schedule })),
                    _ => None,
                },
                Some(_) => {
                    let repeating = row.get(9);
                    let repeated_every = row
                        .try_get(10)
                        .ok()
                        .map(|i: i64| i as u64)
                        .unwrap_or_default();
                    Some(NonCronJobType(NonCronJob {
                        repeating,
                        repeated_every,
                    }))
                }
                None => None,
            }
        };
        Self {
            id: Some(id.into()),
            last_updated,
            last_tick,
            next_tick,
            job_type,
            count,
            extra,
            ran,
            stopped,
            job,
        }
    }
}

impl InitStore for PostgresMetadataStore {
    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<(), JobSchedulerError>> + Send>> {
        let inited = self.inited();
        let store = self.store.clone();
        let init_tables = self.init_tables;
        let table = self.table.clone();
        Box::pin(async move {
            let inited = inited.await;
            if matches!(inited, Ok(false)) || matches!(inited, Err(_)) {
                let mut w = store.write().await;
                let val = w.clone();
                let val = val.init().await;
                match val {
                    Ok(v) => {
                        if init_tables {
                            if let PostgresStore::Inited(client) = &v {
                                let v = client.read().await;
                                let create = v
                                    .query(
                                        "CREATE TABLE IF NOT EXISTS $1 (\
                                            id UUID constraint pk_metadata PRIMARY KEY,\
                                            last_updated BIGINT,\
                                            next_tick BIGINT,\
                                            job_type INTEGER NOT NULL,\
                                            count INTEGER,\
                                            ran BOOL,\
                                            stopped BOOL,\
                                            schedule TEXT,\
                                            repeating BOOL,\
                                            repeated_every BIGINT,\
                                            extra BYTEA
                                        )",
                                        &[&table],
                                    )
                                    .await;
                                if let Err(e) = create {
                                    error!("Error {:?}", e);
                                    return Err(JobSchedulerError::CantInit);
                                }
                            }
                        }
                        *w = v;
                        Ok(())
                    }
                    Err(e) => {
                        error!("Error initialising {:?}", e);
                        Err(e)
                    }
                }
            } else {
                Ok(())
            }
        })
    }

    fn inited(&mut self) -> Pin<Box<dyn Future<Output = Result<bool, JobSchedulerError>> + Send>> {
        let store = self.store.clone();
        Box::pin(async move {
            let store = store.read().await;
            Ok(matches!(*store, PostgresStore::Inited(_)))
        })
    }
}

impl MetaDataStorage for PostgresMetadataStore {
    fn list_next_ticks(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<JobAndNextTick>, JobSchedulerError>> + Send>> {
        let store = self.store.clone();
        let table = self.table.clone();

        Box::pin(async move {
            let store = store.read().await;
            match &*store {
                PostgresStore::Created(_) => Err(JobSchedulerError::CantListNextTicks),
                PostgresStore::Inited(store) => {
                    let store = store.read().await;
                    let now = Utc::now().timestamp();
                    let sql = "SELECT \
                            id, job_type, next_tick, last_tick \
                        FROM $1 \
                        WHERE next_tick > 0 && next_tick < $2";
                    let rows = store.query(sql, &[&table, &now]).await;
                    match rows {
                        Ok(rows) => Ok(rows
                            .iter()
                            .map(|row| {
                                let id: Uuid = row.get(0);
                                let id: JobUuid = id.into();
                                let job_type = row.get(1);
                                let next_tick = row
                                    .try_get(3)
                                    .ok()
                                    .map(|i: i64| i as u64)
                                    .unwrap_or_default();
                                let last_tick = row.try_get(4).ok().map(|i: i64| i as u64);

                                JobAndNextTick {
                                    id: Some(id),
                                    job_type,
                                    next_tick,
                                    last_tick,
                                }
                            })
                            .collect::<Vec<_>>()),
                        Err(e) => {
                            error!("Error getting next ticks {:?}", e);
                            Err(JobSchedulerError::CantListNextTicks)
                        }
                    }
                }
            }
        })
    }

    fn set_next_and_last_tick(
        &mut self,
        guid: Uuid,
        next_tick: Option<DateTime<Utc>>,
        last_tick: Option<DateTime<Utc>>,
    ) -> Pin<Box<dyn Future<Output = Result<(), JobSchedulerError>> + Send>> {
        let store = self.store.clone();
        let table = self.table.clone();

        Box::pin(async move {
            let store = store.read().await;
            match &*store {
                PostgresStore::Created(_) => Err(JobSchedulerError::UpdateJobData),
                PostgresStore::Inited(store) => {
                    let store = store.read().await;
                    let next_tick = next_tick.map(|b| b.timestamp()).unwrap_or(0);
                    let last_tick = last_tick.map(|b| b.timestamp());
                    let sql = "UPDATE $1 \
                        SET \
                         next_tick=$2, last_tick=$3 \
                        WHERE \
                            id = $4";
                    let resp = store
                        .query(sql, &[&table, &next_tick, &last_tick, &guid])
                        .await;
                    if let Err(e) = resp {
                        error!("Error updating next and last tick {:?}", e);
                        Err(JobSchedulerError::UpdateJobData)
                    } else {
                        Ok(())
                    }
                }
            }
        })
    }

    fn time_till_next_job(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Duration>, JobSchedulerError>> + Send>> {
        let store = self.store.clone();
        let table = self.table.clone();
        Box::pin(async move {
            let store = store.read().await;
            match &*store {
                PostgresStore::Created(_) => Err(JobSchedulerError::CouldNotGetTimeUntilNextTick),
                PostgresStore::Inited(store) => {
                    let store = store.read().await;
                    let now = Utc::now().timestamp();
                    let sql = "SELECT \
                            next_tick \
                        FROM $1 \
                        WHERE next_tick > 0 && next_tick > $2 \
                        ORDER BY next_tick ASC \
                        LIMIT 1";
                    let row = store.query(sql, &[&table, &now]).await;
                    if let Err(e) = row {
                        error!("Error getting time until next job {:?}", e);
                        return Err(JobSchedulerError::CouldNotGetTimeUntilNextTick);
                    }
                    let row = row.unwrap();
                    Ok(row
                        .get(0)
                        .map(|r| r.get::<_, i64>(0))
                        .map(|ts| ts - now)
                        .filter(|ts| *ts > 0)
                        .map(|ts| ts as u64)
                        .map(std::time::Duration::from_secs))
                }
            }
        })
    }
}