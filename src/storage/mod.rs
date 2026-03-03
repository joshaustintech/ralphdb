use dashmap::DashMap;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Entry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

impl Entry {
    fn new(value: Vec<u8>) -> Self {
        Self {
            value,
            expires_at: None,
        }
    }

    fn expired(&self) -> bool {
        self.expires_at
            .map(|expiry| Instant::now() >= expiry)
            .unwrap_or(false)
    }
}

#[derive(Debug)]
pub enum StorageError {
    InvalidInteger,
    IntegerOutOfRange,
}

pub struct Storage {
    inner: DashMap<Vec<u8>, Entry>,
}

impl Storage {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: DashMap::new(),
        })
    }

    pub fn set(&self, key: Vec<u8>, value: Vec<u8>) {
        self.inner.insert(key, Entry::new(value));
    }

    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        if let Some(entry) = self.inner.get(key) {
            if entry.expired() {
                drop(entry);
                self.inner.remove(key);
                return None;
            }
            return Some(entry.value.clone());
        }

        None
    }

    pub fn del(&self, keys: &[Vec<u8>]) -> usize {
        let mut count = 0;
        for key in keys {
            if let Some(entry) = self.inner.get(key)
                && entry.expired()
            {
                drop(entry);
                self.inner.remove(key);
                continue;
            }

            if self.inner.remove(key).is_some() {
                count += 1;
            }
        }
        count
    }

    pub fn exists(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }

    pub fn expire(&self, key: &[u8], duration: Duration) -> bool {
        let Some(expiry) = Instant::now().checked_add(duration) else {
            return false;
        };
        if let Some(mut entry) = self.inner.get_mut(key) {
            if entry.expired() {
                drop(entry);
                self.inner.remove(key);
                return false;
            }

            entry.expires_at = Some(expiry);
            true
        } else {
            false
        }
    }

    pub fn ttl(&self, key: &[u8]) -> i64 {
        if let Some(entry) = self.inner.get(key) {
            if entry.expired() {
                drop(entry);
                self.inner.remove(key);
                return -2;
            }
            if let Some(expiry) = entry.expires_at {
                let remaining = expiry.saturating_duration_since(Instant::now());
                remaining.as_secs() as i64
            } else {
                -1
            }
        } else {
            -2
        }
    }

    pub fn incr(&self, key: &[u8]) -> Result<i64, StorageError> {
        self.incr_by(key, 1)
    }

    pub fn decr(&self, key: &[u8]) -> Result<i64, StorageError> {
        self.incr_by(key, -1)
    }

    pub fn incr_by(&self, key: &[u8], delta: i64) -> Result<i64, StorageError> {
        let key_vec = key.to_vec();
        let mut entry = self
            .inner
            .entry(key_vec.clone())
            .or_insert_with(|| Entry::new(b"0".to_vec()));
        if entry.expired() {
            entry.value = b"0".to_vec();
            entry.expires_at = None;
        }
        let current =
            std::str::from_utf8(&entry.value).map_err(|_| StorageError::InvalidInteger)?;
        let current = current
            .parse::<i64>()
            .map_err(|_| StorageError::InvalidInteger)?;
        let updated = current
            .checked_add(delta)
            .ok_or(StorageError::IntegerOutOfRange)?;
        entry.value = updated.to_string().into_bytes();
        Ok(updated)
    }

    pub fn mget(&self, keys: &[Vec<u8>]) -> Vec<Option<Vec<u8>>> {
        keys.iter().map(|key| self.get(key)).collect()
    }

    pub fn mset(&self, pairs: &[(Vec<u8>, Vec<u8>)]) {
        for (key, value) in pairs {
            self.set(key.clone(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        thread::sleep,
        time::{Duration, Instant},
    };

    fn wait_for_removal(storage: &Storage, key: &[u8]) {
        let timeout = Duration::from_secs(1);
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if storage.get(key).is_none() {
                return;
            }
            sleep(Duration::from_millis(5));
        }
        panic!(
            "key {:?} did not expire within {:?}",
            String::from_utf8_lossy(key),
            timeout
        );
    }

    #[test]
    fn set_get_round_trip() {
        let storage = Storage::new();
        storage.set(b"foo".to_vec(), b"bar".to_vec());
        assert_eq!(storage.get(b"foo"), Some(b"bar".to_vec()));
    }

    #[test]
    fn ttl_and_expire() {
        let storage = Storage::new();
        storage.set(b"key".to_vec(), b"value".to_vec());
        assert!(storage.expire(b"key", Duration::from_millis(10)));
        assert!(storage.ttl(b"key") > -1);
        wait_for_removal(&storage, b"key");
        assert_eq!(storage.get(b"key"), None);
        assert_eq!(storage.ttl(b"key"), -2);
    }

    #[test]
    fn expire_on_expired_key_returns_false() {
        let storage = Storage::new();
        storage.set(b"temp".to_vec(), b"value".to_vec());
        assert!(storage.expire(b"temp", Duration::from_millis(5)));
        wait_for_removal(&storage, b"temp");
        assert!(!storage.expire(b"temp", Duration::from_secs(1)));
        assert_eq!(storage.get(b"temp"), None);
    }

    #[test]
    fn expire_with_unrepresentable_deadline_returns_false() {
        let storage = Storage::new();
        storage.set(b"temp".to_vec(), b"value".to_vec());
        assert!(!storage.expire(b"temp", Duration::MAX));
        assert_eq!(storage.get(b"temp"), Some(b"value".to_vec()));
    }

    #[test]
    fn del_on_expired_key_returns_zero() {
        let storage = Storage::new();
        storage.set(b"temp".to_vec(), b"value".to_vec());
        assert!(storage.expire(b"temp", Duration::from_millis(5)));
        sleep(Duration::from_millis(30));

        assert_eq!(storage.del(&[b"temp".to_vec()]), 0);
    }

    #[test]
    fn incr_decr_behaviour() {
        let storage = Storage::new();
        storage.set(b"counter".to_vec(), b"5".to_vec());
        assert_eq!(storage.incr(b"counter").unwrap(), 6);
        assert_eq!(storage.decr(b"counter").unwrap(), 5);
    }

    #[test]
    fn incr_overflow_returns_error() {
        let storage = Storage::new();
        storage.set(b"counter".to_vec(), i64::MAX.to_string().into_bytes());
        assert!(matches!(
            storage.incr(b"counter"),
            Err(StorageError::IntegerOutOfRange)
        ));
    }
}
