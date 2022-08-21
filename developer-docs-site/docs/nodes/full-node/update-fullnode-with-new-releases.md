---
title: "Update FullNode With New Releases"
slug: "update-fullnode-with-new-releases"
sidebar_position: 11
---

# Update FullNode With New Releases

When `devnet` is wiped and updated with newer versions, you will need to update your FullNode as well. If you do not, your FullNode will not continue to synchronize with the network. To update your FullNode, follow these steps:

## If you built the FullNode from aptos-core source code

1. Shutdown your FullNode.

2. Delete the data folder (the directory path is what you specified in the configuration file, e.g., `fullnode.yaml`).

    - The default data folder is `/opt/aptos/data`.

3. Delete the `genesis.blob` file and `waypoint.txt` file (depending on how you configured it, you might not have this file and may instead have a `waypoint` directly in your configuration file).

4. If you use the Rust binary, pull the latest of `devnet` via `git checkout devnet && git pull`, and build the binary again.

5. Download the new [genesis.blob][devnet_genesis] file and the new [waypoint][devnet_waypoint].

6. Update the configuration file (e.g., `fullnode.yaml`) with the new waypoint (if you configure the waypoint directly there).

7. Restart the FullNode.

8. See the [Verify initial synchronization](/nodes/full-node/fullnode-source-code-or-docker#verify-initial-synchronization) section for checking if the FullNode is syncing again.

## If you run a FullNode via Docker

1. Shutdown your FullNode
2. Delete the entire directory which holds your FullNode config and data directory.
3. Rerun the instructions on [Approach #2: Using Docker](fullnode-source-code-or-docker.md#Approach-#2:-Using-Docker)

[rest_spec]: https://github.com/aptos-labs/aptos-core/tree/main/api
[devnet_genesis]: https://devnet.aptoslabs.com/genesis.blob
[devnet_waypoint]: https://devnet.aptoslabs.com/waypoint.txt
[aptos-labs/aptos-core]: https://github.com/aptos-labs/aptos-core.git
[status dashboard]: https://status.devnet.aptos.dev
