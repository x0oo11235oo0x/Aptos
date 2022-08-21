#!/usr/bin/env python3

# Copyright (c) Aptos
# SPDX-License-Identifier: Apache-2.0

from typing import Any, Dict

import requests

from first_transaction import FAUCET_URL, TESTNET_URL, Account, FaucetClient, RestClient

U64_MAX = 18446744073709551615


class TokenClient(RestClient):
    def submit_transaction_helper(self, account: Account, payload: Dict[str, Any]):
        res = self.execute_transaction_with_payload(account, payload)
        self.wait_for_transaction(res["hash"])

    #:!:>section_1
    def create_collection(self, account: Account, name: str, description, uri: str):
        """Creates a new collection within the specified account"""
        mutate_setting = [False, False, False]
        payload = {
            "type": "entry_function_payload",
            "function": f"0x3::token::create_collection_script",
            "type_arguments": [],
            "arguments": [
                name.encode("utf-8").hex(),
                description.encode("utf-8").hex(),
                uri.encode("utf-8").hex(),
                str(U64_MAX),
                mutate_setting,
            ],
        }
        self.submit_transaction_helper(account, payload)

    # <:!:section_1

    #:!:>section_2
    def create_token(
        self,
        account: Account,
        collection_name: str,
        name: str,
        description: str,
        supply: int,
        uri: str,
    ):
        mutate_setting = [False, False, False, False, False]
        payload = {
            "type": "entry_function_payload",
            "function": f"0x3::token::create_token_script",
            "type_arguments": [],
            "arguments": [
                collection_name.encode("utf-8").hex(),
                name.encode("utf-8").hex(),
                description.encode("utf-8").hex(),
                str(supply),
                str(U64_MAX),
                uri.encode("utf-8").hex(),
                account.address(),
                str(0),  # royalty_denominator
                str(0),
                mutate_setting,
                ["GiftTo".encode("utf-8").hex()],
                ["Bob".encode("utf-8").hex()],
                ["string".encode("utf-8").hex()],
            ],
        }
        self.submit_transaction_helper(account, payload)

    # <:!:section_2

    #:!:>section_4
    def offer_token(
        self,
        account: Account,
        receiver: str,
        creator: str,
        collection_name: str,
        token_name: str,
        amount: int,
    ):
        payload = {
            "type": "entry_function_payload",
            "function": f"0x3::token_transfers::offer_script",
            "type_arguments": [],
            "arguments": [
                receiver,
                creator,
                collection_name.encode("utf-8").hex(),
                token_name.encode("utf-8").hex(),
                str(0),
                str(amount),
            ],
        }
        self.submit_transaction_helper(account, payload)

    # <:!:section_4

    #:!:>section_5
    def claim_token(
        self,
        account: Account,
        sender: str,
        creator: str,
        collection_name: str,
        token_name: str,
    ):
        payload = {
            "type": "entry_function_payload",
            "function": f"0x3::token_transfers::claim_script",
            "type_arguments": [],
            "arguments": [
                sender,
                creator,
                collection_name.encode("utf-8").hex(),
                token_name.encode("utf-8").hex(),
                str(0),
            ],
        }
        self.submit_transaction_helper(account, payload)

    # <:!:section_5

    #:!:>section_6
    def mutate_token_properties(
        self,
        account: Account,
        token_owner: str,
        creator: str,
        collection_name: str,
        token_name: str,
        property_version: int,
        amount: int,
        keys: list[str],
        values: list[str],
        types: list[str],
    ):
        payload = {
            "type": "entry_function_payload",
            "function": f"0x3::token::mutate_token_properties",
            "type_arguments": [],
            "arguments": [
                token_owner,
                creator,
                collection_name.encode("utf-8").hex(),
                token_name.encode("utf-8").hex(),
                str(property_version),
                str(amount),
                [k.encode("utf-8").hex() for k in keys],
                [v.encode("utf-8").hex() for v in values],
                [t.encode("utf-8").hex() for t in types],
            ],
        }
        self.submit_transaction_helper(account, payload)

    # <:!:section_6

    #:!:>section_3
    def get_table_item(
        self, handle: str, key_type: str, value_type: str, key: Any
    ) -> Any:
        response = requests.post(
            f"{self.url}/tables/{handle}/item",
            json={
                "key_type": key_type,
                "value_type": value_type,
                "key": key,
            },
        )
        assert response.status_code == 200, response.text
        return response.json()

    def get_token_balance(
        self, owner: str, creator: str, collection_name: str, token_name: str
    ) -> Any:
        token_store = self.account_resource(owner, "0x3::token::TokenStore")["data"][
            "tokens"
        ]["handle"]
        token_id = {
            "token_data_id": {
                "creator": creator,
                "collection": collection_name.encode("utf-8").hex(),
                "name": token_name.encode("utf-8").hex(),
            },
            "property_version": str(0),
        }

        return self.get_table_item(
            token_store,
            "0x3::token::TokenId",
            "0x3::token::Token",
            token_id,
        )["amount"]

    def get_token_data(
        self, creator: str, collection_name: str, token_name: str
    ) -> Any:
        token_data = self.account_resource(creator, "0x3::token::Collections")["data"][
            "token_data"
        ]["handle"]

        token_data_id = {
            "creator": creator,
            "collection": collection_name.encode("utf-8").hex(),
            "name": token_name.encode("utf-8").hex(),
        }

        return self.get_table_item(
            token_data,
            "0x3::token::TokenDataId",
            "0x3::token::TokenData",
            token_data_id,
        )

    def get_collection(self, creator: str, collection_name: str) -> Any:
        token_data = self.account_resource(creator, "0x3::token::Collections")["data"][
            "collection_data"
        ]["handle"]

        return self.get_table_item(
            token_data,
            "0x1::string::String",
            "0x3::token::CollectionData",
            collection_name.encode("utf-8").hex(),
        )

    # <:!:section_3

    def cancel_token_offer(
        self,
        account: Account,
        receiver: str,
        creator: str,
        collection_name: str,
        token_name: str,
    ):
        payload = {
            "type": "entry_function_payload",
            "function": f"0x3::token_transfers::cancel_offer_script",
            "type_arguments": [],
            "arguments": [
                receiver,
                creator,
                collection_name.encode("utf-8").hex(),
                token_name.encode("utf-8").hex(),
                0,
            ],
        }
        self.submit_transaction_helper(account, payload)


if __name__ == "__main__":
    client = TokenClient(TESTNET_URL)
    faucet_client = FaucetClient(FAUCET_URL, client)

    alice = Account()
    bob = Account()
    jack = Account()
    collection_name = "Alice's cat collection"
    token_name = "Alice's tabby"

    print("\n=== Addresses ===")
    print(f"Alice: {alice.address()}")
    print(f"Bob: {bob.address()}")
    print(f"Jack: {bob.address()}")

    faucet_client.fund_account(alice.address(), 10_000_000)
    faucet_client.fund_account(bob.address(), 10_000_000)
    faucet_client.fund_account(jack.address(), 10_000_000)

    print("\n=== Initial Balances ===")
    print(f"Alice: {client.account_balance(alice.address())}")
    print(f"Bob: {client.account_balance(bob.address())}")
    print(f"Jack: {client.account_balance(jack.address())}")

    print("\n=== Creating Collection and Token ===")

    client.create_collection(
        alice, collection_name, "Alice's cat collection", "https://aptos.dev"
    )
    client.create_token(
        alice,
        collection_name,
        token_name,
        "Alice's simple token",
        1,
        "https://aptos.dev/img/nyan.jpeg",
    )
    print(
        f"Alice's collection: {client.get_collection(alice.address(), collection_name)}"
    )
    print(
        f"Alice's token balance: {client.get_token_balance(alice.address(), alice.address(), collection_name, token_name)}"
    )
    print(
        f"Alice's token data: {client.get_token_data(alice.address(), collection_name, token_name)}"
    )

    print("\n=== Transferring the token to Bob ===")
    client.offer_token(
        alice, bob.address(), alice.address(), collection_name, token_name, 1
    )
    client.claim_token(
        bob, alice.address(), alice.address(), collection_name, token_name
    )
    print(
        f"Alice's token balance: {client.get_token_balance(alice.address(), alice.address(), collection_name, token_name)}"
    )
    print(
        f"Bob's token balance: {client.get_token_balance(bob.address(), alice.address(), collection_name, token_name)}"
    )

    print("\n=== On-chain lazy mint through semi-fungible token ===")
    large_volume_token_name = "Alice's fancy cat"
    # alice creates 10 million fancy cat fungible tokens in one txn
    client.create_token(
        alice,
        collection_name,
        large_volume_token_name,
        "Alice's high demand token",
        10000000,
        "https://aptos.dev/img/nyan.jpeg",
    )

    # alice mutate the 1 token. The token has a new property_version and new token ID, thus become an NFT now.
    client.mutate_token_properties(
        alice,
        alice.address(),
        alice.address(),
        collection_name,
        large_volume_token_name,
        0,
        1,
        ["GiftTo".encode("utf-8").hex()],
        ["Jack".encode("utf-8").hex()],
        ["string".encode("utf-8").hex()],
    )
    # alice transfer the token to Jack
    client.offer_token(
        alice,
        jack.address(),
        alice.address(),
        collection_name,
        large_volume_token_name,
        1,
    )
    client.claim_token(
        jack, alice.address(), alice.address(), collection_name, large_volume_token_name
    )
    print(
        f"Alice's uninitialized token balance: {client.get_token_balance(alice.address(), alice.address(), collection_name, large_volume_token_name)}"
    )
    print(
        f"Jack's token balance: {client.get_token_balance(jack.address(), alice.address(), collection_name, large_volume_token_name)}"
    )
