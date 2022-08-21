/// The chain id distinguishes between different chains (e.g., testnet and the main network).
/// One important role is to prevent transactions intended for one chain from being executed on another.
/// This code provides a container for storing a chain id and functions to initialize and get it.
module aptos_framework::chain_id {
    use aptos_framework::system_addresses;
    use aptos_framework::timestamp;

    friend aptos_framework::genesis;

    struct ChainId has key {
        id: u8
    }

    /// Only called during genesis.
    /// Publish the chain ID `id` of this instance under the SystemAddresses address
    public(friend) fun initialize(account: &signer, id: u8) {
        system_addresses::assert_aptos_framework(account);
        move_to(account, ChainId { id })
    }

    /// Return the chain ID of this instance
    public fun get(): u8 acquires ChainId {
        timestamp::assert_operating();
        borrow_global<ChainId>(@aptos_framework).id
    }
}
