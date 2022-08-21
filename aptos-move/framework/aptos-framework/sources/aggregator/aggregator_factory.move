/// This module provides foundations to create aggregators in the system.
///
/// Design rationale (V1)
/// =====================
/// First, we encourage the reader to see rationale of `Aggregator` in
/// `aggregator.move`.
///
/// Recall that the value of any aggregator can be identified in storage by
/// (handle, key) pair. How this pair can be generated? Short answer: with
/// `AggregatorFactory`!
///
/// `AggregatorFactory` is a struct that can be stored as a resource on some
/// account and which contains a `phantom_table` field. When the factory is
/// initialized, we initialize this table. Importantly, table initialization
/// only generates a uniue table `handle` - something we can reuse.
///
/// When the user wants to create a new aggregator, he/she calls a constructor
/// provided by the factory (`create_aggregator(..)`). This constructor generates
/// a unique key, which with the handle is used to initialize `Aggregator` struct.
///
/// Use cases
/// =========
/// We limit the usage of `AggregatorFactory` by only storing it on the core
/// account.
///
/// When something whants to use an aggregator, the factory is queried and an
/// aggregator instance is created. Once aggregator is no longer in use, it
/// should be destroyed by the user.
module aptos_framework::aggregator_factory {
    use std::error;

    use aptos_framework::system_addresses;
    use aptos_std::aggregator::Aggregator;
    use aptos_std::table::{Self, Table};

    friend aptos_framework::optional_aggregator;

    #[test_only]
    friend aptos_framework::aggregator_tests;

    /// When aggregator factory is not published yet.
    const EAGGREGATOR_FACTORY_NOT_FOUND: u64 = 1;

    /// Struct that creates aggregators.
    struct AggregatorFactory has key {
        phantom_table: Table<u128, u128>,
    }

    /// Creates a new factory for aggregators.
    public fun initialize_aggregator_factory(account: &signer) {
        system_addresses::assert_aptos_framework(account);
        let aggregator_factory = AggregatorFactory {
            phantom_table: table::new()
        };
        move_to(account, aggregator_factory);
    }

    /// Creates a new aggregator instance which overflows on exceeding a `limit`.
    public(friend) fun create_aggregator(limit: u128): Aggregator acquires AggregatorFactory {
        assert!(
            exists<AggregatorFactory>(@aptos_framework),
            error::not_found(EAGGREGATOR_FACTORY_NOT_FOUND)
        );

        let aggregator_factory = borrow_global_mut<AggregatorFactory>(@aptos_framework);
        new_aggregator(aggregator_factory, limit)
    }

    /// Similar to `create_aggregator` but takes a signer as well.
    public fun create_aggregator_signed(account: &signer, limit: u128): Aggregator acquires AggregatorFactory {
        system_addresses::assert_aptos_framework(account);

        assert!(
            exists<AggregatorFactory>(@aptos_framework),
            error::not_found(EAGGREGATOR_FACTORY_NOT_FOUND)
        );

        let aggregator_factory = borrow_global_mut<AggregatorFactory>(@aptos_framework);
        new_aggregator(aggregator_factory, limit)
    }

    native fun new_aggregator(aggregator_factory: &mut AggregatorFactory, limit: u128): Aggregator;
}
