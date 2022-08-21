// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use first_transaction::{Account, RestClient};

//:!:>section_1
pub struct HelloBlockchainClient {
    pub rest_client: RestClient,
}

impl HelloBlockchainClient {
    /// Represents an account as well as the private, public key-pair for the Aptos blockchain.
    pub fn new(url: String) -> Self {
        Self {
            rest_client: RestClient::new(url),
        }
    }

    /// Publish a new module to the blockchain within the specified account
    pub fn publish_module(&self, account_from: &mut Account, module_hex: &str) -> String {
        let payload = serde_json::json!({
            "type": "module_bundle_payload",
            "modules": [{"bytecode": format!("0x{}", module_hex)}],
        });
        self.rest_client
            .execution_transaction_with_payload(account_from, payload)
    }
    //<:!:section_1
    //:!:>section_2
    /// Retrieve the resource Message::MessageHolder::message
    pub fn get_message(&self, contract_address: &str, account_address: &str) -> Option<String> {
        let module_type = format!("0x{}::message::MessageHolder", contract_address);
        self.rest_client
            .account_resource(account_address, &module_type)
            .map(|value| value["data"]["message"].as_str().unwrap().to_string())
    }

    //<:!:section_2
    //:!:>section_3
    /// Potentially initialize and set the resource Message::MessageHolder::message
    pub fn set_message(
        &self,
        contract_address: &str,
        account_from: &mut Account,
        message: &str,
    ) -> String {
        let message_hex = hex::encode(message.as_bytes());
        let payload = serde_json::json!({
            "type": "script_function_payload",
            "function": format!("0x{}::message::set_message", contract_address),
            "type_arguments": [],
            "arguments": [message_hex]
        });
        self.rest_client
            .execution_transaction_with_payload(account_from, payload)
    }
    //<:!:section_3
}
