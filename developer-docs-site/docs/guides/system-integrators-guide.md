---
title: "System Integrators Guide"
slug: "system-integrators-guide"
---

:::tip 
This documentation is currently under construction with more being added on a regular basis.
:::

If you provide blockchain services to your customers and wish to add the Aptos blockchain to your platform, then this guide is for you. This system integrators guide will walk you through all you need to integrate the Aptos blockchain into your platform. This guide assumes that you are familiar with the blockchains.

## Overview

This guide will overview the following topics for integrating with Aptos:
- Preparing an environment for testing.
- Create an account on the blockchain.
- Exchange account identifiers with another entity on the blockchain, for example, to perform swaps.
- Create a transaction.
- Obtain a gas estimate and validate the transaction for correctness.
- Submit the transaction to the blockchain.
- Wait for the outcome of the transaction.
- Query historical transactions and interactions for a given account with a specific account, i.e., withdraws and deposits.

## Getting Started

There are two well-supported approaches for integrating with the Aptos blockchain:

1. Local development using our standalone testnet
2. Devnet -- a shared resource for the community

### Local Testnet

There are two options to run a local testnet:
1. Directly run a local testnet is to follow [this guide](/nodes/local-testnet/run-a-local-testnet/).
2. Use the CLI by 1) [installing with the CLI](/cli-tools/aptos-cli-tool/install-aptos-cli) and 2) start a [local node with a faucet](/nodes/local-testnet/using-cli-to-run-a-local-testnet#starting-a-local-testnet-with-a-faucet)

This will expose a REST API service at `http://127.0.0.1:8080/v1` and a Faucet service at `http://127.0.0.1:8000` for option 1 or `http://127.0.0.1:8081` for option 2. The applications will output the location of the services.

### Access Devnet

Faucet service: https://faucet.devnet.aptoslabs.com
REST API service: https://fullnode.devnet.aptoslabs.com/v1

### SDKs

Aptos currently has two SDKs:
1. [Python](/sdks/python-sdk)
2. [Typescript](/sdks/transactions-with-ts-sdk)


### Other Areas

* [Using the CLI](../cli-tools/aptos-cli-tool/use-aptos-cli) which includes creating accounts, transferring coins, and publishing modules
* [Typescript SDK](../transactions-with-ts-sdk)
* [Python SDK](../sdks/python-sdk)
* [REST API spec](https://fullnode.devnet.aptoslabs.com/v1/spec#/)
* [Local testnet development flow](/guides/local-testnet-dev-flow)

## Accounts on Aptos

An [account](/concepts/basics-accounts) represents a resource on the Aptos blockchain that can send transactions. Each account is identified by a particular 32-byte account address and is a container for Move modules and Move resources. On Aptos, accounts must be created on-chain prior to any blockchain operations involving that account. The Aptos framework supports implicitly creating accounts when transferring Aptos coin via [`account::transfer`](https://github.com/aptos-labs/aptos-core/blob/60751b5ed44984178c7163933da3d1b18ad80388/aptos-move/framework/aptos-framework/sources/account.move#L398) or explicitly via [`account::create_account`](https://github.com/aptos-labs/aptos-core/blob/60751b5ed44984178c7163933da3d1b18ad80388/aptos-move/framework/aptos-framework/sources/account.move#L370).

At creation, an [Aptos account](https://github.com/aptos-labs/aptos-core/blob/60751b5ed44984178c7163933da3d1b18ad80388/aptos-move/framework/aptos-framework/sources/account.move#L20) contains:
* A [resource containing Aptos Coin](https://github.com/aptos-labs/aptos-core/blob/60751b5ed44984178c7163933da3d1b18ad80388/aptos-move/framework/aptos-framework/sources/coin.move#L50) and deposit and withdrawal of coins from that resource.
* An authentication key associated with their current public, private key(s).
* A strictly increasing sequence number that represents the account's next transaction's sequence number to prevent replay attacks.
* An event stream for all new types of coins added to the account.

### Account identifiers

Currently, Aptos only supports a single, unified identifier for an account. Accounts on Aptos are universally represented as a 32-byte hex string. A hex string shorter than 32-bytes should is also valid, in those scenarios, the the hex string is padded with leading zeroes, e.g., `0x1` => `0x0000000000000...01`.

### Creating an account address

Account addresses are defined at creation time as a one-way function from the public key(s) and signature algorithm used for authentication for the account.

:::tip Read more
This is covered in depth in the [Accounts](https://aptos.dev/concepts/basics-accounts/) documentation and demonstrated in the [Typescript SDK](https://github.com/aptos-labs/aptos-core/blob/9b85d41ed8ef4a61a9cd64f9de511654fcc02024/ecosystem/typescript/sdk/src/aptos_account.ts#L66) and [Python SDK](https://github.com/aptos-labs/aptos-core/blob/9b85d41ed8ef4a61a9cd64f9de511654fcc02024/ecosystem/python/sdk/aptos_sdk/account_address.py#L43). Note that currently these SDKs only demonstrate how to generate an address from an Ed25519 single signer.
:::

### Rotating the keys

An Account on Aptos has the ability to rotate keys so that potentially compromised keys cannot be used to access the accounts. Keys can be rotated via [`account::rotate_authentication_key`](https://github.com/aptos-labs/aptos-core/blob/60751b5ed44984178c7163933da3d1b18ad80388/aptos-move/framework/aptos-framework/sources/account.move#L183) function.

:::tip Read more
See more in [Account address](/concepts/basics-accounts#account-address).
:::

Refreshing the keys is generally regarded as good hygiene in the security field. However, this presents a challenge for system integrators who are used to using a mnemonic to represent both a private key and its associated account. To simplify this for the system integrators, Aptos will provide an on-chain mapping, before the launch of the mainnet. The on-chain data maps an effective account address as defined by the current mnemonic to the actual account address.

### Preventing replay attacks

When the Aptos blockchain processes the transaction, it looks at the sequence number in the transaction and compares it with the sequence number in the sender’s account (as stored on the blockchain at the current ledger version). The transaction is executed only if the sequence number in the transaction is the same as the sequence number for the sender account, and rejects if they do not match. In this way past transactions, which necessarily contain older sequence numbers, cannot be replayed, hence preventing replay attacks.

:::tip Read more
See more on [Account sequence number here](/concepts/basics-accounts#account-sequence-number).
:::

## Transactions

Aptos [transactions](/concepts/basics-txns-states) are encoded in [BCS](https://github.com/diem/bcs) (Binary Canonical Serialization). Transactions contain  information such as the sender’s account address, authentication from the sender, the desired operation to be performed on the Aptos blockchain, and the amount of gas the sender is willing to pay to execute the transaction.

### Transaction states

A transaction may end in one of the following states:

1. Committed on the blockchain and executed. This is considered as a successful transaction.
2. Committed on the blockchain and aborted. The abort code indicates why the transaction failed to execute.
3. Discarded during transaction submission due to a validation check such as insufficient gas, invalid transaction format, or incorrect key.
4. Discarded after transaction submission but before attempted execution. This could be due to timeouts or insufficient gas due to other transactions affecting the account.

The sender’s account will be charged gas for any committed transactions.

During transaction submission, the submitter is notified of successful submission or a reason for failing validations otherwise.

A transaction that is successfully submitted but ultimately discarded may have no visible state in any accessible Aptos node or within the Aptos network. A user can attempt to resubmit the same transaction to re-validate the transaction. If the submitting node believes that this transaction is still valid, this will return an error stating that there exists an identical transaction already submitted.

The submitter can try to increase the gas cost by a trivial amount to help make progress and adjust for whatever may have been causing the discarding of the transaction further downstream.

On the Aptos devnet, the time between submission and confirmation is within seconds.

:::tip Read more
See [here for a comprehensive description of the transaction lifecycle](/guides/basics-life-of-txn).
:::

### Constructing a transaction

Aptos supports two methods for constructing transactions:

- Constructing JSON-encoded objects and interacting with the Web API to generate native transactions.
- Using the Aptos client libraries to generate native transactions.

#### JSON-encoded transactions

JSON-encoded transactions can be generated via the [REST API](https://fullnode.devnet.aptoslabs.com/v1/spec#/), following these steps:

- First construct an appropriate JSON payload for the `/transactions/encode_submission` endpoint as demonstrated in the [Python SDK](https://github.com/aptos-labs/aptos-core/blob/b0fe7ea6687e9c180ebdbac8d8eb984d11d7e4d4/ecosystem/python/sdk/aptos_sdk/client.py#L128).
- The output of the above contains an object containing a `message` and this must be signed with the sender’s private key locally.
- Finally, the original JSON payload is extended with the signature information and posted to the `/transactions` [endpoint](https://github.com/aptos-labs/aptos-core/blob/b0fe7ea6687e9c180ebdbac8d8eb984d11d7e4d4/ecosystem/python/sdk/aptos_sdk/client.py#L142). This will return back a transaction submission result that, if successful, contains a transaction hash in the `hash` [field](https://github.com/aptos-labs/aptos-core/blob/b0fe7ea6687e9c180ebdbac8d8eb984d11d7e4d4/ecosystem/python/sdk/aptos_sdk/client.py#L145).

JSON-encoded transactions allow for rapid development and support seamless ABI conversions of transaction arguments to native types. However, most system integrators prefer to generate transactions within their own tech stack. Both the [TypeScript SDK](https://github.com/aptos-labs/aptos-core/blob/9b85d41ed8ef4a61a9cd64f9de511654fcc02024/ecosystem/typescript/sdk/src/aptos_client.ts#L259) and [Python SDK](https://github.com/aptos-labs/aptos-core/blob/b0fe7ea6687e9c180ebdbac8d8eb984d11d7e4d4/ecosystem/python/sdk/aptos_sdk/client.py#L100) support generating BCS transactions.

#### BCS-encoded transactions

BCS encoded transactions can be submitted to the `/transactions` endpoint but must specify `Content-Type: application/x.aptos.signed_transaction+bcs` in the HTTP headers. This will return back a transaction submission result that, if successful, contains a transaction hash in the `hash` [field](https://github.com/aptos-labs/aptos-core/blob/9b85d41ed8ef4a61a9cd64f9de511654fcc02024/ecosystem/python/sdk/aptos_sdk/client.py#L138).

### Types of Transactions

Within a given transaction, the target of execution can be one of two types: 

- An entry point (formerly known as script function), and/or
- A script (payload). 

Currently the SDKs: [Python](https://github.com/aptos-labs/aptos-core/blob/b0fe7ea6687e9c180ebdbac8d8eb984d11d7e4d4/ecosystem/python/sdk/aptos_sdk/client.py#L249) and [Typescript](https://github.com/aptos-labs/aptos-core/blob/76b654b54dcfc152de951a728cc1e3f9559d2729/ecosystem/typescript/sdk/src/aptos_client.test.ts#L98) only support the generation of transactions that target entry points. This guide points out many of those entry points, such as `coin::transfer` and `account::create_account`. 

All operations on the Aptos blockchain should be available via entry point calls. While one could submit multiple transactions calling entry points in series, many such operations may benefit from being called atomically from a single transaction. A script payload transaction can call any entry point or public function defined within any module. 

:::tip Move book
Currently there are no tutorials in this guide on script payloads, but the [Move book](https://move-language.github.io/move/modules-and-scripts.html?highlight=script#scripts) does go in some depth.
:::

:::tip Read more

See the following documentation for generating valid transactions:

- [JSON encoded transactions](http://aptos.dev/tutorials/your-first-transaction) via Typescript, Python, and Rust.
- [Python example of BCS encoded coin transfers](https://github.com/aptos-labs/aptos-core/blob/9b85d41ed8ef4a61a9cd64f9de511654fcc02024/ecosystem/python/sdk/aptos_sdk/client.py#L240).
- [Typescript example of BCS encoded coin transfers](https://github.com/aptos-labs/aptos-core/blob/9b85d41ed8ef4a61a9cd64f9de511654fcc02024/ecosystem/typescript/sdk/src/aptos_client.test.ts#L122).
- [CLI-based transaction publishing](http://aptos.dev/cli-tools/aptos-cli-tool/use-aptos-cli#publishing-a-move-package-with-a-named-address).
- [Publish your first Move module](https://aptos.dev/tutorials/your-first-move-module) via Typescript, Python, and Rust.

:::

### Status of a transaction

Transaction status can be obtained by querying the API [`/transactions/by_hash/{hash}`](https://fullnode.devnet.aptoslabs.com/v1/spec#/operations/get_transaction_by_hash) with the hash returned during the submission of the transaction.

A reasonable strategy for submitting transactions is to limit their lifetime to 30 to 60 seconds, and polling that API at regular intervals until success or a several seconds after that time has elapsed. If there is no commitment on-chain, the transaction was likely discarded.

### Testing transactions or transaction pre-execution

To facilitate evaluation of transactions, Aptos supports a simulation API that does not require and should not contain valid signatures on transactions.

The simulation API works identical to the transaction submission API, except that it executes the transaction and returns back the results along with the gas used. The simulation API can be accessed by submitting a transaction to [`/transactions/simulate`](https://fullnode.devnet.aptoslabs.com/v1/spec#/operations/simulate_transaction).

:::tip Read more
Here's an example showing how to use the simulation API in the [Typescript SDK](https://github.com/aptos-labs/aptos-core/blob/9b85d41ed8ef4a61a9cd64f9de511654fcc02024/ecosystem/typescript/sdk/src/aptos_client.ts#L413). Note that the gas use may change based upon the state of the account. We recommend that the maximum gas amount be larger than the amount quoted by this API.
:::

## Viewing current and historical state

Most integrations into the Aptos blockchain benefit from a holistic and comprehensive overview of the current and historical state of the blockchain. Aptos provides historical transactions, state, and events, which are the result of transaction execution.

* Historical transactions specify the execution status, output, and tie to related events. Each transaction has a unique version number associated with it that dictates its global sequential ordering in the history of the blockchain ledger.
* The state is the representation of all transaction outputs up to a specific version. In otherwords, a state version is the accumulation of all transactions inclusive of that transaction version.
* As transactions execute, they may emit events. [Events](/concepts/basics-events) are hints about changes in on-chain data.

The storage service on a node employs two forms of pruning that erase data from nodes: 

1. state, and 
2. events, transactions, and everything else.

While either of these may be disabled, storing the state versions is not particularly sustainable. 

Events and transactions pruning can be disabled via setting the [`enable_ledger_pruner`](https://github.com/aptos-labs/aptos-core/blob/cf0bc2e4031a843cdc0c04e70b3f7cd92666afcf/config/src/config/storage_config.rs#L141)) to `false`. This will be default behavior in Mainnet. In the near future, Aptos will provide indexers that mitigate the need to directly query from a node.

The REST API contains the following useful APIs for querying transactions and events:

* [Transactions for an account](https://fullnode.devnet.aptoslabs.com/v1/spec#/operations/get_account_transactions)
* [Transactions by version](https://fullnode.devnet.aptoslabs.com/v1/spec#/operations/get_transaction_by_version)
* [Events by event handle](https://fullnode.devnet.aptoslabs.com/v1/spec#/operations/get_events_by_event_handle)

## Exchanging and tracking coins

Aptos has a standard [Coin type](https://github.com/aptos-labs/aptos-core/blob/main/aptos-move/framework/aptos-framework/sources/coin.move). Different types of coins can be represented in this type through the use of distinct structs that represent the type parameter or generic for `Coin<T>`. 

Coins are stored within an account under the resource `CoinStore<T>`. At account creation, each user has the resource `CoinStore<0x1::aptos_coin::AptosCoin>` or `CoinStore<AptosCoin>`, for short. Within this resource is the Aptos coin: `Coin<AptosCoin>`.

### Transferring coins between users

Coins can be transferred between users via the [`coin::transfer`](https://github.com/aptos-labs/aptos-core/blob/36a7c00b29a457469264187d8e44070b2d5391fe/aptos-move/framework/aptos-framework/sources/coin.move#L307) function for all coins and [`account::transfer`](https://github.com/aptos-labs/aptos-core/blob/36a7c00b29a457469264187d8e44070b2d5391fe/aptos-move/framework/aptos-framework/sources/account.move#L398) for Aptos coins. The advantage of the latter function is that it creates the destination account if it does not exist. 

:::caution
It is important to note, that if an account has not registered a `CoinStore<T>` for a given `T`, then any transfer of type `T` to that account will fail.
:::

### Current balance for a coin

The current balance for a `Coin<T>` where T is the Aptos coin is available at the account resources url: `https://{rest_api_server}/accounts/{address}/resource/0x1::coin::CoinStore<0x1::aptos_coin::AptosCoin>`. The balance is stored within `coin::amount`. The resource also contains the total number of deposit and withdraw events, the `counter` value within `deposit_event` and `withdraw_event`, respectively.

```
{
  "type": "0x1::coin::CoinStore<0x1::aptos_coin::AptosCoin>",
  "data": {
    "coin": {
      "value": "3927"
    },
    "deposit_events": {
      "counter": "1",
      "guid": {
        "id": {
          "addr": "0xcb2f940705c44ba110cd3b4f6540c96f2634938bd5f2aabd6946abf12ed88457",
          "creation_num": "1"
        }
      }
    },
    "withdraw_events": {
      "counter": "1",
      "guid": {
        "id": {
          "addr": "0xcb2f940705c44ba110cd3b4f6540c96f2634938bd5f2aabd6946abf12ed88457",
          "creation_num": "2"
        }
      }
    }
  }
}
```

### Querying events

Aptos provides clear and canonical events for all withdraw and deposit of coins. This can be used in coordination with the associated transactions to present to a user the change of their account balance over time, when that happened, and what caused it. With some amount of additional parsing, metadata such as the transaction type and the other parties involved can also be shared.

Events can be queried by the events by handle url: `https://{rest_api_server}/accounts/{address}/events/0x1::coin::CoinStore<0x1::aptos_coin::AptosCoin>/withdraw_events`

```
[
  {
    "version":"13629679",
    "key": "0x0200000000000000cb2f940705c44ba110cd3b4f6540c96f2634938bd5f2aabd6946abf12ed88457",
    "sequence_number": "0",
    "type": "0x1::coin::WithdrawEvent",
    "data": {
      "amount": "1000"
    }
  }
]
```

More information can be gathered from the transaction that generated the event. To do so, query: `https://{rest_server_api}/transactions/by_version/{version}`, where `{version}` is the same value as the `{version}` in the event query.

```
{
  "version": "13629679",
  "gas_used": "4",
  "success": true,
  "vm_status": "Executed successfully",
  "changes": [
    ...
  ],
  "sender": "0x810026ca8291dd88b5b30a1d3ca2edd683d33d06c4a7f7c451d96f6d47bc5e8b",
  "sequence_number": "0",
  "max_gas_amount": "2000",
  "gas_unit_price": "1",
  "expiration_timestamp_secs": "1660616127",
  "payload": {
    "function": "0x1::coin::transfer",
    "type_arguments": [
      "0x1::aptos_coin::AptosCoin"
    ],
    "arguments": [
      "0x5098df8e7969b58ab3bd2d440c6203f64c60a1fd5c08b9d4abe6ae4216246c3e",
      "1000"
    ],
    "type": "script_function_payload"
  },
  "events": [
    {
      "key": "0x0200000000000000810026ca8291dd88b5b30a1d3ca2edd683d33d06c4a7f7c451d96f6d47bc5e8b",
      "sequence_number": "0",
      "type": "0x1::coin::WithdrawEvent",
      "data": {
        "amount": "1000"
      }
    },
    {
      "key": "0x01000000000000005098df8e7969b58ab3bd2d440c6203f64c60a1fd5c08b9d4abe6ae4216246c3e",
      "sequence_number": "0",
      "type": "0x1::coin::DepositEvent",
      "data": {
        "amount": "1000"
      }
    }
  ],
  "timestamp": "1660615531147935",
  "type": "user_transaction"
}
```

The transaction in turn provides other important information for generating a complete perspective on what caused the event. For example, this gives the `timestamp` for when the transaction was executed, other related events, and the `gas_used`. Transactions contain all relevant information to present to the user including the events, time executed, and changes made.

:::tip

When tracking full movement of coins, normally events are sufficient. `0x1::aptos_coin::AptosCoin`, however, requires also look at the `gas_used` for each transaction sent from the given account. To reduce unnecessary overheads, extracting gas fees due to transactions does not emit an event. All transactions for an account can be retrieved from this API: `https://{rest_server_api}/accounts/{address}/transactions`.

:::

To create some sample data to explore see ["Your first transaction"](../tutorials/your-first-transaction).

To learn more about coin creation see ["Your First Coin"](../tutorials/your-first-coin).
