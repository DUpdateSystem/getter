use std::sync::Arc;
use tokio::sync::Mutex;

pub struct InstanceContainer<T> {
    instance: Arc<Mutex<T>>,
}

impl<T> InstanceContainer<T> {
    pub fn new(value: T) -> Self {
        Self {
            instance: Arc::new(Mutex::new(value)),
        }
    }

    pub async fn get(&self) -> Arc<Mutex<T>> {
        self.instance.clone()
    }
}