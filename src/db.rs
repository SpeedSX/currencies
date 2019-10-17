use std::cmp::Ordering;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use chrono::naive::NaiveDate;
use chrono::Duration;
use anyhow::{anyhow, Error, Context};
use serde::{de::DeserializeOwned, Serialize};
use sled::IVec;
use tokio_executor::blocking;

use crate::currencies::{self, Currency, Date};

pub fn date_as_key(date: &str) -> Result<Vec<u8>, Error> {
    let date = NaiveDate::from_str(date)?
        .and_hms(0, 0, 0)
        .timestamp()
        .to_be_bytes()
        .to_vec();
    Ok(date)
}

#[derive(Clone)]
pub struct Db {
    inner: Arc<sled::Db>,
}

impl Db {
    pub async fn init<P: AsRef<Path>>(path: P) -> Result<Db, Error> {
        if path.as_ref().exists() {
            log::info!("previous db file found, going to open it");
            Db::open(path)
        } else {
            Db::bootstrap_new(path).await
        }
    }

    pub async fn bootstrap_new<P: AsRef<Path>>(path: P) -> Result<Db, Error> {
        log::info!("no database found, going to bootstrap a new one");
        log::info!("dowloading ECB's currency values since 99");
        let dates = crate::currencies::fetch_hist().await.with_context(|| {
            format!("could not fetch Historical reference rates from ECB")
        })?;

        log::info!("populating new db with currency values");
        let current_date = dates
            .first()
            .ok_or_else(|| anyhow!("fetched Historical reference rates from ECB are empy"))?;

        let db = Db::open(path)?;

        db.put(b"current", &date_as_key(&current_date.value)?)
            .await?;
        for mut date in dates {
            let day = &date_as_key(&date.value)?;
            //insert EUR base
            date.currencies.push(Currency {
                name: "EUR".to_string(),
                rate: 1.0,
            });
            db.put(&day, &date).await?;
        }
        db.inner.flush_async().await?;

        Ok(db)
    }

    fn open<P: AsRef<Path>>(path: P) -> Result<Db, Error> {
        let db = Db {
            inner: Arc::new(sled::Db::open(&path)?),
        };
        Ok(db)
    }

    pub async fn get_current_rates(&self) -> Result<Date, Error> {
        let current = self
            .get::<Vec<u8>>(b"current")
            .await?
            .ok_or_else(|| anyhow!("could not find `current` key on the database"))?;

        let date = self.get::<Date>(&current).await?.ok_or_else(|| {
            anyhow!("could not find `current` reference rates on the database")
        })?;

        Ok(date)
    }

    pub async fn get_day_rates(&self, day: &str) -> Result<Option<Date>, Error> {
        match self.get::<Date>(&date_as_key(day)?).await? {
            Some(date) => Ok(Some(date)),
            None => Ok(None),
        }
    }

    pub async fn get_range_rates(
        &self,
        start_at: NaiveDate,
        end_at: NaiveDate,
    ) -> Result<Vec<Date>, Error> {
        let range_start = date_as_key(&start_at.to_string())?;
        let range_end = date_as_key(&end_at.to_string())?;

        let dates = self
            .execute(move |db| {
                db.range(range_start..=range_end)
                    .map(|result| {
                        let (_key, value) = result.unwrap();
                        bincode::deserialize::<Date>(&value)
                    })
                    .collect::<Result<Vec<Date>, _>>()
                    .with_context(|| format!("could not get range from db"))
            })
            .await?;
        Ok(dates)
    }

    async fn put<T>(&self, key: &[u8], value: &T) -> Result<Option<IVec>, Error>
    where
        T: Serialize,
    {
        let key = key.to_vec();
        let encoded: Vec<u8> = bincode::serialize(value)?;
        let db = self.inner.clone();

        blocking::run(move || db.insert(&key, encoded))
            .await
            .map_err(|e| e.into())
    }

    async fn get<T>(&self, key: &[u8]) -> Result<Option<T>, Error>
    where
        T: DeserializeOwned + Send + 'static,
    {
        let key = key.to_vec();
        let opt = self.execute(move |db| db.get(&key)).await?;
        let blob = match opt {
            Some(blob) => blob,
            None => return Ok(None),
        };

        let t = bincode::deserialize::<T>(&blob)?;
        Ok(Some(t))
    }

    pub async fn update(&self) -> Result<(), Error> {
        let current = currencies::fetch_daily().await?.value_as_date()?;
        let db_current = self.get_current_rates().await?.value_as_date()?;

        match current.cmp(&db_current) {
            Ordering::Equal => {
                log::debug!("database currencies up to date");
                return Ok(());
            }

            Ordering::Greater => {
                log::debug!("going to update database with new currencies");
                let mut dates = match current - db_current {
                    d if d > Duration::days(90) => currencies::fetch_hist().await?,
                    d if d < Duration::days(90) && d > Duration::days(1) => {
                        currencies::fetch_last90().await?
                    }
                    _ => vec![currencies::fetch_daily().await?],
                };

                for date in dates.iter_mut().rev() {
                    if date.value_as_date()? > db_current {
                        let day = &date_as_key(&date.value)?;
                        //insert EUR base
                        date.currencies.push(Currency {
                            name: "EUR".to_string(),
                            rate: 1.0,
                        });
                        self.put(&day, &date).await?;
                        self.put(b"current", &date_as_key(&date.value)?).await?;
                        log::info!("inserted rates for {}", date.value.to_string());
                    }
                }
            }
            Ordering::Less => {
                return Err(anyhow!(
                    "error, current database rates are younger than fetched from ECB"
                ))
            }
        }
        Ok(())
    }

    async fn execute<F, T>(&self, f: F) -> T
    where
        F: FnOnce(Arc<sled::Db>) -> T + Send + 'static,
        T: Send + 'static,
    {
        let db = self.inner.clone();
        tokio_executor::blocking::run(|| f(db)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_date_as_key() {
        let key = date_as_key("1999-01-04").unwrap();
        assert_eq!(key, vec![0, 0, 0, 0, 54, 144, 4, 128]);
    }

    #[tokio::test]
    async fn test_put_get() {
        let dir = tempdir().unwrap();
        let path = dir.into_path();
        let db = Db::open(path.join("db")).unwrap();
        let date = Date {
            value: "1999-01-04".to_string(),
            currencies: Vec::new(),
        };
        let key = date_as_key(&date.value).unwrap();
        db.put(&key, &date).await.unwrap();
        db.inner.flush_async().await.unwrap();
        let date2 = db.get(&key).await.unwrap().unwrap();
        assert_eq!(date, date2);
    }

    #[tokio::test]
    async fn test_get_current_rates() {
        let dir = tempdir().unwrap();
        let path = dir.into_path();
        let db = Db::open(path.join("db")).unwrap();
        let date = Date {
            value: "1999-01-04".to_string(),
            currencies: Vec::new(),
        };
        let key = date_as_key(&date.value).unwrap();
        db.put(b"current", &key).await.unwrap();
        db.put(&key, &date).await.unwrap();
        db.inner.flush_async().await.unwrap();
        let current = db.get_current_rates().await.unwrap();
        assert_eq!(date, current);
    }

    #[tokio::test]
    async fn test_get_day_rates() {
        let dir = tempdir().unwrap();
        let path = dir.into_path();
        let db = Db::open(path.join("db")).unwrap();
        let date = Date {
            value: "1999-01-04".to_string(),
            currencies: Vec::new(),
        };
        let key = date_as_key(&date.value).unwrap();
        db.put(&key, &date).await.unwrap();
        db.inner.flush_async().await.unwrap();
        let current = db.get_day_rates("1999-01-04").await.unwrap().unwrap();
        assert_eq!(date, current);
    }

    #[tokio::test]
    async fn test_get_range_rates() {
        let dir = tempdir().unwrap();
        let path = dir.into_path();
        let db = Db::open(path.join("db")).unwrap();

        let date = Date {
            value: "1999-01-04".to_string(),
            currencies: Vec::new(),
        };
        let key = date_as_key(&date.value).unwrap();
        db.put(&key, &date).await.unwrap();

        let date2 = Date {
            value: "2003-01-04".to_string(),
            currencies: Vec::new(),
        };
        let key2 = date_as_key(&date2.value).unwrap();
        db.put(&key2, &date2).await.unwrap();

        let date3 = Date {
            value: "2012-01-04".to_string(),
            currencies: Vec::new(),
        };
        let key3 = date_as_key(&date3.value).unwrap();
        db.put(&key3, &date3).await.unwrap();
        db.inner.flush_async().await.unwrap();

        let begining = NaiveDate::from_str("1999-01-04").unwrap();
        let end = NaiveDate::from_str("2012-01-04").unwrap();
        let dates = db.get_range_rates(begining, end).await.unwrap();
        assert_eq!(dates.len(), 3);
    }
}
