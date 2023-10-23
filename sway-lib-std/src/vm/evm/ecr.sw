//! Helper functions to verify EVM signatures.
library;

use ::b512::B512;
use ::registers::error;
use ::ecr::{ec_recover, EcRecoverError};
use ::hash::*;
use ::result::Result::{self, *};
use ::vm::evm::evm_address::EvmAddress;

/// Recover the EVM address derived from the private key used to sign a message.
/// Returns a `Result` to let the caller choose an error handling strategy.
///
/// # Arguments
///
/// * `signature`: [B512] - The signature generated by signing a message hash.
/// * `msg_hash`: [b256] - The signed data.
///
/// # Returns
///
/// * [Result<EvmAddress, EcRecoverError>] - The recovered evm address or an error.
///
/// # Examples
///
/// ```sway
/// use std::{vm::evm::{evm_address::EvmAddress, ecr::ec_recover_evm_address}, b512::B512};
///
/// fn foo() {
///     let hi = 0xbd0c9b8792876713afa8bff383eebf31c43437823ed761cc3600d0016de5110c;
///     let lo = 0x44ac566bd156b4fc71a4a4cb2655d3dd360c695edb17dc3b64d611e122fea23d;
///     let msg_hash = 0xee45573606c96c98ba970ff7cf9511f1b8b25e6bcd52ced30b89df1e4a9c4323;
///     let evm_address = EvmAddress::from(0x7AAE2D980BE4C3275C72CE5B527FA23FFB97B766966559DD062E2B78FD9D3766);
///     let signature: B512 = B512::from((hi, lo));
///     // A recovered evm address.
///     let result_address = ec_recover_evm_address(signature, msg_hash).unwrap();
///     assert(result_address == evm_address);
/// }
/// ```
pub fn ec_recover_evm_address(
    signature: B512,
    msg_hash: b256,
) -> Result<EvmAddress, EcRecoverError> {
    let pub_key_result = ec_recover(signature, msg_hash);

    match pub_key_result {
        Result::Err(e) => Result::Err(e),
        _ => {
            let pub_key = pub_key_result.unwrap();
            // Note that EVM addresses are derived from the Keccak256 hash of the pubkey (not sha256)
            let pubkey_hash = keccak256(((pub_key.bytes)[0], (pub_key.bytes)[1]));
            Ok(EvmAddress::from(pubkey_hash))
        }
    }
}
