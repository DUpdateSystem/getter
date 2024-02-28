use std::sync::{Mutex, MutexGuard};

pub struct InstanceContainer<T> {
    instance: Mutex<Option<T>>,
}

pub struct InstanceGuard<'a, T>(MutexGuard<'a, Option<T>>);

impl<'a, T> InstanceGuard<'a, T> {
    pub fn get(&mut self) -> Option<&mut T> {
        self.0.as_mut()
    }
}

impl<T> InstanceContainer<T> {
    pub fn new() -> Self {
        Self {
            instance: Mutex::new(None),
        }
    }

    pub fn is_init(&self) -> bool {
        self.instance.lock().unwrap().is_some()
    }

    pub fn init(&self, instance: T) {
        let mut instance_guard = self.instance.lock().unwrap();
        *instance_guard = Some(instance);
    }

    pub fn get(&self) -> InstanceGuard<T> {
        InstanceGuard(self.instance.lock().unwrap())
    }

    pub fn get_or_init(&self, init: impl FnOnce() -> T) -> InstanceGuard<T> {
        let mut instance_guard = self.instance.lock().unwrap();
        if instance_guard.is_none() {
            *instance_guard = Some(init());
        }
        InstanceGuard(instance_guard)
    }
}
