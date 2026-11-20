let mut map = HashMap::new();
*map.entry(key).or_insert(0) += 1;
