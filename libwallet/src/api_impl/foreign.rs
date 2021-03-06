// Copyright 2019 The Grin Developers
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Generic implementation of owner API functions
use strum::IntoEnumIterator;

use crate::api_impl::owner::check_ttl;
use crate::grin_keychain::Keychain;
use crate::grin_util::secp::key::SecretKey;
use crate::internal::{tx, updater};
use crate::slate_versions::SlateVersion;
use crate::{
	address, BlockFees, CbData, Error, ErrorKind, NodeClient, Slate, TxLogEntryType, VersionInfo,
	WalletBackend,
};

const FOREIGN_API_VERSION: u16 = 2;
const USER_MESSAGE_MAX_LEN: usize = 256;

/// Return the version info
pub fn check_version() -> VersionInfo {
	VersionInfo {
		foreign_api_version: FOREIGN_API_VERSION,
		supported_slate_versions: SlateVersion::iter().collect(),
	}
}

/// Build a coinbase transaction
pub fn build_coinbase<'a, T: ?Sized, C, K>(
	w: &mut T,
	keychain_mask: Option<&SecretKey>,
	block_fees: &BlockFees,
	test_mode: bool,
) -> Result<CbData, Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	updater::build_coinbase(&mut *w, keychain_mask, block_fees, test_mode)
}

/// verify slate messages
pub fn verify_slate_messages(slate: &Slate) -> Result<(), Error> {
	slate.verify_messages()
}

/// Receive a tx as recipient
/// Note: key_id & output_amounts needed for secure claims, mwc713.
pub fn receive_tx<'a, T: ?Sized, C, K>(
	w: &mut T,
	keychain_mask: Option<&SecretKey>,
	slate: &Slate,
	address: Option<String>,
	key_id_opt: Option<&str>,
	output_amounts: Option<Vec<u64>>,
	dest_acct_name: Option<&str>,
	message: Option<String>,
	use_test_rng: bool,
) -> Result<Slate, Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	let mut ret_slate = slate.clone();
	check_ttl(w, &ret_slate)?;
	let parent_key_id = match dest_acct_name {
		Some(d) => {
			let pm = w.get_acct_path(d.to_owned())?;
			match pm {
				Some(p) => p.path,
				None => w.parent_key_id(),
			}
		}
		None => w.parent_key_id(),
	};
	// Don't do this multiple times
	let tx = updater::retrieve_txs(
		&mut *w,
		keychain_mask,
		None,
		Some(ret_slate.id),
		Some(&parent_key_id),
		use_test_rng,
		None,
		None,
	)?;
	for t in &tx {
		if t.tx_type == TxLogEntryType::TxReceived {
			return Err(ErrorKind::TransactionAlreadyReceived(ret_slate.id.to_string()).into());
		}
	}

	let message = match message {
		Some(mut m) => {
			m.truncate(USER_MESSAGE_MAX_LEN);
			Some(m)
		}
		None => None,
	};

	let num_outputs = match &output_amounts {
		Some(v) => v.len(),
		None => 1,
	};

	// Note: key_id & output_amounts needed for secure claims, mwc713.
	tx::add_output_to_slate(
		&mut *w,
		keychain_mask,
		&mut ret_slate,
		address,
		key_id_opt,
		output_amounts,
		&parent_key_id,
		1,
		message,
		false,
		use_test_rng,
		num_outputs,
	)?;
	tx::update_message(&mut *w, keychain_mask, &mut ret_slate)?;

	let keychain = w.keychain(keychain_mask)?;
	let excess = ret_slate.calc_excess(&keychain)?;

	if let Some(ref mut p) = ret_slate.payment_proof {
		let sig = tx::create_payment_proof_signature(
			ret_slate.amount,
			&excess,
			p.sender_address,
			address::address_from_derivation_path(&keychain, &parent_key_id, 0)?,
		)?;

		p.receiver_signature = Some(sig);
	}

	Ok(ret_slate)
}

/// Receive an tx that this wallet has issued
pub fn finalize_invoice_tx<'a, T: ?Sized, C, K>(
	w: &mut T,
	keychain_mask: Option<&SecretKey>,
	slate: &Slate,
) -> Result<Slate, Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	let mut sl = slate.clone();
	check_ttl(w, &sl)?;
	// Participant id 0 for mwc713 compatibility
	let context = w.get_private_context(keychain_mask, sl.id.as_bytes(), 0)?;
	// Participant id 0 for mwc713 compatibility
	tx::complete_tx(&mut *w, keychain_mask, &mut sl, 0, &context)?;
	tx::update_stored_tx(&mut *w, keychain_mask, &context, &mut sl, true)?;
	tx::update_message(&mut *w, keychain_mask, &mut sl)?;
	{
		let mut batch = w.batch(keychain_mask)?;
		// Participant id 0 for mwc713 compatibility
		batch.delete_private_context(sl.id.as_bytes(), 0)?;
		batch.commit()?;
	}
	Ok(sl)
}
