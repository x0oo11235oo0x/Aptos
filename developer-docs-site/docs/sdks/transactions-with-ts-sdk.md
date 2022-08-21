---
title: "Transactions with Typescript SDK"
slug: "transactions-with-ts-sdk"
---

# Transactions with Typescript SDK

This tutorial shows the steps of creating, signing and submitting a transaction in BCS format using the Aptos Typescript SDK.

## Submitting transactions in BCS vs JSON

**BCS:** Submitting transactions in the BCS format is more secure than submitting in JSON format. In this method you will create the BCS-serialized signing message on the client side. For a conceptual guide on submitting in BCS format, see [Creating a Signed Transaction](../guides/sign-a-transaction.md). The Typescript SDK supports signing and submitting transactions in BCS format.

**JSON:** When you submit the transactions in JSON format, you will use the REST API and rely on the Aptos server to create the signing message. This approach creates a risk that a user signs an unintended transaction faked by a malicious API server. See the tutorial [Your First Transaction](../tutorials/first-transaction.md), for how to submit transactions in JSON format. In addition, the Typescript SDK provides wrappers to significantly reduce the amount of manual work needed to prepare and submit transactions in JSON format.

:::tip

We strongly recommend that you use the BCS format for submitting transactions to the Aptos Blockchain.

:::

## Before you proceed

Before you proceed, install the latest Aptos TS SDK. Go to your project root directory and run:

`npm install aptos`

or

`yarn add aptos`

:::note
See [the source code for this tutorial](https://github.com/aptos-labs/aptos-core/blob/main/ecosystem/typescript/sdk/examples/typescript/bcs_transaction.ts). Although Typescript is used in this tutorial, Aptos TS SDK also works in Javascript projects.
:::

## Step 1: Create accounts

Let’s assume user Alice wants to send 717 test coins to user Bob. We need to create two user accounts first.

```ts
import { AptosClient, AptosAccount, FaucetClient, BCS, TxnBuilderTypes } from "aptos";

// devnet is used here for testing
const NODE_URL = "https://fullnode.devnet.aptoslabs.com";
const FAUCET_URL = "https://faucet.devnet.aptoslabs.com";

const client = new AptosClient(NODE_URL);
const faucetClient = new FaucetClient(NODE_URL, FAUCET_URL);

// Generates key pair for Alice
const alice = new AptosAccount();
// Creates Alice's account and mint 5000 test coins
await faucetClient.fundAccount(alice.address(), 5000);

let resources = await client.getAccountResources(alice.address());
let accountResource = resources.find((r) => r.type === "0x1::coin::CoinStore<0x1::aptos_coin::AptosCoin>");
console.log(`Alice coins: ${(accountResource?.data as any).coin.value}. Should be 5000!`);

// Generates key pair for Bob
const bob = new AptosAccount();
// Creates Bob's account and mint 0 test coins
await faucetClient.fundAccount(bob.address(), 0);

resources = await client.getAccountResources(bob.address());
accountResource = resources.find((r) => r.type === "0x1::coin::CoinStore<0x1::aptos_coin::AptosCoin>");
console.log(`Bob coins: ${(accountResource?.data as any).coin.value}. Should be 0!`);
```

With the above code we created two accounts on Aptos devnet and minted 5000 test coins for the Alice’s account and 0 test coin for the Bob’s account.

## Step 2: Prepare the transaction payload

The Typescript SDK supports three types of transaction payloads:

1. `ScriptFunction`
2. `Script` and
3. `ModuleBundle`.

See [https://aptos-labs.github.io/ts-sdk-doc/classes/TxnBuilderTypes.TransactionPayload.html](https://aptos-labs.github.io/ts-sdk-doc/classes/TxnBuilderTypes.TransactionPayload.html) for details.

The `ScriptFunction` payload is used to invoke an on-chain Move script function. Within `ScriptFunction` payload you can specify the function name and arguments.

The `Script` payload contains the bytecode for the Aptos MoveVM (Move Virtual Machine) to execute. Within the `Script` payload, you can provide the script code in bytes and the arguments to the script.

The `ModuleBundle` payload is used to publish multiple modules at once. Within `ModuleBundle` payload, you can provide the module bytecode.

To transfer coins from Alice’s account to Bob’s account, we need to prepare a `ScriptFunction` payload with a `transfer` function.

```ts
// We need to pass a token type to the `transfer` function.
const token = new TxnBuilderTypes.TypeTagStruct(TxnBuilderTypes.StructTag.fromString("0x1::aptos_coin::AptosCoin"));

const scriptFunctionPayload = new TxnBuilderTypes.TransactionPayloadScriptFunction(
  TxnBuilderTypes.ScriptFunction.natural(
    // Fully qualified module name, `AccountAddress::ModuleName`
    "0x1::coin",
    // Module function
    "transfer",
    // The coin type to transfer
    [token],
    // Arguments for function `transfer`: receiver account address and amount to transfer
    [BCS.bcsToBytes(TxnBuilderTypes.AccountAddress.fromHex(bob.address())), BCS.bcsSerializeUint64(717)],
  ),
);
```

The Move function `transfer` requires a coin type as type argument. The function `transfer` is defined here [https://github.com/aptos-labs/aptos-core/blob/faf4f94260d4716c8a774b3c17f579d203cc4013/aptos-move/framework/aptos-framework/sources/Coin.move#L311](https://github.com/aptos-labs/aptos-core/blob/faf4f94260d4716c8a774b3c17f579d203cc4013/aptos-move/framework/aptos-framework/sources/Coin.move#L311).

In above code snippet, we want to transfer the `AptosCoin` that is defined under account `0x1` and module `AptosCoin`. The fully qualified name for the `AptosCoin` is therefore `0x1::aptos_coin::AptosCoin`.

:::note
All arguments in `ScriptFunction` payload must be BCS serialized. In above code, we serialized Bob’s account address and the amount number to transfer.
:::

## Step 3: Sign and submit the transaction

After assembling a transaction payload, we are ready to create a `RawTransaction` instance that wraps the payload we just created. The `RawTransaction` can then be signed and submitted.

```ts
// Create a raw transaction out of the transaction payload
const rawTxn = await client.generateRawTransaction(alice.address(), scriptFunctionPayload);

// Sign the raw transaction with Alice's private key
const bcsTxn = AptosClient.generateBCSTransaction(alice, rawTxn);
// Submit the transaction
const transactionRes = await client.submitSignedBCSTransaction(bcsTxn);

// Wait for the transaction to finish
await client.waitForTransaction(transactionRes.hash);

resources = await client.getAccountResources(bob.address());
accountResource = resources.find((r) => r.type === "0x1::coin::CoinStore<0x1::aptos_coin::AptosCoin>");
console.log(`Bob coins: ${(accountResource?.data as any).coin.value}. Should be 717!`);
```

## Output

The output after executing:

```tsx
Alice coins: 5000. Should be 5000!
Bob coins: 0. Should be 0!
Bob coins: 717. Should be 717!
```

## Build raw transactions with the ABI transaction builder

To reduce the burden of serializing payload arguments manually, Typescript SDK also provides an ABI (Application Binary Interface) based transaction builder. The ABI includes the information about the Move function signatures. With the help of ABI, the Typescript SDK is able to serialize native JS/TS values. The ABI files are produced while compiling Move packages with the Aptos CLI. To build raw transactions with the ABI transaction builder, you first need to convert the ABI files into hex strings. On linux/Mac, you can do it with below command::

```bash
cat aptos-core/aptos-move/framework/aptos-token/build/AptosToken/abis/token_transfers/offer_script.abi | od -v -t x1 -A n | tr -d ' \n'
```

And then, you can build raw transactions with:

```ts
import { TransactionBuilderABI, HexString } from "aptos";

// You can pass in multiple ABIs
const transactionBuilder = new TransactionBuilderABI([
  new HexString("ABI_HEX_STRING_1").toUint8Array(),
  new HexString("ABI_HEX_STRING_2").toUint8Array(),
]);

const rawTransaction = transactionBuilder.build(
  "0x3::token_transfers::offer_script",
  [],
  [receiver, creator, collectionName, name, property_version, amount],
);
```

After building the raw transactions, you can follow the `Step 3` above to sign and submit the transaction.
