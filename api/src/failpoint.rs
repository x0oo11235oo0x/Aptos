// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

#![allow(unused_imports)]

use crate::response::InternalError;
use anyhow::{format_err, Result};
use aptos_api_types::{AptosError, AptosErrorCode};
use poem_openapi::payload::Json;

#[allow(unused_variables)]
#[inline]
pub fn fail_point_poem<E: InternalError>(name: &str) -> Result<(), E> {
    Ok(fail::fail_point!(format!("api::{}", name).as_str(), |_| {
        Err(E::internal_with_code_no_info(
            format!("unexpected internal error for {}", name),
            AptosErrorCode::InternalError,
        ))
    }))
}
