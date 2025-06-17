//! OFAC (Office of Foreign Assets Control) compliance filtering for transactions.
//! 
//! This module provides functionality to detect and filter transactions that involve
//! addresses subject to OFAC sanctions. This is critical for regulatory compliance
//! in jurisdictions where operators must block transactions involving sanctioned entities.
//! 
//! The filtering supports both:
//! - **Static addresses**: Directly specified in transaction account lists
//! - **Dynamic addresses**: Referenced through Solana address lookup tables
//! 
//! When a transaction is identified as OFAC-related, it should be dropped before
//! processing to ensure compliance with financial regulations.

use std::collections::HashSet;

use dashmap::DashMap;
use solana_sdk::{
    address_lookup_table::AddressLookupTableAccount, pubkey::Pubkey,
    transaction::VersionedTransaction,
};

/// Determines if a transaction involves any OFAC-sanctioned addresses.
/// 
/// This function performs comprehensive scanning of both static account keys
/// and dynamic addresses referenced through lookup tables. A transaction is
/// considered OFAC-related if it involves a sanctioned address in any capacity:
/// - As a signer, writable account, or readonly account
/// - Referenced through address lookup tables
/// - As a program ID or fee payer
/// 
/// # Arguments
/// * `tx` - The versioned transaction to analyze
/// * `ofac_addresses` - Set of known OFAC-sanctioned public keys
/// * `address_lookup_table_cache` - Cache of address lookup tables for dynamic address resolution
/// 
/// # Returns
/// `true` if the transaction involves any sanctioned addresses, `false` otherwise
/// 
/// # Compliance Note
/// Operators in regulated jurisdictions should drop transactions that return `true`
/// to maintain compliance with OFAC sanctions programs.
pub fn is_tx_ofac_related(
    tx: &VersionedTransaction,
    ofac_addresses: &HashSet<Pubkey>,
    address_lookup_table_cache: &DashMap<Pubkey, AddressLookupTableAccount>,
) -> bool {
    is_ofac_address_in_static_keys(tx, ofac_addresses)
        || is_ofac_address_in_lookup_table(tx, ofac_addresses, address_lookup_table_cache)
}

/// Checks if any OFAC-sanctioned addresses appear in the transaction's static account keys.
/// 
/// Static account keys include:
/// - Fee payer (always index 0)
/// - All signers
/// - All writable accounts
/// - All readonly accounts
/// - Program IDs
/// 
/// # Arguments
/// * `tx` - The versioned transaction to check
/// * `ofac_addresses` - Set of known OFAC-sanctioned public keys
/// 
/// # Returns
/// `true` if any static account key matches a sanctioned address
fn is_ofac_address_in_static_keys(
    tx: &VersionedTransaction,
    ofac_addresses: &HashSet<Pubkey>,
) -> bool {
    tx.message
        .static_account_keys()
        .iter()
        .any(|acc| ofac_addresses.contains(acc))
}

/// Checks if any OFAC-sanctioned addresses are referenced through address lookup tables.
/// 
/// Solana's address lookup tables allow transactions to reference accounts indirectly
/// to reduce transaction size. This function resolves those references and checks
/// if any resolved addresses are sanctioned.
/// 
/// Only addresses that are actually referenced by the transaction (through writable_indexes
/// or readonly_indexes) are checked - addresses that exist in the lookup table but
/// aren't used by the transaction are ignored.
/// 
/// # Arguments
/// * `tx` - The versioned transaction to check
/// * `ofac_addresses` - Set of known OFAC-sanctioned public keys  
/// * `address_lookup_table_cache` - Cache containing lookup table data
/// 
/// # Returns
/// `true` if any referenced lookup table address matches a sanctioned address
fn is_ofac_address_in_lookup_table(
    tx: &VersionedTransaction,
    ofac_addresses: &HashSet<Pubkey>,
    address_lookup_table_cache: &DashMap<Pubkey, AddressLookupTableAccount>,
) -> bool {
    // Check if transaction uses any address lookup tables
    if let Some(lookup_tables) = tx.message.address_table_lookups() {
        for table in lookup_tables {
            // Resolve the lookup table from cache
            if let Some(lookup_info) = address_lookup_table_cache.get(&table.account_key) {
                // Check both writable and readonly referenced addresses
                for idx in table
                    .writable_indexes
                    .iter()
                    .chain(table.readonly_indexes.iter())
                {
                    // Resolve the index to an actual address
                    if let Some(account) = lookup_info.addresses.get(*idx as usize) {
                        if ofac_addresses.contains(account) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use dashmap::DashMap;
    use solana_sdk::{
        address_lookup_table::AddressLookupTableAccount,
        hash::Hash,
        instruction::{AccountMeta, CompiledInstruction, Instruction},
        message::{v0, v0::MessageAddressTableLookup, MessageHeader, VersionedMessage},
        packet::Packet,
        pubkey::Pubkey,
        signature::Signer,
        signer::keypair::Keypair,
        transaction::{Transaction, VersionedTransaction},
    };

    use crate::ofac::{
        is_ofac_address_in_lookup_table, is_ofac_address_in_static_keys, is_tx_ofac_related,
    };

    #[test]
    fn test_is_ofac_address_in_static_keys() {
        let ofac_signer = Keypair::new();
        let ofac_pubkey = ofac_signer.pubkey();
        let ofac_addresses: HashSet<Pubkey> = HashSet::from_iter([ofac_pubkey]);

        let payer = Keypair::new();

        // random address passes
        let tx = Transaction::new_signed_with_payer(
            &[Instruction::new_with_bytes(
                Pubkey::new_unique(),
                &[0],
                vec![AccountMeta {
                    pubkey: Pubkey::new_unique(),
                    is_signer: false,
                    is_writable: false,
                }],
            )],
            Some(&payer.pubkey()),
            &[&payer],
            Hash::default(),
        );
        let tx = VersionedTransaction::from(tx);
        assert!(!is_ofac_address_in_static_keys(&tx, &ofac_addresses));

        // transaction with ofac account as writable
        let tx = Transaction::new_signed_with_payer(
            &[Instruction::new_with_bytes(
                Pubkey::new_unique(),
                &[0],
                vec![AccountMeta {
                    pubkey: ofac_pubkey,
                    is_signer: false,
                    is_writable: true,
                }],
            )],
            Some(&payer.pubkey()),
            &[&payer],
            Hash::default(),
        );
        let tx = VersionedTransaction::from(tx);
        assert!(is_ofac_address_in_static_keys(&tx, &ofac_addresses));

        // transaction with ofac account as readonly
        let tx = Transaction::new_signed_with_payer(
            &[Instruction::new_with_bytes(
                Pubkey::new_unique(),
                &[0],
                vec![AccountMeta {
                    pubkey: ofac_pubkey,
                    is_signer: false,
                    is_writable: false,
                }],
            )],
            Some(&payer.pubkey()),
            &[&payer],
            Hash::default(),
        );
        let tx = VersionedTransaction::from(tx);

        assert!(is_ofac_address_in_static_keys(&tx, &ofac_addresses));

        // transaction with ofac account as signer
        let tx = Transaction::new_signed_with_payer(
            &[Instruction::new_with_bytes(
                Pubkey::new_unique(),
                &[0],
                vec![AccountMeta {
                    pubkey: Pubkey::new_unique(),
                    is_signer: false,
                    is_writable: true,
                }],
            )],
            Some(&ofac_signer.pubkey()),
            &[&ofac_signer],
            Hash::default(),
        );
        let tx = VersionedTransaction::from(tx);
        assert!(is_ofac_address_in_static_keys(&tx, &ofac_addresses));
    }

    #[test]
    fn test_is_ofac_address_in_lookup_table() {
        let ofac_pubkey = Pubkey::new_unique();
        let ofac_addresses: HashSet<Pubkey> = HashSet::from_iter([ofac_pubkey]);

        let payer = Keypair::new();

        let lookup_table_pubkey = Pubkey::new_unique();
        let lookup_table = AddressLookupTableAccount {
            key: lookup_table_pubkey,
            addresses: vec![ofac_pubkey, Pubkey::new_unique()],
        };

        let address_lookup_table_cache = DashMap::from_iter([(lookup_table_pubkey, lookup_table)]);

        // test read-only ofac address
        let message = VersionedMessage::V0(v0::Message {
            header: MessageHeader {
                num_required_signatures: 1,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 0,
            },
            recent_blockhash: Hash::new_unique(),
            account_keys: vec![payer.pubkey(), Pubkey::new_unique()],
            address_table_lookups: vec![MessageAddressTableLookup {
                account_key: lookup_table_pubkey,
                writable_indexes: vec![],
                readonly_indexes: vec![0],
            }],
            instructions: vec![CompiledInstruction {
                program_id_index: 1,
                accounts: vec![0],
                data: vec![],
            }],
        });
        let tx = VersionedTransaction::try_new(message, &[&payer]).expect("valid tx");

        assert!(is_ofac_address_in_lookup_table(
            &tx,
            &ofac_addresses,
            &address_lookup_table_cache
        ));

        // test writeable ofac
        let message = VersionedMessage::V0(v0::Message {
            header: MessageHeader {
                num_required_signatures: 1,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 0,
            },
            recent_blockhash: Hash::new_unique(),
            account_keys: vec![payer.pubkey(), Pubkey::new_unique()],
            address_table_lookups: vec![MessageAddressTableLookup {
                account_key: lookup_table_pubkey,
                writable_indexes: vec![0],
                readonly_indexes: vec![],
            }],
            instructions: vec![CompiledInstruction {
                program_id_index: 1,
                accounts: vec![0],
                data: vec![],
            }],
        });
        let tx = VersionedTransaction::try_new(message, &[&payer]).expect("valid tx");
        assert!(is_ofac_address_in_lookup_table(
            &tx,
            &ofac_addresses,
            &address_lookup_table_cache
        ));

        // test proximate ofac (in same lookup table, but not referenced)
        let message = VersionedMessage::V0(v0::Message {
            header: MessageHeader {
                num_required_signatures: 1,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 0,
            },
            recent_blockhash: Hash::new_unique(),
            account_keys: vec![payer.pubkey(), Pubkey::new_unique()],
            address_table_lookups: vec![MessageAddressTableLookup {
                account_key: lookup_table_pubkey,
                writable_indexes: vec![1],
                readonly_indexes: vec![],
            }],
            instructions: vec![CompiledInstruction {
                program_id_index: 1,
                accounts: vec![1],
                data: vec![],
            }],
        });
        let tx = VersionedTransaction::try_new(message, &[&payer]).expect("valid tx");
        assert!(!is_ofac_address_in_lookup_table(
            &tx,
            &ofac_addresses,
            &address_lookup_table_cache
        ));
    }

    #[test]
    fn test_discard_ofac_packets() {
        let ofac_pubkey = Pubkey::new_unique();
        let ofac_addresses: HashSet<Pubkey> = HashSet::from_iter([ofac_pubkey]);

        let address_lookup_table_cache = DashMap::new();

        let payer = Keypair::new();

        // random address packet
        let random_tx = Transaction::new_signed_with_payer(
            &[Instruction::new_with_bytes(
                Pubkey::new_unique(),
                &[0],
                vec![AccountMeta {
                    pubkey: Pubkey::new_unique(),
                    is_signer: false,
                    is_writable: false,
                }],
            )],
            Some(&payer.pubkey()),
            &[&payer],
            Hash::default(),
        );
        let random_tx = VersionedTransaction::from(random_tx);
        let random_packet = Packet::from_data(None, random_tx).expect("can create packet");

        let ofac_tx = Transaction::new_signed_with_payer(
            &[Instruction::new_with_bytes(
                Pubkey::new_unique(),
                &[0],
                vec![AccountMeta {
                    pubkey: ofac_pubkey,
                    is_signer: false,
                    is_writable: true,
                }],
            )],
            Some(&payer.pubkey()),
            &[&payer],
            Hash::default(),
        );
        let ofac_tx = VersionedTransaction::from(ofac_tx);
        let ofac_packet = Packet::from_data(None, ofac_tx).expect("can create packet");

        assert!(!is_tx_ofac_related(
            &random_packet.deserialize_slice(..).unwrap(),
            &ofac_addresses,
            &address_lookup_table_cache
        ));
        assert!(is_tx_ofac_related(
            &ofac_packet.deserialize_slice(..).unwrap(),
            &ofac_addresses,
            &address_lookup_table_cache
        ));
    }
}
