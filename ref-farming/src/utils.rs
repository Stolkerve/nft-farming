
use near_sdk::json_types::{U128};
use near_sdk::{Balance, env, ext_contract, Gas, Timestamp};
use uint::construct_uint;
use crate::{SeedId, FarmId, NftBalance};
use crate::errors::*;
use crate::farm_seed::{FarmSeed, NFTTokenId};
use crate::farm::ContractNFTTokenId;
use std::collections::HashMap;

pub type TimestampSec = u32;

pub const MIN_SEED_DEPOSIT: u128 = 1_000_000_000_000_000_000;
pub const MAX_ACCOUNT_LENGTH: u128 = 64;
/// Amount of gas for fungible token transfers.
pub const GAS_FOR_FT_TRANSFER: Gas = 10_000_000_000_000;
pub const GAS_FOR_NFT_TRANSFER: Gas = 50_000_000_000_000;

pub const GAS_FOR_RESOLVE_TRANSFER: Gas = 50_000_000_000_000;
pub const MFT_TAG: &str = "@";
pub const FT_INDEX_TAG: &str = "$";
pub const NFT_DELIMETER: &str = "@";
pub const PARAS_SERIES_DELIMETER: &str = ":";


construct_uint! {
    /// 256-bit unsigned integer.
    pub struct U256(4);
}

/// TODO: this should be in the near_standard_contracts
#[ext_contract(ext_fungible_token)]
pub trait FungibleToken {
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>);
}

#[ext_contract(ext_non_fungible_token)]
pub trait NonFungibleToken {
    fn nft_transfer(
        &mut self,
        receiver_id: String,
        token_id: String,
        approval_id: Option<u64>,
        memo: Option<String>,
    );
}

#[ext_contract(ext_self)]
pub trait TokenPostActions {
    fn callback_post_withdraw_reward(
        &mut self,
        token_id: AccountId,
        sender_id: AccountId,
        amount: U128,
    );

    fn callback_post_withdraw_ft_seed(
        &mut self,
        seed_id: SeedId,
        sender_id: AccountId,
        amount: U128,
    );

    fn callback_post_withdraw_mft_seed(
        &mut self,
        seed_id: SeedId,
        sender_id: AccountId,
        amount: U128,
    );

    fn callback_post_withdraw_nft(
        &mut self,
        seed_id: SeedId,
        sender_id: AccountId,
        nft_contract_id: String,
        nft_token_id: String
    );
}

/// Assert that 1 yoctoNEAR was attached.
pub fn assert_one_yocto() {
    assert_eq!(env::attached_deposit(), 1, "Requires attached deposit of exactly 1 yoctoNEAR")
}

// return receiver_id, token_id
pub fn parse_seed_id(lpt_id: &str) -> (String, String) {
    let v: Vec<&str> = lpt_id.split(MFT_TAG).collect();
    if v.len() == 1 { // receiver_id
        (v[0].to_string(), v[0].to_string())
    } else {
        env::panic(format!("{}", ERR33_INVALID_SEED_ID).as_bytes())
    }
}

pub fn parse_farm_id(farm_id: &FarmId) -> (String, usize) {
    let v: Vec<&str> = farm_id.split("#").collect();
    if v.len() != 2 {
        env::panic(format!("{}", ERR42_INVALID_FARM_ID).as_bytes())
    }
    (v[0].to_string(), v[1].parse::<usize>().unwrap())
}

pub fn gen_farm_id(seed_id: &SeedId, index: usize) -> FarmId {
    format!("{}#{}", seed_id, index)
}

pub(crate) fn to_nano(timestamp: TimestampSec) -> Timestamp {
    Timestamp::from(timestamp) * 10u64.pow(9)
}

pub(crate) fn to_sec(timestamp: Timestamp) -> TimestampSec {
    (timestamp / 10u64.pow(9)) as u32
}

pub fn get_nft_balance_equivalent(
    nft_balance: NftBalance,
    nft_staked: ContractNFTTokenId
) -> Option<Balance> {
    // split x.paras.near@1:1
    // to "x.paras.near@1", ":1"
    let mut result: Option<Balance> = None;

    if let Some(nft_balance_equivalent) = nft_balance.get(&nft_staked.to_string()) {
        result = Some(nft_balance_equivalent.0);
    } else if nft_staked.contains(PARAS_SERIES_DELIMETER) {
        let contract_token_series_id_split: Vec<&str> = nft_staked.split(PARAS_SERIES_DELIMETER).collect();
        if let Some(nft_balance_equivalent) = nft_balance.get(&contract_token_series_id_split[0].to_string()) {
            result = Some(nft_balance_equivalent.0);
        } else {
            let contract_token_series_id_split: Vec<&str> = nft_staked.split(NFT_DELIMETER).collect();
            if let Some(nft_balance_equivalent) = nft_balance.get(&contract_token_series_id_split[0].to_string()) {
                result = Some(nft_balance_equivalent.0);
            } else {
                result = None;
            }
        }
    } else {
        let contract_token_series_id_split: Vec<&str> = nft_staked.split(NFT_DELIMETER).collect();
        if let Some(nft_balance_equivalent) = nft_balance.get(&contract_token_series_id_split[0].to_string()) {
            result = Some(nft_balance_equivalent.0);
        } else {
            result = None;
        }
    }

    return result;
}
