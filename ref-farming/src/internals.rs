use near_sdk::{env, Balance};

use crate::utils::{get_nft_balance_equivalent};
use crate::farm_seed::SeedType;
use crate::*;
use uint::construct_uint;

construct_uint! {
    /// 256-bit unsigned integer.
    pub struct U256(4);
}

fn claim_user_reward_from_farm(
    farm: &mut Farm, 
    farmer: &mut Farmer, 
    total_seeds: &Balance,
    silent: bool,
) {
    let user_seeds = farmer.seeds.get(&farm.get_seed_id()).unwrap_or(&0_u128);
    let user_rps = farmer.get_rps(&farm.get_farm_id());
    let (new_user_rps, reward_amount) = farm.claim_user_reward(&user_rps, user_seeds, total_seeds, silent);
    if !silent {
        env::log(
            format!(
                "user_rps@{} increased to {}",
                farm.get_farm_id(), U256::from_little_endian(&new_user_rps),
            )
            .as_bytes(),
        );
    }
        
    farmer.set_rps(&farm.get_farm_id(), new_user_rps);
    if reward_amount > 0 {
        farmer.add_reward(&farm.get_reward_token(), reward_amount);
        if !silent {
            env::log(
                format!(
                    "claimed {} {} as reward from {}",
                    reward_amount, farm.get_reward_token() , farm.get_farm_id(),
                )
                .as_bytes(),
            );
        }
    }
}

impl Contract {

    pub(crate) fn data(&self) -> &ContractData {
        return &self.data;
    }

    pub(crate) fn data_mut(&mut self) -> &mut ContractData {
        return &mut self.data;
    }

    /// Adds given farm to the vec and returns it's id.
    /// If there is not enough attached balance to cover storage, fails.
    /// If too much attached - refunds it back.
    pub(crate) fn internal_add_farm(
        &mut self,
        terms: &HRFarmTerms,
        min_deposit: Balance,
        nft_balance: Option<HashMap<NFTTokenId, U128>>,
        metadata: Option<FarmSeedMetadata>
    ) -> FarmId {
        
        // let mut farm_seed = self.get_seed_default(&terms.seed_id, min_deposit);
        let mut farm_seed: FarmSeed;
        if let Some(fs) = self.get_seed_wrapped(&terms.seed_id) {
            farm_seed = fs;
            env::log(
                format!(
                    "New farm created In seed {}, with existed min_deposit {}",
                    terms.seed_id, farm_seed.get_ref().min_deposit
                )
                .as_bytes(),
            );
        } else {
            if let Some(nft_balance) = nft_balance {
                farm_seed = FarmSeed::new(&terms.seed_id, min_deposit, true, metadata);
                self.data_mut().nft_balance_seeds.insert(&terms.seed_id, &nft_balance);
            } else {
                farm_seed = FarmSeed::new(&terms.seed_id, min_deposit, false, metadata);
            }
            env::log(
                format!(
                    "The first farm created In seed {}, with min_deposit {}",
                    terms.seed_id, farm_seed.get_ref().min_deposit
                )
                .as_bytes(),
            );
        }

        let farm_id: FarmId = gen_farm_id(&terms.seed_id, farm_seed.get_ref().next_index as usize);

        let farm = Farm::new(
            farm_id.clone(),
            terms.into()
        );
        
        farm_seed.get_ref_mut().farms.insert(farm_id.clone());
        farm_seed.get_ref_mut().next_index += 1;
        self.data_mut().seeds.insert(&terms.seed_id, &farm_seed);
        self.data_mut().farms.insert(&farm_id.clone(), &farm);
        farm_id
    }

    pub(crate) fn internal_remove_farm_by_farm_id(&mut self, farm_id: &FarmId) -> bool {
        let (seed_id, _) = parse_farm_id(farm_id);
        let mut removable = false;
        if let Some(mut farm_seed) = self.get_seed_wrapped(&seed_id) {
            let seed_amount = farm_seed.get_ref().amount;
            if let Some(farm) = self.data().farms.get(farm_id) {
                if farm.can_be_removed(&seed_amount) {
                    removable = true;
                }
            }
            if removable {
                let mut farm = self.data_mut().farms.remove(farm_id).expect(ERR41_FARM_NOT_EXIST);
                farm.move_to_clear(&seed_amount);
                self.data_mut().outdated_farms.insert(farm_id, &farm);
                farm_seed.get_ref_mut().farms.remove(farm_id);
                self.data_mut().seeds.insert(&seed_id, &farm_seed);
                return true;
            }
        }
        false
    }

    pub(crate) fn internal_claim_user_reward_by_seed_id(
        &mut self, 
        sender_id: &AccountId,
        seed_id: &SeedId) {
        let mut farmer = self.get_farmer(sender_id);
        if let Some(mut farm_seed) = self.get_seed_wrapped(seed_id) {
            let amount = farm_seed.get_ref().amount;
            for farm_id in &mut farm_seed.get_ref_mut().farms.iter() {
                let mut farm = self.data().farms.get(farm_id).unwrap();
                claim_user_reward_from_farm(
                    &mut farm, 
                    farmer.get_ref_mut(),  
                    &amount,
                    true,
                );
                self.data_mut().farms.insert(farm_id, &farm);
            }
            self.data_mut().seeds.insert(seed_id, &farm_seed);
            self.data_mut().farmers.insert(sender_id, &farmer);
        }
    }

    pub(crate) fn internal_claim_user_reward_by_farm_id(
        &mut self, 
        sender_id: &AccountId, 
        farm_id: &FarmId) {
        let mut farmer = self.get_farmer(sender_id);

        let (seed_id, _) = parse_farm_id(farm_id);

        if let Some(farm_seed) = self.get_seed_wrapped(&seed_id) {
            let amount = farm_seed.get_ref().amount;
            if let Some(mut farm) = self.data().farms.get(farm_id) {
                claim_user_reward_from_farm(
                    &mut farm, 
                    farmer.get_ref_mut(), 
                    &amount,
                    false,
                );
                self.data_mut().farms.insert(farm_id, &farm);
                self.data_mut().farmers.insert(sender_id, &farmer);
            }
        }
    }


    #[inline]
    pub(crate) fn get_farmer(&self, from: &AccountId) -> VersionedFarmer {
        let orig = self.data().farmers
            .get(from)
            .expect(ERR10_ACC_NOT_REGISTERED);
        if orig.need_upgrade() {
                orig.upgrade()
            } else {
                orig
            }
    }

    #[inline]
    pub(crate) fn get_farmer_default(&self, from: &AccountId) -> VersionedFarmer {
        let orig = self.data().farmers.get(from).unwrap_or(VersionedFarmer::new(from.clone(), 0));
        if orig.need_upgrade() {
            orig.upgrade()
        } else {
            orig
        }
    }

    #[inline]
    pub(crate) fn get_farmer_wrapped(&self, from: &AccountId) -> Option<VersionedFarmer> {
        if let Some(farmer) = self.data().farmers.get(from) {
            if farmer.need_upgrade() {
                Some(farmer.upgrade())
            } else {
                Some(farmer)
            }
        } else {
            None
        }
    }

    /// Returns current balance of given token for given user. 
    /// If there is nothing recorded, returns 0.
    pub(crate) fn internal_get_reward(
        &self,
        sender_id: &AccountId,
        token_id: &AccountId,
    ) -> Balance {
        self.get_farmer_default(sender_id)
            .get_ref().rewards.get(token_id).cloned()
            .unwrap_or_default()
    }

    #[inline]
    pub(crate) fn get_seed_and_upgrade(&mut self, seed_id: &String) -> FarmSeed {
        return self.data().seeds.get(seed_id).expect(&format!("{}", ERR31_SEED_NOT_EXIST));
    }

    #[inline]
    pub(crate) fn get_seed(&self, seed_id: &String) -> FarmSeed {
        return self.data().seeds.get(seed_id).expect(&format!("{}", ERR31_SEED_NOT_EXIST)); 
    }

    #[inline]
    pub(crate) fn get_seed_wrapped(&self, seed_id: &String) -> Option<FarmSeed> {
        if let Some(farm_seed) = self.data().seeds.get(seed_id) {
            Some(farm_seed)
        } else {
            None
        }
    }

    pub(crate) fn internal_seed_deposit(
        &mut self, 
        seed_id: &String, 
        sender_id: &AccountId, 
        amount: Balance, 
        seed_type: SeedType) {

        // first claim all reward of the user for this seed farms
        // to update user reward_per_seed in each farm
        self.internal_claim_user_reward_by_seed_id(sender_id, seed_id);

        let mut farm_seed = self.get_seed(seed_id);

        let mut farmer = self.get_farmer(sender_id);

        // **** update seed (new version)
        farm_seed.get_ref_mut().add_amount(amount);
        self.data_mut().seeds.insert(&seed_id, &farm_seed);

        farmer.get_ref_mut().add_seed(&seed_id, amount);
        self.data_mut().farmers.insert(sender_id, &farmer);

        let mut reward_tokens: Vec<AccountId> = vec![];
        for farm_id in farm_seed.get_ref().farms.iter() {
            let reward_token = self.data().farms.get(farm_id).unwrap().get_reward_token();
            if !reward_tokens.contains(&reward_token) {
                if farmer.get_ref().rewards.get(&reward_token).is_some() {
                    self.private_withdraw_reward(reward_token.clone(), sender_id.to_string(), None);
                }
                reward_tokens.push(reward_token);
            }
        };
    }

    pub(crate) fn internal_seed_withdraw(
        &mut self, 
        seed_id: &SeedId, 
        sender_id: &AccountId, 
        amount: Balance) -> SeedType {

        // first claim all reward of the user for this seed farms
        // to update user reward_per_seed in each farm
        self.internal_claim_user_reward_by_seed_id(sender_id, seed_id);

        let mut farm_seed = self.get_seed(seed_id);
        let mut farmer = self.get_farmer(sender_id);

        // Then update user seed and total seed of this LPT
        let farmer_seed_remain = farmer.get_ref_mut().sub_seed(seed_id, amount);
        let _seed_remain = farm_seed.get_ref_mut().sub_amount(amount);

        if farmer_seed_remain == 0 {
            // remove farmer rps of relative farm
            for farm_id in farm_seed.get_ref().farms.iter() {
                farmer.get_ref_mut().remove_rps(farm_id);
            }
        }
        self.data_mut().farmers.insert(sender_id, &farmer);
        self.data_mut().seeds.insert(seed_id, &farm_seed);

        let mut reward_tokens: Vec<AccountId> = vec![];
        for farm_id in farm_seed.get_ref().farms.iter() {
            let reward_token = self.data().farms.get(farm_id).unwrap().get_reward_token();
            if !reward_tokens.contains(&reward_token) {
                if farmer.get_ref().rewards.get(&reward_token).is_some() {
                    self.private_withdraw_reward(reward_token.clone(), sender_id.to_string(), None);
                }
                reward_tokens.push(reward_token);
            }
        };

        farm_seed.get_ref().seed_type.clone()
    }

    pub(crate) fn internal_nft_deposit(
        &mut self,
        seed_id: &String,
        sender_id: &AccountId,
        nft_contract_id: &String,
        nft_token_id: &String,
    ) -> bool {
        let mut farm_seed = self.get_seed(seed_id);

        assert_eq!(farm_seed.get_ref().seed_type, SeedType::NFT, "Cannot deposit NFT to this farm");

        // update farmer seed
        let contract_nft_token_id = format!("{}{}{}", nft_contract_id, NFT_DELIMETER, nft_token_id);
        let nft_balance = self.data().nft_balance_seeds.get(&seed_id).unwrap();
        return if let Some(nft_balance_equivalent) = get_nft_balance_equivalent(nft_balance, contract_nft_token_id.clone()) {
            // first claim all reward of the user for this seed farms
            // to update user reward_per_seed in each farm
            self.internal_claim_user_reward_by_seed_id(sender_id, seed_id);
            let mut farmer = self.get_farmer(sender_id);
            farmer.get_ref_mut().add_nft(seed_id, contract_nft_token_id);

            farmer.get_ref_mut().add_seed(seed_id, nft_balance_equivalent);
            self.data_mut().farmers.insert(sender_id, &farmer);

            // **** update seed (new version)
            farm_seed.get_ref_mut().add_amount(nft_balance_equivalent);
            self.data_mut().seeds.insert(&seed_id, &farm_seed);

            let mut reward_tokens: Vec<AccountId> = vec![];
            for farm_id in farm_seed.get_ref().farms.iter() {
                let reward_token = self.data().farms.get(farm_id).unwrap().get_reward_token();
                if !reward_tokens.contains(&reward_token) {
                    if farmer.get_ref().rewards.get(&reward_token).is_some() {
                        self.private_withdraw_reward(reward_token.clone(), sender_id.to_string(), None);
                    }
                    reward_tokens.push(reward_token);
                }
            };

            true
        } else {
            false
        }
    }

    pub(crate) fn internal_nft_withdraw(
        &mut self,
        seed_id: &String,
        sender_id: &AccountId,
        nft_contract_id: &String,
        nft_token_id: &String
    ) -> ContractNFTTokenId {
        self.internal_claim_user_reward_by_seed_id(sender_id, seed_id);

        let mut farm_seed = self.get_seed(seed_id);
        let mut farmer = self.get_farmer(sender_id);

        // sub nft
        let contract_nft_token_id : ContractNFTTokenId = format!("{}{}{}", nft_contract_id, NFT_DELIMETER, nft_token_id);
        farmer.get_ref_mut().sub_nft(seed_id, contract_nft_token_id.clone()).unwrap();
        let nft_balance = self.data().nft_balance_seeds.get(&seed_id).unwrap();
        let nft_balance_equivalent: Balance = get_nft_balance_equivalent(nft_balance, contract_nft_token_id.clone()).unwrap();

        let farmer_seed_remain = farmer.get_ref_mut().sub_seed(seed_id, nft_balance_equivalent);

        // calculate farm_seed after multiplier get removed
        farm_seed.get_ref_mut().sub_amount(nft_balance_equivalent);

        if farmer_seed_remain == 0 {
            // remove farmer rps of relative farm
            for farm_id in farm_seed.get_ref().farms.iter() {
                farmer.get_ref_mut().remove_rps(farm_id);
            }
        }

        self.data_mut().farmers.insert(sender_id, &farmer);
        self.data_mut().seeds.insert(seed_id, &farm_seed);

        let mut reward_tokens: Vec<AccountId> = vec![];
        for farm_id in farm_seed.get_ref().farms.iter() {
            let reward_token = self.data().farms.get(farm_id).unwrap().get_reward_token();
            if !reward_tokens.contains(&reward_token) {
                if farmer.get_ref().rewards.get(&reward_token).is_some() {
                    self.private_withdraw_reward(reward_token.clone(), sender_id.to_string(), None);
                }
                reward_tokens.push(reward_token);
            }
        };

        contract_nft_token_id
    }
}