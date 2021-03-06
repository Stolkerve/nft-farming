//! Farmer records a farmer's 
//! * all claimed reward tokens, 
//! * all seeds he staked,
//! * user_rps per farm,
//! and the deposited near amount prepaid as storage fee


use std::collections::HashMap;
use near_sdk::collections::LookupMap;
use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::{env, AccountId, Balance};
use crate::{SeedId, FarmId, RPS, Contract};
use crate::farm::{ContractNFTTokenId, NFTTokenId};
use crate::errors::*;
use crate::utils::{MAX_ACCOUNT_LENGTH, PARAS_SERIES_DELIMETER};
use crate::StorageKeys;
use crate::utils::NFT_DELIMETER;

use near_sdk::collections::UnorderedSet;
use near_sdk::json_types::U128;
use crate::farm_seed::FarmSeed;

/// each entry cost MAX_ACCOUNT_LENGTH bytes,
/// amount: Balance cost 16 bytes
/// each empty hashmap cost 4 bytes
pub const MIN_FARMER_LENGTH: u128 = MAX_ACCOUNT_LENGTH + 16 + 4 * 3;

/// Account deposits information and storage cost.
#[derive(BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "test", derive(Clone))]
pub struct Farmer {
    pub farmer_id: AccountId,
    /// Native NEAR amount sent to this contract.
    /// Used for storage.
    pub amount: Balance,
    /// Amounts of various reward tokens the farmer claimed.
    pub rewards: HashMap<AccountId, Balance>,
    /// Amounts of various seed tokens the farmer staked.
    pub seeds: HashMap<SeedId, Balance>,
    /// record user_last_rps of farms
    pub user_rps: LookupMap<FarmId, RPS>,
    pub rps_count: u32,
    pub nft_seeds: HashMap<SeedId, UnorderedSet<ContractNFTTokenId>>,
}

impl Farmer {

    /// Adds amount to the balance of given token
    pub(crate) fn add_reward(&mut self, token: &AccountId, amount: Balance) {
        if let Some(x) = self.rewards.get_mut(token) {
            *x = *x + amount;
        } else {
            self.rewards.insert(token.clone(), amount);
        }
    }

    /// Subtract from `reward` balance.
    /// if amount == 0, subtract all reward balance.
    /// Panics if `amount` is bigger than the current balance.
    /// return actual subtract amount
    pub(crate) fn sub_reward(&mut self, token: &AccountId, amount: Balance) -> Balance {
        let value = *self.rewards.get(token).expect(ERR21_TOKEN_NOT_REG);
        assert!(value >= amount, "{}", ERR22_NOT_ENOUGH_TOKENS);
        if amount == 0 {
            self.rewards.remove(&token.clone());
            value
        } else {
            self.rewards.insert(token.clone(), value - amount);
            amount
        }
    }

    pub fn add_seed(&mut self, seed_id: &SeedId, amount: Balance) {
        if amount > 0 {
            self.seeds.insert(
                seed_id.clone(), 
                amount + self.seeds.get(seed_id).unwrap_or(&0_u128)
            );
        }
        
    }

    /// return seed remained.
    pub fn sub_seed(&mut self, seed_id: &SeedId, amount: Balance) -> Balance {
        let prev_balance = self.seeds.get(seed_id).expect(&format!("{}", ERR31_SEED_NOT_EXIST));
        assert!(prev_balance >= &amount, "{}", ERR32_NOT_ENOUGH_SEED);
        let cur_balance = prev_balance - amount;
        if cur_balance > 0 {
            self.seeds.insert(seed_id.clone(), cur_balance);
        } else {
            self.seeds.remove(seed_id);
        }
        cur_balance
    }

    pub fn get_rps(&self, farm_id: &FarmId) -> RPS {
        self.user_rps.get(farm_id).unwrap_or(RPS::default()).clone()
    }

    pub fn set_rps(&mut self, farm_id: &FarmId, rps: RPS) {
        if !self.user_rps.contains_key(farm_id) {
            self.rps_count += 1;
        } 
        self.user_rps.insert(farm_id, &rps);
    }

    pub fn remove_rps(&mut self, farm_id: &FarmId) {
        if self.user_rps.contains_key(farm_id) {
            self.user_rps.remove(farm_id);
            self.rps_count -= 1;
        }
    }

    /// Returns amount of yocto near necessary to cover storage used by this data structure.
    pub fn storage_usage(&self) -> Balance {
        (
            MIN_FARMER_LENGTH 
            + self.rewards.len() as u128 * (4 + MAX_ACCOUNT_LENGTH + 16)
            + self.seeds.len() as u128 * (4 + MAX_ACCOUNT_LENGTH + 16)
            + self.rps_count as u128 * (4 + 1 + 2 * MAX_ACCOUNT_LENGTH + 32)
        )
        * env::storage_byte_cost()
    }

    pub fn add_nft(&mut self, seed_id: &SeedId, contract_nft_token_id: ContractNFTTokenId) {
        if let Some(nft_contract_seed) = self.nft_seeds.get_mut(seed_id) {
            nft_contract_seed.insert(&contract_nft_token_id);
        } else {
            let mut new_nft_contract_seeds = UnorderedSet::new(StorageKeys::AccountSeedId {
                account_seed_id: format!("{}:{}", self.farmer_id, seed_id)
            });
            new_nft_contract_seeds.insert(&contract_nft_token_id);
            self.nft_seeds.insert(seed_id.clone(), new_nft_contract_seeds);
        }
    }

    pub fn sub_nft(&mut self, seed_id: &SeedId, contract_nft_token_id: ContractNFTTokenId ) -> Option<ContractNFTTokenId> {
        let mut nft_token_id_exist: bool = false;
        if let Some(nft_contract_seed) = self.nft_seeds.get_mut(seed_id) {
            nft_token_id_exist = nft_contract_seed.remove(&contract_nft_token_id);
        }
        if nft_token_id_exist {
            Some(contract_nft_token_id)
        } else {
            None
        }
    }
}


/// Versioned Farmer, used for lazy upgrade.
/// Which means this structure would upgrade automatically when used.
/// To achieve that, each time the new version comes in, 
/// each function of this enum should be carefully re-code!
#[derive(BorshSerialize, BorshDeserialize)]
pub enum VersionedFarmer {
    V101(Farmer),
}

impl VersionedFarmer {

    pub fn new(farmer_id: AccountId, amount: Balance) -> Self {
        VersionedFarmer::V101(Farmer {
            farmer_id: farmer_id.clone(),
            amount: amount,
            rewards: HashMap::new(),
            seeds: HashMap::new(),
            user_rps: LookupMap::new(StorageKeys::UserRps {
                account_id: farmer_id.clone(),
            }),
            rps_count: 0,
            nft_seeds: HashMap::new(),
        })
    }

    /// Upgrades from other versions to the currently used version.
    pub fn upgrade(self) -> Self {
        match self {
            VersionedFarmer::V101(farmer) => VersionedFarmer::V101(farmer),
        }
    }

    #[inline]
    #[allow(unreachable_patterns)]
    pub fn need_upgrade(&self) -> bool {
        match self {
            VersionedFarmer::V101(_) => false,
            _ => true,
        }
    }

    #[inline]
    #[allow(unreachable_patterns)]
    pub fn get_ref(&self) -> &Farmer {
        match self {
            VersionedFarmer::V101(farmer) => farmer,
            _ => unimplemented!(),
        }
    }

    #[inline]
    #[allow(unreachable_patterns)]
    pub fn get(self) -> Farmer {
        match self {
            VersionedFarmer::V101(farmer) => farmer,
            _ => unimplemented!(),
        }
    }

    #[inline]
    #[allow(unreachable_patterns)]
    pub fn get_ref_mut(&mut self) -> &mut Farmer {
        match self {
            VersionedFarmer::V101(farmer) => farmer,
            _ => unimplemented!(),
        }
    }
}
