//! The storage module handles both the current state in-memory and the stored
//! state in DB.

mod rocksdb;

use std::fmt;

use arse_merkle_tree::blake2b::Blake2bHasher;
use arse_merkle_tree::traits::Hasher;
use arse_merkle_tree::H256;
use blake2b_rs::{Blake2b, Blake2bBuilder};
use namada::ledger::storage::{Storage, StorageHasher};

#[derive(Default)]
pub struct PersistentStorageHasher(Blake2bHasher);

pub type PersistentDB = rocksdb::RocksDB;

pub type PersistentStorage = Storage<PersistentDB, PersistentStorageHasher>;

impl Hasher for PersistentStorageHasher {
    fn write_bytes(&mut self, h: &[u8]) {
        self.0.write_bytes(h)
    }

    fn finish(self) -> H256 {
        self.0.finish()
    }
}

impl StorageHasher for PersistentStorageHasher {
    fn hash(value: impl AsRef<[u8]>) -> H256 {
        let mut buf = [0u8; 32];
        let mut hasher = new_blake2b();
        hasher.update(value.as_ref());
        hasher.finalize(&mut buf);
        buf.into()
    }
}

impl fmt::Debug for PersistentStorageHasher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PersistentStorageHasher")
    }
}

fn new_blake2b() -> Blake2b {
    Blake2bBuilder::new(32).personal(b"namada storage").build()
}

#[cfg(test)]
mod tests {
    use namada::ledger::storage::types;
    use namada::types::address;
    use namada::types::chain::ChainId;
    use namada::types::storage::{BlockHash, BlockHeight, Key};
    use proptest::collection::vec;
    use proptest::prelude::*;
    use proptest::test_runner::Config;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_crud_value() {
        let db_path =
            TempDir::new().expect("Unable to create a temporary DB directory");
        let mut storage = PersistentStorage::open(
            db_path.path(),
            ChainId::default(),
            address::nam(),
            None,
        );
        let key = Key::parse("key").expect("cannot parse the key string");
        let value: u64 = 1;
        let value_bytes = types::encode(&value);
        let value_bytes_len = value_bytes.len();

        // before insertion
        let (result, gas) = storage.has_key(&key).expect("has_key failed");
        assert!(!result);
        assert_eq!(gas, key.len() as u64);
        let (result, gas) = storage.read(&key).expect("read failed");
        assert_eq!(result, None);
        assert_eq!(gas, key.len() as u64);

        // insert
        storage.write(&key, value_bytes).expect("write failed");

        // read
        let (result, gas) = storage.has_key(&key).expect("has_key failed");
        assert!(result);
        assert_eq!(gas, key.len() as u64);
        let (result, gas) = storage.read(&key).expect("read failed");
        let read_value: u64 =
            types::decode(result.expect("value doesn't exist"))
                .expect("decoding failed");
        assert_eq!(read_value, value);
        assert_eq!(gas, key.len() as u64 + value_bytes_len as u64);

        // delete
        storage.delete(&key).expect("delete failed");

        // read again
        let (result, _) = storage.has_key(&key).expect("has_key failed");
        assert!(!result);
        let (result, _) = storage.read(&key).expect("read failed");
        assert_eq!(result, None);
    }

    #[test]
    fn test_commit_block() {
        let db_path =
            TempDir::new().expect("Unable to create a temporary DB directory");
        let mut storage = PersistentStorage::open(
            db_path.path(),
            ChainId::default(),
            address::nam(),
            None,
        );
        storage
            .begin_block(BlockHash::default(), BlockHeight(100))
            .expect("begin_block failed");
        let key = Key::parse("key").expect("cannot parse the key string");
        let value: u64 = 1;
        let value_bytes = types::encode(&value);

        // insert and commit
        storage
            .write(&key, value_bytes.clone())
            .expect("write failed");
        storage.commit().expect("commit failed");

        // save the last state and drop the storage
        let root = storage.merkle_root().0;
        let hash = storage.get_block_hash().0;
        let address_gen = storage.address_gen.clone();
        drop(storage);

        // load the last state
        let mut storage = PersistentStorage::open(
            db_path.path(),
            ChainId::default(),
            address::nam(),
            None,
        );
        storage
            .load_last_state()
            .expect("loading the last state failed");
        let (loaded_root, height) =
            storage.get_state().expect("no block exists");
        assert_eq!(loaded_root.0, root);
        assert_eq!(height, 100);
        assert_eq!(storage.get_block_hash().0, hash);
        assert_eq!(storage.address_gen, address_gen);
        let (val, _) = storage.read(&key).expect("read failed");
        assert_eq!(val.expect("no value"), value_bytes);
    }

    #[test]
    fn test_iter() {
        let db_path =
            TempDir::new().expect("Unable to create a temporary DB directory");
        let mut storage = PersistentStorage::open(
            db_path.path(),
            ChainId::default(),
            address::nam(),
            None,
        );
        storage
            .begin_block(BlockHash::default(), BlockHeight(100))
            .expect("begin_block failed");

        let mut expected = Vec::new();
        let prefix = Key::parse("prefix").expect("cannot parse the key string");
        for i in (0..9).rev() {
            let key = prefix
                .push(&format!("{}", i))
                .expect("cannot push the key segment");
            let value_bytes = types::encode(&(i as u64));
            // insert
            storage
                .write(&key, value_bytes.clone())
                .expect("write failed");
            expected.push((key.to_string(), value_bytes));
        }
        storage.commit().expect("commit failed");

        let (iter, gas) = storage.iter_prefix(&prefix);
        assert_eq!(gas, prefix.len() as u64);
        for (k, v, gas) in iter {
            match expected.pop() {
                Some((expected_key, expected_val)) => {
                    assert_eq!(k, expected_key);
                    assert_eq!(v, expected_val);
                    let expected_gas = expected_key.len() + expected_val.len();
                    assert_eq!(gas, expected_gas as u64);
                }
                None => panic!("read a pair though no expected pair"),
            }
        }
    }

    #[test]
    fn test_validity_predicate() {
        let db_path =
            TempDir::new().expect("Unable to create a temporary DB directory");
        let mut storage = PersistentStorage::open(
            db_path.path(),
            ChainId::default(),
            address::nam(),
            None,
        );
        storage
            .begin_block(BlockHash::default(), BlockHeight(100))
            .expect("begin_block failed");

        let addr = storage.address_gen.generate_address("test".as_bytes());
        let key = Key::validity_predicate(&addr);

        // not exist
        let (vp, gas) =
            storage.validity_predicate(&addr).expect("VP load failed");
        assert_eq!(vp, None);
        assert_eq!(gas, key.len() as u64);

        // insert
        let vp1 = "vp1".as_bytes().to_vec();
        storage.write(&key, vp1.clone()).expect("write failed");

        // check
        let (vp, gas) =
            storage.validity_predicate(&addr).expect("VP load failed");
        assert_eq!(vp.expect("no VP"), vp1);
        assert_eq!(gas, (key.len() + vp1.len()) as u64);
    }

    proptest! {
        #![proptest_config(Config {
            cases: 5,
            .. Config::default()
        })]
        #[test]
        fn test_read_with_height(blocks_write_value in vec(any::<bool>(), 20)) {
            test_read_with_height_aux(blocks_write_value).unwrap()
        }
    }

    /// Test reads at arbitrary block heights.
    ///
    /// We generate `blocks_write_value` with random bools as the input to this
    /// function, then:
    ///
    /// 1. For each `blocks_write_value`, write the current block height if true
    ///    or delete otherwise.
    /// 2. We try to read from these heights to check that we get back expected
    ///    value if was written at that block height or `None` if it was
    ///    deleted.
    /// 3. We try to read past the last height and we expect the last written
    ///    value, if any.
    fn test_read_with_height_aux(
        blocks_write_value: Vec<bool>,
    ) -> namada::ledger::storage::Result<()> {
        let db_path =
            TempDir::new().expect("Unable to create a temporary DB directory");
        let mut storage = PersistentStorage::open(
            db_path.path(),
            ChainId::default(),
            address::nam(),
            None,
        );

        // 1. For each `blocks_write_value`, write the current block height if
        // true or delete otherwise.
        // We `.enumerate()` height (starting from `0`)
        let blocks_write_value = blocks_write_value
            .into_iter()
            .enumerate()
            .map(|(height, write_value)| {
                println!(
                    "At height {height} will {}",
                    if write_value { "write" } else { "delete" }
                );
                (BlockHeight::from(height as u64), write_value)
            });

        let key = Key::parse("key").expect("cannot parse the key string");
        for (height, write_value) in blocks_write_value.clone() {
            let hash = BlockHash::default();
            storage.begin_block(hash, height)?;
            assert_eq!(
                height, storage.block.height,
                "sanity check - height is as expected"
            );

            if write_value {
                let value_bytes = types::encode(&storage.block.height);
                storage.write(&key, value_bytes)?;
            } else {
                storage.delete(&key)?;
            }
            storage.commit()?;
        }

        // 2. We try to read from these heights to check that we get back
        // expected value if was written at that block height or
        // `None` if it was deleted.
        for (height, write_value) in blocks_write_value.clone() {
            let (value_bytes, _gas) = storage.read_with_height(&key, height)?;
            if write_value {
                let value_bytes = value_bytes.unwrap_or_else(|| {
                    panic!("Couldn't read from height {height}")
                });
                let value: BlockHeight = types::decode(value_bytes).unwrap();
                assert_eq!(value, height);
            } else if value_bytes.is_some() {
                let value: BlockHeight =
                    types::decode(value_bytes.unwrap()).unwrap();
                panic!("Expected no value at height {height}, got {}", value,);
            }
        }

        // 3. We try to read past the last height and we expect the last written
        // value, if any.

        // If height is >= storage.last_height, it should read the latest state.
        let is_last_write = blocks_write_value.last().unwrap().1;

        // The upper bound is arbitrary.
        for height in storage.last_height.0..storage.last_height.0 + 10 {
            let height = BlockHeight::from(height);
            let (value_bytes, _gas) = storage.read_with_height(&key, height)?;
            if is_last_write {
                let value_bytes =
                    value_bytes.expect("Should have been written");
                let value: BlockHeight = types::decode(value_bytes).unwrap();
                assert_eq!(value, storage.last_height);
            } else if value_bytes.is_some() {
                let value: BlockHeight =
                    types::decode(value_bytes.unwrap()).unwrap();
                panic!("Expected no value at height {height}, got {}", value,);
            }
        }

        Ok(())
    }
}
