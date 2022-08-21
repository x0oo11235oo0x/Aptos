---
title: "Getting Started"
slug: "getting-started"
---

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';

# Getting Started

To kick-start your journey as a developer in the Aptos ecosystem, set up your development environment as described in this section.

### Clone the Aptos-core repo

Start by cloning the `aptos-core` GitHub repo from [GitHub](https://github.com/aptos-labs/aptos-core).

1. Clone the Aptos repo.

      ```
      git clone https://github.com/aptos-labs/aptos-core.git
      ```

2. `cd` into `aptos-core` directory.

    ```
    cd aptos-core
    ```

3. Run the `scripts/dev_setup.sh` Bash script as shown below. This will prepare your developer environment.

    ```
    ./scripts/dev_setup.sh
    ```

4. Update your current shell environment.

    ```
    source ~/.cargo/env
    ```
5. Skip this step if you are not installing an Aptos node.

    <Tabs>
    <TabItem value="devnet" label="Devnet" default>

    Checkout the `devnet` branch using:

    ```
    git checkout --track origin/devnet
    ```
    </TabItem>
    <TabItem value="testnet" label="Testnet" default>

    Checkout the `testnet` branch using:

    ```
    git checkout --track origin/testnet
    ```
    </TabItem>
    </Tabs>


Now your basic Aptos development environment is ready.

### Aptos devnet

- [REST API Open API Spec](https://fullnode.devnet.aptoslabs.com/v1/spec#/).
- **REST service:** [https://fullnode.devnet.aptoslabs.com/v1](https://fullnode.devnet.aptoslabs.com/v1).
- **Faucet service:** [https://faucet.devnet.aptoslabs.com](https://faucet.devnet.aptoslabs.com).
- [Genesis](https://devnet.aptoslabs.com/genesis.blob).
- [Waypoint](https://devnet.aptoslabs.com/waypoint.txt).
- [ChainID](http://fullnode.devnet.aptoslabs.com/).

### IDE plugins for Move

- [Syntax hightlighting for Visual Studio Code](https://marketplace.visualstudio.com/items?itemName=damirka.move-syntax).
- [Move analyzer for Visual Studio Code](https://marketplace.visualstudio.com/items?itemName=move.move-analyzer) - supports advanced code navigation and syntax highlighting.
- [Move language plugin for Jetbrains IDEs](https://plugins.jetbrains.com/plugin/14721-move-language) - supports syntax highlighting, code navigation, renames, formatting, type checks and code generation.

### Aptos CLI

- [Aptos CLI releases](https://github.com/aptos-labs/aptos-core/releases?q=cli&expanded=true).
- [Aptos CLI Documentation](/cli-tools/aptos-cli-tool/aptos-cli-index).

### Aptos SDK

- [Typescript](https://www.npmjs.com/package/aptos).
- [Python](https://pypi.org/project/aptos-sdk/).

### Aptos explorer

- [Aptos Exporer](https://explorer.devnet.aptos.dev/). Use the top-right drop-down menu to select the network.
