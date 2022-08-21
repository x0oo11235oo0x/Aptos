// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use aptos_config::config::PeerRole;
use aptos_crypto::x25519;
use aptos_types::{chain_id::ChainId, PeerId};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct AuthRequest {
    pub chain_id: ChainId,
    pub peer_id: PeerId,
    pub server_public_key: x25519::PublicKey,
    pub handshake_msg: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub struct AuthResponse {
    pub handshake_msg: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Claims {
    pub chain_id: ChainId,
    pub peer_id: PeerId,
    pub peer_role: PeerRole,
    pub epoch: u64,
    pub exp: usize,
    pub iat: usize,
}
