use std::collections::{BTreeMap, HashMap};

pub fn convert_hashmap_to_btreemap<K, V>(hash_map: HashMap<K, V>) -> BTreeMap<K, V>
where
    K: Ord + Eq + std::hash::Hash,
    V: Clone,
{
    let mut btree_map: BTreeMap<K, V> = BTreeMap::new();
    for (key, value) in hash_map {
        btree_map.insert(key, value);
    }
    btree_map
}

pub fn convert_btreemap<'a>(original: &'a BTreeMap<String, String>) -> BTreeMap<&'a str, &'a str> {
    let mut new_map: BTreeMap<&str, &str> = BTreeMap::new();
    for (key, value) in original.iter() {
        new_map.insert(key.as_str(), value.as_str());
    }
    new_map
}
