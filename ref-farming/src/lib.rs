/*!
* Ref-Farming
*
* lib.rs is the main entry point.
*/

use std::collections::HashMap;
use std::convert::TryInto;

use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::collections::{LookupMap, UnorderedMap};
use near_sdk::json_types::{ValidAccountId, U128};
use near_sdk::BorshStorageKey;
use near_sdk::{
    assert_one_yocto, env, near_bindgen, AccountId, Balance, PanicOnDefault, Promise, PromiseResult,
};

use crate::farm::{ContractNFTTokenId, Farm, FarmId, RPS};
use crate::farm_seed::SeedType;
use crate::farm_seed::{FarmSeedMetadata, NFTTokenId, NftBalance, SeedId, FarmSeed};
use crate::farmer::{Farmer, VersionedFarmer};
use crate::utils::{
    ext_fungible_token, ext_non_fungible_token, ext_self, gen_farm_id, get_nft_balance_equivalent,
    parse_farm_id, FT_INDEX_TAG, GAS_FOR_FT_TRANSFER, GAS_FOR_NFT_TRANSFER,
    GAS_FOR_RESOLVE_TRANSFER, MIN_SEED_DEPOSIT, NFT_DELIMETER,
};

// for simulator test
use crate::errors::*;
pub use crate::farm::HRFarmTerms;
pub use crate::view::FarmInfo;

mod errors;
mod farm;
mod farm_seed;
mod farmer;
mod internals;
mod storage_impl;
mod token_receiver;
mod utils;

mod view;

mod owner;

near_sdk::setup_alloc!();

#[derive(BorshStorageKey, BorshSerialize)]
pub enum StorageKeys {
    Seed,
    Farm,
    OutdatedFarm,
    Farmer,
    RewardInfo,
    UserRps { account_id: AccountId },
    AccountSeedId { account_seed_id: String },
    NftBalanceSeed,
}

#[derive(BorshDeserialize, BorshSerialize)]
pub struct ContractData {
    // owner of this contract
    owner_id: AccountId,

    // record seeds and the farms under it.
    // seeds: UnorderedMap<SeedId, FarmSeed>,
    seeds: UnorderedMap<SeedId, FarmSeed>,

    // each farmer has a structure to describe
    // farmers: LookupMap<AccountId, Farmer>,
    farmers: LookupMap<AccountId, VersionedFarmer>,

    farms: UnorderedMap<FarmId, Farm>,
    outdated_farms: UnorderedMap<FarmId, Farm>,

    nft_balance_seeds: LookupMap<SeedId, NftBalance>,

    // for statistic
    farmer_count: u64,
    reward_info: UnorderedMap<AccountId, Balance>,
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct Contract {
    data: ContractData,
}

#[near_bindgen]
impl Contract {
    #[init]
    pub fn new(owner_id: ValidAccountId) -> Self {
        assert!(!env::state_exists(), "Already initialized");
        Self {
            data: ContractData {
                owner_id: owner_id.into(),
                farmer_count: 0,
                seeds: UnorderedMap::new(StorageKeys::Seed),
                farmers: LookupMap::new(StorageKeys::Farmer),
                farms: UnorderedMap::new(StorageKeys::Farm),
                outdated_farms: UnorderedMap::new(StorageKeys::OutdatedFarm),
                reward_info: UnorderedMap::new(StorageKeys::RewardInfo),
                nft_balance_seeds: LookupMap::new(StorageKeys::NftBalanceSeed),
            },
        }
    }

    /// create farm and pay for its storage fee
    #[payable]
    pub fn create_simple_farm(
        &mut self,
        terms: HRFarmTerms,
        min_deposit: Option<U128>,
        nft_balance: Option<HashMap<NFTTokenId, U128>>,
        metadata: Option<FarmSeedMetadata>,
    ) -> FarmId {
        self.assert_owner();
        let prev_storage = env::storage_usage();
        let min_deposit: u128 = min_deposit.unwrap_or(U128(MIN_SEED_DEPOSIT)).0;
        let farm_id = self.internal_add_farm(&terms, min_deposit, nft_balance, metadata);
        // Check how much storage cost and refund the left over back.
        let storage_needed = env::storage_usage() - prev_storage;
        let storage_cost = storage_needed as u128 * env::storage_byte_cost();
        assert!(
            storage_cost <= env::attached_deposit(),
            "{}: {}",
            ERR11_INSUFFICIENT_STORAGE,
            storage_needed
        );
        let refund = env::attached_deposit() - storage_cost;
        if refund > 0 {
            Promise::new(env::predecessor_account_id()).transfer(refund);
        }
        farm_id
    }

    /// Clean invalid rps,
    /// return false if the rps is still valid.
    pub fn remove_user_rps_by_farm(&mut self, farm_id: FarmId) -> bool {
        let sender_id = env::predecessor_account_id();
        let mut farmer = self.get_farmer(&sender_id);
        let (seed_id, _) = parse_farm_id(&farm_id);
        let farm_seed = self.get_seed(&seed_id);
        if !farm_seed.get_ref().farms.contains(&farm_id) {
            farmer.get_ref_mut().remove_rps(&farm_id);
            self.data_mut().farmers.insert(&sender_id, &farmer);
            true
        } else {
            false
        }
    }

    pub fn claim_reward_by_farm(&mut self, farm_id: FarmId) {
        let sender_id = env::predecessor_account_id();
        self.internal_claim_user_reward_by_farm_id(&sender_id, &farm_id);
        self.assert_storage_usage(&sender_id);
    }

    pub fn claim_reward_by_seed(&mut self, seed_id: SeedId) {
        let sender_id = env::predecessor_account_id();
        self.internal_claim_user_reward_by_seed_id(&sender_id, &seed_id);
        self.assert_storage_usage(&sender_id);
    }

    #[payable]
    pub fn claim_reward_by_farm_and_withdraw(&mut self, farm_id: FarmId) {
        assert_one_yocto();
        let sender_id = env::predecessor_account_id();
        self.internal_claim_user_reward_by_farm_id(&sender_id, &farm_id);
        self.assert_storage_usage(&sender_id);

        let token_id = self.get_farm(farm_id).unwrap().reward_token;
        self.internal_withdraw_reward(token_id, None);
    }

    #[payable]
    pub fn claim_reward_by_seed_and_withdraw(&mut self, seed_id: SeedId) {
        assert_one_yocto();
        let sender_id = env::predecessor_account_id();
        self.internal_claim_user_reward_by_seed_id(&sender_id, &seed_id);
        self.assert_storage_usage(&sender_id);

        let farmer = self.get_farmer(&sender_id);

        let seed = self.data().seeds.get(&seed_id).unwrap();
        let mut reward_tokens: Vec<AccountId> = vec![];
        for farm_id in seed.get_ref().farms.iter() {
            let reward_token = self.data().farms.get(farm_id).unwrap().get_reward_token();
            if !reward_tokens.contains(&reward_token) {
                if farmer.get_ref().rewards.get(&reward_token).is_some() {
                    self.internal_withdraw_reward(reward_token.clone(), None);
                }
                reward_tokens.push(reward_token);
            }
        }
    }

    /// Withdraws given reward token of given user.
    #[payable]
    pub fn withdraw_reward(&mut self, token_id: ValidAccountId, amount: Option<U128>) {
        assert_one_yocto();

        self.internal_withdraw_reward(token_id.to_string(), amount);
    }

    #[private]
    pub fn private_withdraw_reward(
        &mut self,
        token_id: AccountId,
        sender_id: AccountId,
        amount: Option<U128>,
    ) {
        self.internal_execute_withdraw_reward(token_id, sender_id, amount);
    }

    fn internal_withdraw_reward(&mut self, token_id: AccountId, amount: Option<U128>) {
        let sender_id = env::predecessor_account_id();
        self.internal_execute_withdraw_reward(token_id, sender_id, amount);
    }

    fn internal_execute_withdraw_reward(
        &mut self,
        token_id: AccountId,
        sender_id: AccountId,
        amount: Option<U128>,
    ) {
        let token_id: AccountId = token_id.into();
        let amount: u128 = amount.unwrap_or(U128(0)).into();
        let mut farmer = self.get_farmer(&sender_id);

        // Note: subtraction, will be reverted if the promise fails.
        let amount = farmer.get_ref_mut().sub_reward(&token_id, amount);
        self.data_mut().farmers.insert(&sender_id, &farmer);
        ext_fungible_token::ft_transfer(
            sender_id.clone().try_into().unwrap(),
            amount.into(),
            None,
            &token_id,
            1,
            GAS_FOR_FT_TRANSFER,
        )
        .then(ext_self::callback_post_withdraw_reward(
            token_id,
            sender_id,
            amount.into(),
            &env::current_account_id(),
            0,
            GAS_FOR_RESOLVE_TRANSFER,
        ));
    }

    #[private]
    pub fn callback_post_withdraw_reward(
        &mut self,
        token_id: AccountId,
        sender_id: AccountId,
        amount: U128,
    ) {
        assert_eq!(
            env::promise_results_count(),
            1,
            "{}",
            ERR25_CALLBACK_POST_WITHDRAW_INVALID
        );
        match env::promise_result(0) {
            PromiseResult::NotReady => unreachable!(),
            PromiseResult::Successful(_) => {
                env::log(
                    format!(
                        "{} withdraw reward {} amount {}, Succeed.",
                        sender_id, token_id, amount.0,
                    )
                    .as_bytes(),
                );
            }
            PromiseResult::Failed => {
                env::log(
                    format!(
                        "{} withdraw reward {} amount {}, Callback Failed.",
                        sender_id, token_id, amount.0,
                    )
                    .as_bytes(),
                );
                // This reverts the changes from withdraw function.
                let mut farmer = self.get_farmer(&sender_id);
                farmer.get_ref_mut().add_reward(&token_id, amount.0);
                self.data_mut().farmers.insert(&sender_id, &farmer);
            }
        };
    }

    pub fn force_upgrade_seed(&mut self, seed_id: SeedId) {
        self.assert_owner();
        let seed = self.get_seed_and_upgrade(&seed_id);
        self.data_mut().seeds.insert(&seed_id, &seed);
    }

    #[payable]
    pub fn withdraw_nft(
        &mut self,
        seed_id: SeedId,
        nft_contract_id: String,
        nft_token_id: NFTTokenId,
    ) {
        assert_one_yocto();
        let sender_id = env::predecessor_account_id();

        self.internal_nft_withdraw(&seed_id, &sender_id, &nft_contract_id, &nft_token_id);

        // transfer nft back to the owner
        ext_non_fungible_token::nft_transfer(
            sender_id.clone(),
            nft_token_id.clone(),
            None,
            None,
            &nft_contract_id,
            1,
            GAS_FOR_NFT_TRANSFER,
        )
        .then(ext_self::callback_post_withdraw_nft(
            seed_id,
            sender_id,
            nft_contract_id,
            nft_token_id,
            &env::current_account_id(),
            0,
            GAS_FOR_RESOLVE_TRANSFER,
        ));
    }

    #[payable]
    pub fn withdraw_seed(&mut self, seed_id: SeedId, amount: U128) {
        assert_one_yocto();
        let sender_id = env::predecessor_account_id();

        let seed_contract_id: AccountId = seed_id.split(FT_INDEX_TAG).next().unwrap().to_string();
        let amount: Balance = amount.into();

        // update inner state
        let seed_type = self.internal_seed_withdraw(&seed_id, &sender_id, amount);

        match seed_type {
            SeedType::FT => {
                ext_fungible_token::ft_transfer(
                    sender_id.clone().try_into().unwrap(),
                    amount.into(),
                    None,
                    &seed_contract_id,
                    1, // one yocto near
                    GAS_FOR_FT_TRANSFER,
                )
                .then(ext_self::callback_post_withdraw_ft_seed(
                    seed_id,
                    sender_id,
                    amount.into(),
                    &env::current_account_id(),
                    0,
                    GAS_FOR_RESOLVE_TRANSFER,
                ));
            }
            SeedType::NFT => {
                panic!("Use withdraw_nft for this");
            }
        }
    }

    #[private]
    pub fn callback_post_withdraw_nft(
        &mut self,
        seed_id: SeedId,
        sender_id: AccountId,
        nft_contract_id: String,
        nft_token_id: String,
    ) {
        assert_eq!(
            env::promise_results_count(),
            1,
            "{}",
            ERR25_CALLBACK_POST_WITHDRAW_INVALID
        );

        match env::promise_result(0) {
            PromiseResult::NotReady => unreachable!(),
            PromiseResult::Failed => {
                env::log(
                    format!(
                        "{} withdraw {} nft from {}, Callback failed.",
                        sender_id, nft_token_id, nft_contract_id
                    )
                    .as_bytes(),
                );

                // revert withdraw

                let mut farmer = self.get_farmer(&sender_id);
                let mut farm_seed = self.get_seed(&seed_id);

                let contract_nft_token_id: ContractNFTTokenId =
                    format!("{}{}{}", nft_contract_id, NFT_DELIMETER, nft_token_id);
                let nft_balance = self.data().nft_balance_seeds.get(&seed_id).unwrap();
                if let Some(nft_balance_equivalent) =
                    get_nft_balance_equivalent(nft_balance, contract_nft_token_id.clone())
                {
                    self.internal_claim_user_reward_by_seed_id(&sender_id, &seed_id);

                    farmer
                        .get_ref_mut()
                        .add_nft(&seed_id, contract_nft_token_id);

                    farmer
                        .get_ref_mut()
                        .add_seed(&seed_id, nft_balance_equivalent);
                    self.data_mut().farmers.insert(&sender_id, &farmer);

                    // **** update seed (new version)
                    farm_seed.get_ref_mut().add_amount(nft_balance_equivalent);
                    self.data_mut().seeds.insert(&seed_id, &farm_seed);
                }
            }
            PromiseResult::Successful(_) => {
                env::log(
                    format!(
                        "{} withdraw {} nft from {}, Succeed.",
                        sender_id, nft_token_id, nft_contract_id
                    )
                    .as_bytes(),
                );
            }
        }
    }
    #[private]
    pub fn callback_post_withdraw_ft_seed(
        &mut self,
        seed_id: SeedId,
        sender_id: AccountId,
        amount: U128,
    ) {
        assert_eq!(
            env::promise_results_count(),
            1,
            "{}",
            ERR25_CALLBACK_POST_WITHDRAW_INVALID
        );
        let amount: Balance = amount.into();
        match env::promise_result(0) {
            PromiseResult::NotReady => unreachable!(),
            PromiseResult::Failed => {
                env::log(
                    format!(
                        "{} withdraw {} ft seed with amount {}, Callback Failed.",
                        sender_id, seed_id, amount,
                    )
                    .as_bytes(),
                );
                // revert withdraw, equal to deposit, claim reward to update user reward_per_seed
                self.internal_claim_user_reward_by_seed_id(&sender_id, &seed_id);
                // **** update seed (new version)
                let mut farm_seed = self.get_seed(&seed_id);
                farm_seed.get_ref_mut().add_amount(amount);
                self.data_mut().seeds.insert(&seed_id, &farm_seed);

                let mut farmer = self.get_farmer(&sender_id);
                farmer.get_ref_mut().add_seed(&seed_id, amount);
                self.data_mut().farmers.insert(&sender_id, &farmer);
            }
            PromiseResult::Successful(_) => {
                env::log(
                    format!(
                        "{} withdraw {} ft seed with amount {}, Succeed.",
                        sender_id, seed_id, amount,
                    )
                    .as_bytes(),
                );
            }
        };
    }
}

#[cfg(test)]
mod tests {

    use farm::HRFarmTerms;
    use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
    use near_contract_standards::storage_management::{StorageBalance, StorageManagement};
    use near_sdk::json_types::{ValidAccountId, U128};
    use near_sdk::test_utils::{accounts, VMContextBuilder};
    use near_sdk::{testing_env, Balance, MockedBlockchain};

    use super::utils::*;
    use super::*;

    fn setup_contract() -> (VMContextBuilder, Contract) {
        let mut context = VMContextBuilder::new();
        testing_env!(context.predecessor_account_id(accounts(0)).build());
        let contract = Contract::new(accounts(0));
        (context, contract)
    }

    fn create_farm(
        context: &mut VMContextBuilder,
        contract: &mut Contract,
        seed: ValidAccountId,
        reward: ValidAccountId,
        session_amount: Balance,
        session_interval: u32,
    ) -> FarmId {
        // storage needed: 341
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .attached_deposit(env::storage_byte_cost() * 573)
            .build());
        contract.create_simple_farm(
            HRFarmTerms {
                seed_id: seed.into(),
                reward_token: reward.into(),
                start_at: 0,
                reward_per_session: U128(session_amount),
                session_interval: session_interval,
            },
            Some(U128(10)),
            None,
            None,
        )
    }

    fn deposit_reward(
        context: &mut VMContextBuilder,
        contract: &mut Contract,
        amount: u128,
        time_stamp: u32,
    ) {
        testing_env!(context
            .predecessor_account_id(accounts(2))
            .block_timestamp(to_nano(time_stamp))
            .attached_deposit(1)
            .build());
        contract.ft_on_transfer(accounts(0), U128(amount), String::from("bob#0"));
    }

    fn register_farmer(
        context: &mut VMContextBuilder,
        contract: &mut Contract,
        farmer: ValidAccountId,
    ) -> StorageBalance {
        testing_env!(context
            .predecessor_account_id(farmer.clone())
            .is_view(false)
            .attached_deposit(env::storage_byte_cost() * 1852)
            .build());
        contract.storage_deposit(Some(farmer), Some(true))
    }

    fn storage_withdraw(
        context: &mut VMContextBuilder,
        contract: &mut Contract,
        farmer: ValidAccountId,
    ) -> StorageBalance {
        testing_env!(context
            .predecessor_account_id(farmer.clone())
            .is_view(false)
            .attached_deposit(1)
            .build());
        contract.storage_withdraw(None)
    }

    fn deposit_seed(
        context: &mut VMContextBuilder,
        contract: &mut Contract,
        farmer: ValidAccountId,
        time_stamp: u32,
        amount: Balance,
    ) {
        testing_env!(context
            .predecessor_account_id(accounts(1))
            .is_view(false)
            .block_timestamp(to_nano(time_stamp))
            .attached_deposit(1)
            .build());
        contract.ft_on_transfer(farmer, U128(amount), String::from(""));
    }

    fn withdraw_seed(
        context: &mut VMContextBuilder,
        contract: &mut Contract,
        farmer: ValidAccountId,
        time_stamp: u32,
        amount: Balance,
    ) {
        testing_env!(context
            .predecessor_account_id(farmer)
            .is_view(false)
            .block_timestamp(to_nano(time_stamp))
            .attached_deposit(1)
            .build());
        contract.withdraw_seed(accounts(1).into(), U128(amount));
    }

    fn claim_reward(
        context: &mut VMContextBuilder,
        contract: &mut Contract,
        farmer: ValidAccountId,
        time_stamp: u32,
    ) {
        testing_env!(context
            .predecessor_account_id(farmer)
            .is_view(false)
            .block_timestamp(to_nano(time_stamp))
            .attached_deposit(1)
            .build());
        contract.claim_reward_by_farm(String::from("bob#0"));
    }

    fn claim_reward_by_seed(
        context: &mut VMContextBuilder,
        contract: &mut Contract,
        farmer: ValidAccountId,
        time_stamp: u32,
    ) {
        testing_env!(context
            .predecessor_account_id(farmer)
            .is_view(false)
            .block_timestamp(to_nano(time_stamp))
            .attached_deposit(1)
            .build());
        contract.claim_reward_by_seed(String::from("bob"));
    }

    fn remove_farm(context: &mut VMContextBuilder, contract: &mut Contract, time_stamp: u32) {
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .is_view(false)
            .block_timestamp(to_nano(time_stamp))
            .build());
        contract.force_clean_farm(String::from("bob#0"));
    }

    fn remove_user_rps(
        context: &mut VMContextBuilder,
        contract: &mut Contract,
        farmer: ValidAccountId,
        farm_id: String,
        time_stamp: u32,
    ) -> bool {
        testing_env!(context
            .predecessor_account_id(farmer)
            .is_view(false)
            .block_timestamp(to_nano(time_stamp))
            .build());
        contract.remove_user_rps_by_farm(farm_id)
    }

    fn to_yocto(value: &str) -> u128 {
        let vals: Vec<_> = value.split('.').collect();
        let part1 = vals[0].parse::<u128>().unwrap() * 10u128.pow(24);
        if vals.len() > 1 {
            let power = vals[1].len() as u32;
            let part2 = vals[1].parse::<u128>().unwrap() * 10u128.pow(24 - power);
            part1 + part2
        } else {
            part1
        }
    }

    #[test]
    fn test_basics() {
        let (mut context, mut contract) = setup_contract();
        // seed is bob, reward is charlie
        let farm_id = create_farm(
            &mut context,
            &mut contract,
            accounts(1),
            accounts(2),
            5000,
            50,
        );
        assert_eq!(farm_id, String::from("bob#0"));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        // assert_eq!(farm_info.farm_kind, String::from("SIMPLE_FARM"));
        assert_eq!(farm_info.farm_status, String::from("Created"));
        assert_eq!(farm_info.seed_id, String::from("bob"));
        assert_eq!(farm_info.reward_token, String::from("charlie"));
        assert_eq!(farm_info.reward_per_session, U128(5000));
        assert_eq!(farm_info.session_interval, 50);

        // deposit 50k, can last 10 rounds from 0 to 9
        deposit_reward(&mut context, &mut contract, 50000, 100);
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.farm_status, String::from("Running"));
        assert_eq!(farm_info.start_at, 100);

        // Farmer accounts(0) come in round 1
        register_farmer(&mut context, &mut contract, accounts(0));
        deposit_seed(&mut context, &mut contract, accounts(0), 160, 10);
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(0));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.beneficiary_reward, U128(5000));
        assert_eq!(farm_info.cur_round, 1);
        assert_eq!(farm_info.last_round, 1);

        // move to round 2, 5k unclaimed for accounts(0)
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(210))
            .is_view(true)
            .build());
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(5000));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 2);
        assert_eq!(farm_info.last_round, 1);

        // Farmer accounts(3) come in
        register_farmer(&mut context, &mut contract, accounts(3));
        // deposit seed
        deposit_seed(&mut context, &mut contract, accounts(3), 260, 10);
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed, U128(0));
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(10000));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 3);
        assert_eq!(farm_info.last_round, 3);

        // move to round 4,
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(320))
            .is_view(true)
            .build());
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(12500));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed, U128(2500));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 4);
        assert_eq!(farm_info.last_round, 3);

        // remove all seeds at round 5
        println!("----> remove all seeds at round 5");
        withdraw_seed(&mut context, &mut contract, accounts(0), 360, 10);
        withdraw_seed(&mut context, &mut contract, accounts(3), 370, 10);
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(380))
            .is_view(true)
            .build());
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(0));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed, U128(0));
        let rewarded = contract.get_reward(accounts(0), accounts(2));
        assert_eq!(rewarded, U128(0));
        let rewarded = contract.get_reward(accounts(3), accounts(2));
        assert_eq!(rewarded, U128(0));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 5);
        assert_eq!(farm_info.last_round, 5);

        // move to round 7, account3 come in again
        println!("----> move to round 7, account3 come in again");
        deposit_seed(&mut context, &mut contract, accounts(3), 460, 10);
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed, U128(0));
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(0));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.beneficiary_reward, U128(15000));
        assert_eq!(farm_info.cur_round, 7);
        assert_eq!(farm_info.last_round, 7);

        // move to round 8, account0 come in again
        println!("----> move to round 8, account0 come in again");
        deposit_seed(&mut context, &mut contract, accounts(0), 520, 10);
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed, U128(5000));
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(0));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 8);
        assert_eq!(farm_info.last_round, 8);

        // move to round 9,
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(580))
            .is_view(true)
            .build());
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(2500));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed, U128(7500));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 9);
        assert_eq!(farm_info.last_round, 8);
        assert_eq!(farm_info.farm_status, String::from("Running"));

        // move to round 10,
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(610))
            .is_view(true)
            .build());
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(5000));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed, U128(10000));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 10);
        assert_eq!(farm_info.last_round, 8);
        assert_eq!(farm_info.farm_status, String::from("Ended"));

        // claim reward
        println!("----> accounts(0) and accounts(3) claim reward");
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(710))
            .is_view(true)
            .build());
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(5000));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed, U128(10000));
        claim_reward(&mut context, &mut contract, accounts(0), 720);
        claim_reward(&mut context, &mut contract, accounts(3), 730);
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(740))
            .is_view(true)
            .build());
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(0));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed, U128(0));
        let rewarded = contract.get_reward(accounts(0), accounts(2));
        assert_eq!(rewarded, U128(5000));
        let rewarded = contract.get_reward(accounts(3), accounts(2));
        assert_eq!(rewarded, U128(10000));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 10);
        assert_eq!(farm_info.last_round, 10);

        // clean farm
        println!("----> clean farm");
        remove_farm(&mut context, &mut contract, 750);
        assert!(contract.get_farm(farm_id.clone()).is_none());

        // remove user rps
        println!("----> remove user rps");
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(760))
            .is_view(true)
            .build());
        let prev_available = contract
            .storage_balance_of(accounts(0))
            .expect("Error")
            .available
            .0;
        let ret = remove_user_rps(
            &mut context,
            &mut contract,
            accounts(0).into(),
            String::from("bob#0"),
            770,
        );
        assert!(ret);
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(780))
            .is_view(true)
            .build());
        let post_available = contract
            .storage_balance_of(accounts(0))
            .expect("Error")
            .available
            .0;
        assert_eq!(post_available - prev_available, 165 * 10_u128.pow(19));

        // withdraw seed
        println!("----> accounts(0) and accounts(3) withdraw seed");
        withdraw_seed(&mut context, &mut contract, accounts(0), 800, 10);
        withdraw_seed(&mut context, &mut contract, accounts(3), 810, 10);
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(820))
            .is_view(true)
            .build());
        let rewarded = contract.get_reward(accounts(0), accounts(2));
        assert_eq!(rewarded, U128(5000));
        let rewarded = contract.get_reward(accounts(3), accounts(2));
        assert_eq!(rewarded, U128(10000));
    }

    #[test]
    fn test_unclaimed_rewards() {
        let (mut context, mut contract) = setup_contract();
        // seed is bob, reward is charlie
        let farm_id = create_farm(
            &mut context,
            &mut contract,
            accounts(1),
            accounts(2),
            to_yocto("1"),
            50,
        );
        assert_eq!(farm_id, String::from("bob#0"));

        // deposit 10, can last 10 rounds from 0 to 9
        deposit_reward(&mut context, &mut contract, to_yocto("10"), 100);

        // Farmer1 accounts(0) come in round 0
        register_farmer(&mut context, &mut contract, accounts(0));
        deposit_seed(&mut context, &mut contract, accounts(0), 110, to_yocto("1"));
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed, U128(0));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 0);
        assert_eq!(farm_info.last_round, 0);
        assert_eq!(farm_info.claimed_reward.0, 0);
        assert_eq!(farm_info.unclaimed_reward.0, 0);

        // move to round 1,
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(160))
            .is_view(true)
            .build());
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("1"));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 1);
        assert_eq!(farm_info.last_round, 0);
        assert_eq!(farm_info.claimed_reward.0, to_yocto("0"));
        assert_eq!(farm_info.unclaimed_reward.0, to_yocto("1"));

        // Farmer2 accounts(3) come in round 1
        register_farmer(&mut context, &mut contract, accounts(3));
        // deposit seed
        deposit_seed(&mut context, &mut contract, accounts(3), 180, to_yocto("1"));
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("1"));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0"));

        // move to round 2,
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(210))
            .is_view(true)
            .build());
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("1.5"));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0.5"));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 2);
        assert_eq!(farm_info.last_round, 1);
        assert_eq!(farm_info.claimed_reward.0, to_yocto("0"));
        assert_eq!(farm_info.unclaimed_reward.0, to_yocto("2"));

        // farmer1 claim reward by farm_id at round 3
        claim_reward(&mut context, &mut contract, accounts(0), 260);
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0"));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("1"));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 3);
        assert_eq!(farm_info.last_round, 3);
        assert_eq!(farm_info.claimed_reward.0, to_yocto("2"));
        assert_eq!(farm_info.unclaimed_reward.0, to_yocto("1"));

        // farmer2 claim reward by seed_id at round 4
        claim_reward_by_seed(&mut context, &mut contract, accounts(3), 310);
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0.5"));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0"));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 4);
        assert_eq!(farm_info.last_round, 4);
        assert_eq!(farm_info.claimed_reward.0, to_yocto("3.5"));
        assert_eq!(farm_info.unclaimed_reward.0, to_yocto("0.5"));

        // farmer1 unstake half lpt at round 5
        withdraw_seed(
            &mut context,
            &mut contract,
            accounts(0),
            360,
            to_yocto("0.4"),
        );
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0"));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0.5"));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 5);
        assert_eq!(farm_info.last_round, 5);
        assert_eq!(farm_info.claimed_reward.0, to_yocto("4.5"));
        assert_eq!(farm_info.unclaimed_reward.0, to_yocto("0.5"));

        // farmer2 unstake all his lpt at round 6
        withdraw_seed(&mut context, &mut contract, accounts(3), 410, to_yocto("1"));
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0.375"));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0"));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 6);
        assert_eq!(farm_info.last_round, 6);
        assert_eq!(farm_info.claimed_reward.0, to_yocto("5.625"));
        assert_eq!(farm_info.unclaimed_reward.0, to_yocto("0.375"));

        // move to round 7
        testing_env!(context
            .predecessor_account_id(accounts(0))
            .block_timestamp(to_nano(460))
            .is_view(true)
            .build());
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("1.374999999999999999999999"));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0"));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 7);
        assert_eq!(farm_info.last_round, 6);
        assert_eq!(farm_info.claimed_reward.0, to_yocto("5.625"));
        assert_eq!(farm_info.unclaimed_reward.0, to_yocto("1.375"));
        withdraw_seed(
            &mut context,
            &mut contract,
            accounts(0),
            470,
            to_yocto("0.6"),
        );
        let unclaimed = contract.get_unclaimed_reward(accounts(0), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0"));
        let unclaimed = contract.get_unclaimed_reward(accounts(3), farm_id.clone());
        assert_eq!(unclaimed.0, to_yocto("0"));
        let farm_info = contract.get_farm(farm_id.clone()).expect("Error");
        assert_eq!(farm_info.cur_round, 7);
        assert_eq!(farm_info.last_round, 7);
        assert_eq!(
            farm_info.claimed_reward.0,
            to_yocto("6.999999999999999999999999")
        );
        assert_eq!(farm_info.unclaimed_reward.0, 1);
    }

    #[test]
    #[should_panic(expected = "E11: insufficient $NEAR storage deposit")]
    fn test_storage_withdraw() {
        let (mut context, mut contract) = setup_contract();
        // Farmer1 accounts(0) come in round 0
        register_farmer(&mut context, &mut contract, accounts(0));
        // println!("locked: {}, deposited: {}", sb.total.0, sb.available.0);
        let sb = storage_withdraw(&mut context, &mut contract, accounts(0));
        // println!("locked: {}, deposited: {}", sb.total.0, sb.available.0);
        assert_eq!(sb.total.0, 920000000000000000000);
        assert_eq!(sb.available.0, 0);

        let farm_id = create_farm(
            &mut context,
            &mut contract,
            accounts(1),
            accounts(2),
            5000,
            50,
        );
        assert_eq!(farm_id, String::from("bob#0"));

        deposit_seed(&mut context, &mut contract, accounts(0), 60, 10);
    }
}
