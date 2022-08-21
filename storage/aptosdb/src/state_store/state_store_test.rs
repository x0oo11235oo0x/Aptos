// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use proptest::{collection::hash_map, prelude::*};

use aptos_jellyfish_merkle::{restore::StateSnapshotRestore, TreeReader};
use aptos_temppath::TempPath;
use aptos_types::{
    access_path::AccessPath, account_address::AccountAddress, state_store::state_key::StateKeyTag,
};
use storage_interface::{jmt_update_refs, jmt_updates, DbReader, DbWriter, StateSnapshotReceiver};

use crate::{
    pruner::state_store::StateMerklePruner,
    test_helper::{arb_state_kv_sets, update_store},
    AptosDB,
};

use super::*;

fn put_value_set(
    state_store: &StateStore,
    value_set: Vec<(StateKey, StateValue)>,
    version: Version,
    base_version: Option<Version>,
) -> HashValue {
    let value_set: HashMap<_, _> = value_set
        .iter()
        .map(|(key, value)| (key.clone(), Some(value.clone())))
        .collect();
    let jmt_updates = jmt_updates(&value_set);

    let root = state_store
        .merklize_value_set(jmt_update_refs(&jmt_updates), None, version, base_version)
        .unwrap();
    let mut batch = SchemaBatch::new();
    state_store
        .put_value_sets(
            vec![&value_set],
            version,
            StateStorageUsage::new_untracked(),
            &mut batch,
        )
        .unwrap();
    state_store.ledger_db.write_schemas(batch).unwrap();
    root
}

fn prune_stale_indices(
    state_pruner: &StateMerklePruner,
    min_readable_version: Version,
    target_min_readable_version: Version,
    limit: usize,
) -> Version {
    state_pruner
        .prune_state_merkle(
            min_readable_version,
            target_min_readable_version,
            limit,
            None,
        )
        .unwrap()
}

fn verify_value_and_proof(
    store: &StateStore,
    key: StateKey,
    expected_value: Option<&StateValue>,
    version: Version,
    root: HashValue,
) {
    verify_value_and_proof_in_store(store, key.clone(), expected_value, version, root);
    verify_value_index_in_store(store, key, expected_value, version);
}

fn verify_value_and_proof_in_store(
    store: &StateStore,
    key: StateKey,
    expected_value: Option<&StateValue>,
    version: Version,
    root: HashValue,
) {
    let (value, proof) = store
        .get_state_value_with_proof_by_version(&key, version)
        .unwrap();
    assert_eq!(value.as_ref(), expected_value);
    proof.verify(root, key.hash(), value.as_ref()).unwrap();
}

fn verify_value_index_in_store(
    store: &StateStore,
    key: StateKey,
    expected_value: Option<&StateValue>,
    version: Version,
) {
    let value = store.get_state_value_by_version(&key, version).unwrap();
    assert_eq!(value.as_ref(), expected_value);
}

#[test]
fn test_empty_store() {
    let tmp_dir = TempPath::new();
    let db = AptosDB::new_for_test(&tmp_dir);
    let store = &db.state_store;
    let key = StateKey::Raw(String::from("test_key").into_bytes());
    assert!(store
        .get_state_value_with_proof_by_version(&key, 0)
        .is_err());
}

#[test]
fn test_state_store_reader_writer() {
    let tmp_dir = TempPath::new();
    let db = AptosDB::new_for_test(&tmp_dir);
    let store = &db.state_store;
    let key1 = StateKey::Raw(String::from("test_key1").into_bytes());
    let key2 = StateKey::Raw(String::from("test_key2").into_bytes());
    let key3 = StateKey::Raw(String::from("test_key3").into_bytes());

    let value1 = StateValue::from(String::from("test_val1").into_bytes());
    let value1_update = StateValue::from(String::from("test_val1_update").into_bytes());
    let value2 = StateValue::from(String::from("test_val2").into_bytes());
    let value3 = StateValue::from(String::from("test_val3").into_bytes());

    // Insert address1 with value 1 and verify new states.
    let mut root = put_value_set(
        store,
        vec![(key1.clone(), value1.clone())],
        0, /* version */
        None,
    );
    verify_value_and_proof(store, key1.clone(), Some(&value1), 0, root);

    verify_value_and_proof(store, key2.clone(), None, 0, root);
    verify_value_and_proof(store, key3.clone(), None, 0, root);

    // Insert address 1 with updated value1, address2 with value 2 and address3 with value3 and
    // verify new states.

    root = put_value_set(
        store,
        vec![
            (key1.clone(), value1_update.clone()),
            (key2.clone(), value2.clone()),
            (key3.clone(), value3.clone()),
        ],
        1, /* version */
        Some(0),
    );

    verify_value_and_proof(store, key1, Some(&value1_update), 1, root);
    verify_value_and_proof(store, key2, Some(&value2), 1, root);
    verify_value_and_proof(store, key3, Some(&value3), 1, root);
}

#[test]
fn test_get_values_by_key_prefix() {
    let tmp_dir = TempPath::new();
    let db = AptosDB::new_for_test(&tmp_dir);
    let store = &db.state_store;
    let address = AccountAddress::new([12u8; AccountAddress::LENGTH]);

    let key1 = StateKey::AccessPath(AccessPath::new(address, b"state_key1".to_vec()));
    let key2 = StateKey::AccessPath(AccessPath::new(address, b"state_key2".to_vec()));

    let value1_v0 = StateValue::from(String::from("value1_v0").into_bytes());
    let value2_v0 = StateValue::from(String::from("value2_v0").into_bytes());

    let account_key_prefx = StateKeyPrefix::new(StateKeyTag::AccessPath, address.to_vec());

    put_value_set(
        store,
        vec![
            (key1.clone(), value1_v0.clone()),
            (key2.clone(), value2_v0.clone()),
        ],
        0,
        None,
    );

    let key_value_map = store
        .get_values_by_key_prefix(&account_key_prefx, 0)
        .unwrap();
    assert_eq!(key_value_map.len(), 2);
    assert_eq!(*key_value_map.get(&key1).unwrap(), value1_v0);
    assert_eq!(*key_value_map.get(&key2).unwrap(), value2_v0);

    let key4 = StateKey::AccessPath(AccessPath::new(address, b"state_key4".to_vec()));

    let value2_v1 = StateValue::from(String::from("value2_v1").into_bytes());
    let value4_v1 = StateValue::from(String::from("value4_v1").into_bytes());

    put_value_set(
        store,
        vec![
            (key2.clone(), value2_v1.clone()),
            (key4.clone(), value4_v1.clone()),
        ],
        1,
        Some(0),
    );

    // Ensure that we still get only values for key1 and key2 for version 0 after the update
    let key_value_map = store
        .get_values_by_key_prefix(&account_key_prefx, 0)
        .unwrap();
    assert_eq!(key_value_map.len(), 2);
    assert_eq!(*key_value_map.get(&key1).unwrap(), value1_v0);
    assert_eq!(*key_value_map.get(&key2).unwrap(), value2_v0);

    // Ensure that key value map for version 1 returns value for key1 at version 0.
    let key_value_map = store
        .get_values_by_key_prefix(&account_key_prefx, 1)
        .unwrap();
    assert_eq!(key_value_map.len(), 3);
    assert_eq!(*key_value_map.get(&key1).unwrap(), value1_v0);
    assert_eq!(*key_value_map.get(&key2).unwrap(), value2_v1);
    assert_eq!(*key_value_map.get(&key4).unwrap(), value4_v1);

    // Add values for one more account and verify the state
    let address1 = AccountAddress::new([22u8; AccountAddress::LENGTH]);
    let key5 = StateKey::AccessPath(AccessPath::new(address1, b"state_key5".to_vec()));
    let value5_v2 = StateValue::from(String::from("value5_v2").into_bytes());

    let account1_key_prefx = StateKeyPrefix::new(StateKeyTag::AccessPath, address1.to_vec());

    put_value_set(store, vec![(key5.clone(), value5_v2.clone())], 2, Some(1));

    // address1 did not exist in version 0 and 1.
    let key_value_map = store
        .get_values_by_key_prefix(&account1_key_prefx, 0)
        .unwrap();
    assert_eq!(key_value_map.len(), 0);

    let key_value_map = store
        .get_values_by_key_prefix(&account1_key_prefx, 1)
        .unwrap();
    assert_eq!(key_value_map.len(), 0);

    let key_value_map = store
        .get_values_by_key_prefix(&account1_key_prefx, 2)
        .unwrap();
    assert_eq!(key_value_map.len(), 1);
    assert_eq!(*key_value_map.get(&key5).unwrap(), value5_v2);
}

#[test]
fn test_stale_node_index() {
    let key1 = StateKey::Raw(String::from("test_key1").into_bytes());
    let key2 = StateKey::Raw(String::from("test_key2").into_bytes());
    let key3 = StateKey::Raw(String::from("test_key3").into_bytes());

    let value1 = StateValue::from(String::from("test_val1").into_bytes());
    let value2 = StateValue::from(String::from("test_val2").into_bytes());
    let value2_update = StateValue::from(String::from("test_val2_update").into_bytes());
    let value3 = StateValue::from(String::from("test_val3").into_bytes());
    let value3_update = StateValue::from(String::from("test_val3_update").into_bytes());

    let tmp_dir = TempPath::new();
    let db = AptosDB::new_for_test_no_cache(&tmp_dir);
    let store = &db.state_store;

    // Update.
    // ```text
    // | batch    | 0      | 1             | 2             |
    // | address1 | value1 |               |               |
    // | address2 | value2 | value2_update |               |
    // | address3 |        | value3        | value3_update |
    // ```
    // The stale node indexes will have 4 entries in total.
    // ```
    // index: StaleNodeIndex { stale_since_version: 1, node_key: NodeKey { version: 0, nibble_path:  } }
    // index: StaleNodeIndex { stale_since_version: 1, node_key: NodeKey { version: 0, nibble_path: 2 } }
    // index: StaleNodeIndex { stale_since_version: 2, node_key: NodeKey { version: 1, nibble_path:  } }
    // index: StaleNodeIndex { stale_since_version: 2, node_key: NodeKey { version: 1, nibble_path: d } }
    // ```
    // On version 1, there are two entries, one changes address2 and the other changes the root node.
    // On version 2, there are two entries, one changes address3 and the other changes the root node.

    let _root0 = put_value_set(
        store,
        vec![(key1.clone(), value1.clone()), (key2.clone(), value2)],
        0, /* version */
        None,
    );
    let root1 = put_value_set(
        store,
        vec![
            (key2.clone(), value2_update.clone()),
            (key3.clone(), value3.clone()),
        ],
        1, /* version */
        Some(0),
    );
    let root2 = put_value_set(
        store,
        vec![(key3.clone(), value3_update.clone())],
        2, /* version */
        Some(1),
    );

    // Verify.
    // Prune with limit = 2 and target_min_readable_version = 2, two entries with
    // stale_since_version = 1 will be pruned. min_readable_version will be promoted to 1.
    {
        let pruner = StateMerklePruner::new(Arc::clone(&db.state_merkle_db));
        assert_eq!(
            prune_stale_indices(
                &pruner, 0, /* min_readable_version */
                2, /* target_min_readable_version */
                2  /* limit */
            ),
            1
        );
        assert!(store
            .get_state_value_with_proof_by_version(&key2, 0)
            .is_err());
        // root1 is still there.
        verify_value_and_proof(store, key1.clone(), Some(&value1), 1, root1);
        verify_value_and_proof(store, key2.clone(), Some(&value2_update), 1, root1);
        verify_value_and_proof(store, key3.clone(), Some(&value3), 1, root1);
    }
    // Prune with limit = 1 and target_min_readable_version = 2, one entries with
    // stale_since_version = 2 will be pruned. Min readable version will change even though there
    // is one more entry with stale_since_version = 2 remaining.
    {
        let pruner = StateMerklePruner::new(Arc::clone(&db.state_merkle_db));
        assert_eq!(
            prune_stale_indices(
                &pruner, 1, /* min_readable_version */
                2, /* target_min_readable_version */
                1, /* limit */
            ),
            2
        );
        // root1 is gone.
        assert!(store
            .get_state_value_with_proof_by_version(&key2, 1)
            .is_err());
        // root2 is still there.
        verify_value_and_proof(store, key1.clone(), Some(&value1), 2, root2);
        verify_value_and_proof(store, key2.clone(), Some(&value2_update), 2, root2);
        verify_value_and_proof(store, key3.clone(), Some(&value3_update), 2, root2);
    }
    // Prune with limit = 1 and target_min_readable_version = 2, one entries with
    // stale_since_version = 2 will be pruned. Min_readable_version will change since there is
    // one more entry with stale_since_version = 2 remaining.
    {
        let pruner = StateMerklePruner::new(Arc::clone(&db.state_merkle_db));
        assert_eq!(
            prune_stale_indices(
                &pruner, 1, /* min_readable_version */
                2, /* target_min_readable_version */
                1, /* limit */
            ),
            2
        );
        // root1 is gone.
        assert!(store
            .get_state_value_with_proof_by_version(&key2, 1)
            .is_err());
        // root2 is still there.
        verify_value_and_proof(store, key1, Some(&value1), 2, root2);
        verify_value_and_proof(store, key2, Some(&value2_update), 2, root2);
        verify_value_and_proof(store, key3, Some(&value3_update), 2, root2);
    }
}

#[test]
fn test_stale_node_index_with_target_version() {
    let key1 = StateKey::Raw(String::from("test_key1").into_bytes());
    let key2 = StateKey::Raw(String::from("test_key2").into_bytes());
    let key3 = StateKey::Raw(String::from("test_key3").into_bytes());

    let value1 = StateValue::from(String::from("test_val1").into_bytes());
    let value2 = StateValue::from(String::from("test_val2").into_bytes());
    let value2_update = StateValue::from(String::from("test_val2_update").into_bytes());
    let value3 = StateValue::from(String::from("test_val3").into_bytes());
    let value3_update = StateValue::from(String::from("test_val3_update").into_bytes());

    let tmp_dir = TempPath::new();
    let db = AptosDB::new_for_test_no_cache(&tmp_dir);
    let store = &db.state_store;

    // Update.
    // ```text
    // | batch    | 0      | 1             | 2             |
    // | address1 | value1 |               |               |
    // | address2 | value2 | value2_update |               |
    // | address3 |        | value3        | value3_update |
    // ```
    // The stale node indexes will have 4 entries in total.
    // ```
    // index: StaleNodeIndex { stale_since_version: 1, node_key: NodeKey { version: 0, nibble_path:  } }
    // index: StaleNodeIndex { stale_since_version: 1, node_key: NodeKey { version: 0, nibble_path: 2 } }
    // index: StaleNodeIndex { stale_since_version: 2, node_key: NodeKey { version: 1, nibble_path:  } }
    // index: StaleNodeIndex { stale_since_version: 2, node_key: NodeKey { version: 1, nibble_path: d } }
    // ```
    // On version 1, there are two entries, one changes address2 and the other changes the root node.
    // On version 2, there are two entries, one changes address3 and the other changes the root node.

    let _root0 = put_value_set(
        store,
        vec![(key1.clone(), value1.clone()), (key2.clone(), value2)],
        0, /* version */
        None,
    );
    let root1 = put_value_set(
        store,
        vec![
            (key2.clone(), value2_update.clone()),
            (key3.clone(), value3.clone()),
        ],
        1, /* version */
        Some(0),
    );
    let root2 = put_value_set(
        store,
        vec![(key3.clone(), value3_update.clone())],
        2, /* version */
        Some(1),
    );

    // Verify.
    // Prune with limit = 2 and target_min_readable_version = 1, two entries with
    // stale_since_version = 1 will be pruned. min_readable_version will be promoted to 1. Create a
    // new pruner everytime to test the min_readable_version initialization logic.
    {
        let pruner = StateMerklePruner::new(Arc::clone(&db.state_merkle_db));
        assert_eq!(
            prune_stale_indices(
                &pruner, 0, /* min_readable_version */
                1, /* target_min_readable_version */
                2  /* limit */
            ),
            1
        );
        // root0 is gone.
        println!(
            "store.get_state_value_with_proof_by_version(&key2, 0):{:?}",
            store
                .get_state_value_with_proof_by_version(&key2, 0)
                .err()
                .unwrap()
        );
        assert!(store
            .get_state_value_with_proof_by_version(&key2, 0)
            .is_err());
        // root1 is still there.
        verify_value_and_proof(store, key1.clone(), Some(&value1), 1, root1);
        verify_value_and_proof(store, key2.clone(), Some(&value2_update), 1, root1);
        verify_value_and_proof(store, key3.clone(), Some(&value3), 1, root1);
    }
    // Prune with limit = 1 and target_min_readable_version = 1, entries with
    // stale_since_version = 2 will not be pruned. Create a new pruner everytime to test the
    // min_readable_version initialization logic.
    {
        let pruner = StateMerklePruner::new(Arc::clone(&db.state_merkle_db));
        assert_eq!(
            prune_stale_indices(
                &pruner, 1, /* min_readable_version */
                1, /* target_min_readable_version */
                1, /* limit */
            ),
            1
        );
        // root1 is still there.
        verify_value_and_proof(store, key1.clone(), Some(&value1), 1, root1);
        verify_value_and_proof(store, key2.clone(), Some(&value2_update), 1, root1);
        verify_value_and_proof(store, key3.clone(), Some(&value3), 1, root1);
        // root2 is still there.
        verify_value_and_proof(store, key1, Some(&value1), 2, root2);
        verify_value_and_proof(store, key2, Some(&value2_update), 2, root2);
        verify_value_and_proof(store, key3, Some(&value3_update), 2, root2);
    }
}

#[test]
fn test_stale_node_index_all_at_once() {
    let key1 = StateKey::Raw(String::from("test_key1").into_bytes());
    let key2 = StateKey::Raw(String::from("test_key2").into_bytes());
    let key3 = StateKey::Raw(String::from("test_key3").into_bytes());

    let value1 = StateValue::from(String::from("test_val1").into_bytes());
    let value2 = StateValue::from(String::from("test_val2").into_bytes());
    let value2_update = StateValue::from(String::from("test_val2_update").into_bytes());
    let value3 = StateValue::from(String::from("test_val3").into_bytes());
    let value3_update = StateValue::from(String::from("test_val3_update").into_bytes());

    let tmp_dir = TempPath::new();
    let db = AptosDB::new_for_test_no_cache(&tmp_dir);
    let store = &db.state_store;
    let pruner = StateMerklePruner::new(Arc::clone(&db.state_merkle_db));

    // Update.
    // ```text
    // | batch    | 0      | 1             | 2             |
    // | address1 | value1 |               |               |
    // | address2 | value2 | value2_update |               |
    // | address3 |        | value3        | value3_update |
    // ```
    // The stale node indexes will have 4 entries in total.
    // ```
    // index: StaleNodeIndex { stale_since_version: 1, node_key: NodeKey { version: 0, nibble_path:  } }
    // index: StaleNodeIndex { stale_since_version: 1, node_key: NodeKey { version: 0, nibble_path: 2 } }
    // index: StaleNodeIndex { stale_since_version: 2, node_key: NodeKey { version: 1, nibble_path:  } }
    // index: StaleNodeIndex { stale_since_version: 2, node_key: NodeKey { version: 1, nibble_path: d } }
    // ```
    // On version 1, there are two entries, one changes address2 and the other changes the root node.
    // On version 2, there are two entries, one changes address3 and the other changes the root node.

    let _root0 = put_value_set(
        store,
        vec![(key1.clone(), value1.clone()), (key2.clone(), value2)],
        0, /* version */
        None,
    );
    let _root1 = put_value_set(
        store,
        vec![
            (key2.clone(), value2_update.clone()),
            (key3.clone(), value3),
        ],
        1, /* version */
        Some(0),
    );
    let root2 = put_value_set(
        store,
        vec![(key3.clone(), value3_update.clone())],
        2, /* version */
        Some(1),
    );

    // Verify.
    // Prune with limit = 5, there are 4 stale index entries in total and all the stale index
    // entries will be pruned.
    {
        assert_eq!(
            prune_stale_indices(
                &pruner, 0, /* min_readable_version */
                2, /* target_min_readable_version */
                5, /* limit */
            ),
            2
        );
        // root0 is gone.
        assert!(store
            .get_state_value_with_proof_by_version(&key2, 1)
            .is_err());

        assert!(store
            .get_state_value_with_proof_by_version(&key2, 1)
            .is_err());
        // root2 is still there.
        verify_value_and_proof(store, key1, Some(&value1), 2, root2);
        verify_value_and_proof(store, key2, Some(&value2_update), 2, root2);
        verify_value_and_proof(store, key3, Some(&value3_update), 2, root2);
    }
}

#[test]
pub fn test_get_state_snapshot_before() {
    let tmp_dir = TempPath::new();
    let db = AptosDB::new_for_test(&tmp_dir);
    let store = &db.state_store;

    // Empty store
    assert_eq!(store.get_state_snapshot_before(0).unwrap(), None,);

    // put in genesis
    let kv = vec![(
        StateKey::Raw(b"key".to_vec()),
        StateValue::from(b"value".to_vec()),
    )];
    let hash = put_value_set(store, kv.clone(), 0, None);
    assert_eq!(store.get_state_snapshot_before(0).unwrap(), None);
    assert_eq!(store.get_state_snapshot_before(1).unwrap(), Some((0, hash)));
    assert_eq!(store.get_state_snapshot_before(2).unwrap(), Some((0, hash)));

    // hack: VersionData expected on every version, so duplicate the data at version 1
    let usage = store.get_usage(Some(0)).unwrap();
    store
        .ledger_db
        .put::<VersionDataSchema>(&1, &usage.into())
        .unwrap();

    // put in another version
    put_value_set(store, kv, 2, Some(0));
    assert_eq!(store.get_state_snapshot_before(4).unwrap(), Some((2, hash)));
    assert_eq!(store.get_state_snapshot_before(3).unwrap(), Some((2, hash)));
    assert_eq!(store.get_state_snapshot_before(2).unwrap(), Some((0, hash)));
    assert_eq!(store.get_state_snapshot_before(1).unwrap(), Some((0, hash)));
    assert_eq!(store.get_state_snapshot_before(0).unwrap(), None,);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10))]

    #[test]
    fn test_get_account_iter(
        input in hash_map(any::<StateKey>(), any::<StateValue>(), 1..200)
    ) {
        // Convert to a vector so iteration order becomes deterministic.
        let kvs: Vec<_> = input.into_iter().collect();

        let tmp_dir = TempPath::new();
        let db = AptosDB::new_for_test(&tmp_dir);
        let store = &db.state_store;
        init_store(store, kvs.clone().into_iter());

        // Test iterator at each version.
        for i in 0..kvs.len() {
            let actual_values = db
                .get_backup_handler()
                .get_account_iter(i as Version)
                .unwrap()
                .collect::<Result<Vec<_>>>()
                .unwrap();
            let mut expected_values: Vec<_> = kvs[..=i]
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            expected_values.sort_unstable_by_key(|item| item.0.hash());
            prop_assert_eq!(actual_values, expected_values);
        }
    }

    #[test]
    fn test_raw_restore(
        (input, batch1_size) in hash_map(any::<StateKey>(), any::<StateValue>(), 2..1000)
            .prop_flat_map(|input| {
                let len = input.len();
                (Just(input), 1..len)
            })
    ) {
        let tmp_dir1 = TempPath::new();
        let db1 = AptosDB::new_for_test(&tmp_dir1);
        let store1 = &db1.state_store;
        init_store(store1, input.clone().into_iter());

        let version = (input.len() - 1) as Version;
        let expected_root_hash = store1.get_root_hash(version).unwrap();

        let tmp_dir2 = TempPath::new();
        let db2 = AptosDB::new_for_test(&tmp_dir2);
        let store2 = &db2.state_store;

        let mut restore =
            StateSnapshotRestore::new(&store2.state_merkle_db, store2, version, expected_root_hash).unwrap();

        let mut ordered_input: Vec<_> = input
            .into_iter()
            .collect();
        ordered_input.sort_unstable_by_key(|(key, _value)| key.hash());

        let batch1: Vec<_> = ordered_input
            .clone()
            .into_iter()
            .take(batch1_size)
            .collect();
        let rightmost_of_batch1 = batch1.last().map(|(key, _value)| key.hash()).unwrap();
        let proof_of_batch1 = store1
            .get_value_range_proof(rightmost_of_batch1, version)
            .unwrap();

        restore.add_chunk(batch1, proof_of_batch1).unwrap();

        let batch2: Vec<_> = ordered_input
            .into_iter()
            .skip(batch1_size)
            .collect();
        let rightmost_of_batch2 = batch2.last().map(|(key, _value)| key.hash()).unwrap();
        let proof_of_batch2 = store1
            .get_value_range_proof(rightmost_of_batch2, version)
            .unwrap();

        restore.add_chunk(batch2, proof_of_batch2).unwrap();

        restore.finish().unwrap();

        let actual_root_hash = store2.get_root_hash(version).unwrap();
        prop_assert_eq!(actual_root_hash, expected_root_hash);
    }

    #[test]
    fn test_restore(
        (input, batch_size) in hash_map(any::<StateKey>(), any::<StateValue>(), 2..1000)
            .prop_flat_map(|input| {
                let len = input.len();
                (Just(input), 1..len*2)
            })
    ) {
        let tmp_dir1 = TempPath::new();
        let db1 = AptosDB::new_for_test(&tmp_dir1);
        let store1 = &db1.state_store;
        init_store(store1, input.clone().into_iter());

        let version = (input.len() - 1) as Version;
        let expected_root_hash = store1.get_root_hash(version).unwrap();
        prop_assert_eq!(
            store1.get_value_count(version).unwrap(),
            input.len()
        );

        let tmp_dir2 = TempPath::new();
        let db2 = AptosDB::new_for_test(&tmp_dir2);
        let store2 = &db2.state_store;

        let mut restore = store2.get_snapshot_receiver(version, expected_root_hash).unwrap();
        let mut current_idx = 0;
        while current_idx < input.len() {
            let chunk = store1.get_value_chunk_with_proof(version, current_idx, batch_size).unwrap();
            restore.add_chunk(chunk.raw_values, chunk.proof).unwrap();
            current_idx += batch_size;
        }

        restore.finish_box().unwrap();
        let actual_root_hash = store2.get_root_hash(version).unwrap();
        prop_assert_eq!(actual_root_hash, expected_root_hash);
        prop_assert_eq!(
            store2.get_value_count(version).unwrap(),
            input.len()
        );
    }

    #[test]
    fn test_get_rightmost_leaf(
        (input, batch1_size) in hash_map(any::<StateKey>(), any::<StateValue>(), 2..1000)
            .prop_flat_map(|input| {
                let len = input.len();
                (Just(input), 1..len)
            })
    ) {
        let tmp_dir1 = TempPath::new();
        let db1 = AptosDB::new_for_test(&tmp_dir1);
        let store1 = &db1.state_store;
        init_store(store1, input.clone().into_iter());

        let version = (input.len() - 1) as Version;
        let expected_root_hash = store1.get_root_hash(version).unwrap();

        let tmp_dir2 = TempPath::new();
        let db2 = AptosDB::new_for_test(&tmp_dir2);
        let store2 = &db2.state_store;

        let mut restore =
            StateSnapshotRestore::new(&store2.state_merkle_db, store2, version, expected_root_hash).unwrap();

        let mut ordered_input: Vec<_> = input
            .into_iter()
            .collect();
        ordered_input.sort_unstable_by_key(|(key, _value)| key.hash());

        let batch1: Vec<_> = ordered_input
            .into_iter()
            .take(batch1_size)
            .collect();
        let rightmost_of_batch1 = batch1.last().map(|(key, _value)| key.hash()).unwrap();
        let proof_of_batch1 = store1
            .get_value_range_proof(rightmost_of_batch1, version)
            .unwrap();

        restore.add_chunk(batch1, proof_of_batch1).unwrap();

        let expected = store2.state_merkle_db.get_rightmost_leaf_naive().unwrap();
        let actual = store2.state_merkle_db.get_rightmost_leaf().unwrap();
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn test_get_usage(
        input in arb_state_kv_sets(10, 5, 5)
    ) {
        let tmp_dir = TempPath::new();
        let db = AptosDB::new_for_test(&tmp_dir);
        let store = &db.state_store;

        let mut version = 0;
        for batch in input {
            let next_version = version + batch.len() as Version;
            let root_hash = update_store(store, batch.into_iter(), version);

            let last_version = next_version - 1;
            let snapshot = db
                .get_backup_handler()
                .get_account_iter(last_version)
                .unwrap()
                .collect::<Result<Vec<_>>>()
                .unwrap();
            let (items, bytes) = snapshot.iter().fold((0, 0), |(items, bytes), (k, v)| {
                (items + 1, bytes + k.size() + v.size())
            });
            let expected_usage = StateStorageUsage::new(items, bytes);
            prop_assert_eq!(
                expected_usage,
                store.get_usage(Some(last_version)).unwrap(),
                "version: {} next_version: {}",
                version,
                next_version,
            );

            // Check db-restore calculates usage correctly as well.
            let tmp_dir = TempPath::new();
            let db2 = AptosDB::new_for_test(&tmp_dir);
            let mut restore = db2.get_state_snapshot_receiver(100, root_hash).unwrap();
            let proof = if let Some((k, _v)) = snapshot.last() {
                db.get_backup_handler().get_account_state_range_proof(k.hash(), last_version).unwrap()
            } else {
                SparseMerkleRangeProof::new(vec![])
            };
            restore.add_chunk(snapshot, proof).unwrap();
            restore.finish_box().unwrap();
            prop_assert_eq!(
                expected_usage,
                db2.state_store.get_usage(Some(100)).unwrap(),
                "version: {} next_version: {}",
                version,
                next_version,
            );

            version = next_version;
        }

    }
}

// Initializes the state store by inserting one key at each version.
fn init_store(store: &StateStore, input: impl Iterator<Item = (StateKey, StateValue)>) {
    update_store(store, input.into_iter().map(|(k, v)| (k, Some(v))), 0);
}
