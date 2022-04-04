//!   The SimpleFarm provide a way to gain farming rewards periodically and 
//! proportionally.
//!   The creator first wrap his reward distribution schema with 
//! `SimpleFarmRewardTerms`, and create the farm with it, attached enough near 
//! for storage fee.
//!   But to enable farming, the creator or someone else should deposit reward 
//! token to the farm, after it was created.

use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::json_types::{U128, ValidAccountId};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, AccountId, Balance};
use crate::SeedId;
use crate::errors::*;
use crate::utils::*;
use uint::construct_uint;

pub(crate) type FarmId = String;

construct_uint! {
    /// 256-bit unsigned integer.
    pub struct U256(4);
}

pub type ContractNFTTokenId = String;
pub type NFTTokenId = String;

pub type RPS = [u8; 32];

// to ensure precision, all reward_per_seed would be multiplied by this DENOM
// this value should be carefully choosen, now is 10**24.
pub const DENOM: u128 = 1_000_000_000_000_000_000_000_000;

///   The terms defines how the farm works.
///   In this version, we distribute reward token with a start height, a reward 
/// session interval, and reward amount per session.  
///   In this way, the farm will take the amount from undistributed reward to  
/// unclaimed reward each session. And all farmers would got reward token pro  
/// rata of their seeds.
#[derive(BorshSerialize, BorshDeserialize, Clone)]
pub struct FarmTerms {
    pub seed_id: SeedId,
    pub reward_token: AccountId,
    pub start_at: TimestampSec,
    pub reward_per_session: Balance,
    pub session_interval: TimestampSec,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct HRFarmTerms {
    pub seed_id: SeedId,
    pub reward_token: ValidAccountId,
    pub start_at: u32,
    pub reward_per_session: U128,
    pub session_interval: u32,
}

impl From<&HRFarmTerms> for FarmTerms {
    fn from(terms: &HRFarmTerms) -> Self {
        FarmTerms {
            seed_id: terms.seed_id.clone(),
            reward_token: terms.reward_token.clone().into(),
            start_at: terms.start_at,
            reward_per_session: terms.reward_per_session.into(),
            session_interval: terms.session_interval,
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone)]
pub enum FarmStatus {
    Created, Running, Ended, Cleared
}

impl From<&FarmStatus> for String {
    fn from(status: &FarmStatus) -> Self {
        match *status {
            FarmStatus::Created => { String::from("Created") },
            FarmStatus::Running => { String::from("Running") },
            FarmStatus::Ended => { String::from("Ended") },
            FarmStatus::Cleared => { String::from("Cleared") },
        }
    }
}

/// Reward Distribution Record
#[derive(BorshSerialize, BorshDeserialize, Clone, Default)]
pub struct FarmRewardDistribution {
    /// unreleased reward
    pub undistributed: Balance,
    /// the total rewards distributed but not yet claimed by farmers.
    pub unclaimed: Balance,
    /// Reward_Per_Seed
    /// rps(cur) = rps(prev) + distributing_reward / total_seed_staked
    pub rps: RPS,
    /// Reward_Round
    /// rr = (cur_block_timestamp in sec - start_at) / session_interval
    pub rr: u32,
}

///   Implementation of simple farm, Similar to the design of "berry farm".
///   Farmer stake their seed to farming on multiple farm accept that seed.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct Farm {

    pub farm_id: FarmId,
    
    pub terms: FarmTerms,

    pub status: FarmStatus,

    pub last_distribution: FarmRewardDistribution,

    /// total reward send into this farm by far, 
    /// every time reward deposited in, add to this field
    pub amount_of_reward: Balance,
    /// reward token has been claimed by farmer by far
    pub amount_of_claimed: Balance,
    /// when there is no seed token staked, reward goes to beneficiary
    pub amount_of_beneficiary: Balance,
}

impl Farm {
    pub fn new(
        id: FarmId,
        terms: FarmTerms,
    ) -> Self {
        Self {
            farm_id: id.clone(),
            amount_of_reward: 0,
            amount_of_claimed: 0,
            amount_of_beneficiary: 0,

            status: FarmStatus::Created,
            last_distribution: FarmRewardDistribution::default(),
            terms,
        }
    }

    /// return None if the farm can not accept reward anymore
    /// else return amount of undistributed reward 
    pub fn add_reward(&mut self, amount: &Balance) -> Option<Balance> {

        match self.status {
            FarmStatus::Created => {
                // When a farm gots first deposit of reward, it turns to Running state,
                // but farming or not depends on `start_at` 
                self.status = FarmStatus::Running;
                if self.terms.start_at == 0 {
                    // for a farm without start time, the first deposit of reward 
                    // would trigger the farming
                    self.terms.start_at = to_sec(env::block_timestamp());
                }
                self.amount_of_reward += amount;
                self.last_distribution.undistributed += amount;
                Some(self.last_distribution.undistributed)
            },
            FarmStatus::Running => {
                if let Some(dis) = self.try_distribute(&DENOM) {
                    if dis.undistributed == 0 {
                        // farm has ended actually
                        return None;
                    }
                }
                // For a running farm, can add reward to extend duration
                self.amount_of_reward += amount;
                self.last_distribution.undistributed += amount;
                Some(self.last_distribution.undistributed)
            },
            _ => {None},
        }
        
    }


    /// Try to distribute reward according to current timestamp
    /// return None if farm is not in Running state or haven't start farming yet;
    /// return new dis :FarmRewardDistribution 
    /// Note, if total_seed is 0, the rps in new dis would be reset to 0 too.
    pub fn try_distribute(&self, total_seeds: &Balance) -> Option<FarmRewardDistribution> {

        if let FarmStatus::Running = self.status {
            if env::block_timestamp() < to_nano(self.terms.start_at) {
                // a farm haven't start yet
                return None;
            }
            let mut dis = self.last_distribution.clone();
            // calculate rr according to cur_timestamp
            dis.rr = (to_sec(env::block_timestamp()) - self.terms.start_at) / self.terms.session_interval;
            let mut reward_added = (dis.rr - self.last_distribution.rr) as u128 
                * self.terms.reward_per_session;
            if self.last_distribution.undistributed < reward_added {
                // all undistribution would be distributed this time
                reward_added = self.last_distribution.undistributed;
                // recalculate rr according to undistributed
                let increased_rr = (reward_added / self.terms.reward_per_session) as u32;
                dis.rr = self.last_distribution.rr + increased_rr;
                let reward_caculated = increased_rr as u128 * self.terms.reward_per_session;
                if reward_caculated < reward_added {
                    // add the tail round
                    dis.rr += 1;

                }
                // env::log(
                //     format!(
                //         "Farm ends at Round #{}, unclaimed reward: {}.",
                //         dis.rr, reward_added + dis.unclaimed
                //     )
                //     .as_bytes(),
                // );
            }
            dis.unclaimed += reward_added;
            dis.undistributed -= reward_added;

            // calculate rps
            if total_seeds == &0 {
                U256::from(0).to_little_endian(&mut dis.rps);
            } else {
                (
                    U256::from_little_endian(&self.last_distribution.rps) + 
                    U256::from(reward_added) 
                    * U256::from(DENOM) 
                    / U256::from(*total_seeds)
                ).to_little_endian(&mut dis.rps);
            }
            Some(dis)
        } else {
            None
        }

    }

    /// Return how many reward token that the user hasn't claimed yet.
    /// return (cur_rps - last_user_rps) * user_seeds / DENOM
    pub fn view_farmer_unclaimed_reward(
        &self,
        user_rps: &RPS,
        user_seeds: &Balance,
        total_seeds: &Balance,
    ) -> Balance {
        if total_seeds == &0 {
            return 0;
        }
        if user_seeds == &0 {
            return 0;
        }
        if let Some(dis) = self.try_distribute(total_seeds) {
            (U256::from(*user_seeds) 
            * (U256::from_little_endian(&dis.rps) - U256::from_little_endian(user_rps))
            / U256::from(DENOM)).as_u128()
        } else {
            (U256::from(*user_seeds) 
            * (U256::from_little_endian(&self.last_distribution.rps) - U256::from_little_endian(user_rps))
            / U256::from(DENOM)).as_u128()
        }
    }

    /// Distribute reward generated from previous distribution to now,
    /// only works for farm in Running state and has reward deposited in,
    /// Note 1, if undistribute equals 0, the farm goes to Ended state;
    /// Note 2, if total_seed is 0, reward is claimed directly by beneficiary
    pub fn distribute(&mut self, total_seeds: &Balance, silent: bool) {
        if let Some(dis) = self.try_distribute(total_seeds) {
            if self.last_distribution.rr != dis.rr {
                self.last_distribution = dis.clone();
                if total_seeds == &0 {
                    // if total_seeds == &0, reward goes to beneficiary,
                    self.amount_of_claimed += self.last_distribution.unclaimed;
                    self.amount_of_beneficiary += self.last_distribution.unclaimed;
                    self.last_distribution.unclaimed = 0;
                }   
                if !silent {
                    env::log(
                        format!(
                            "{} RPS increased to {} and RR update to #{}",
                            self.farm_id, U256::from_little_endian(&dis.rps), dis.rr,
                        )
                        .as_bytes(),
                    );
                }
                
            }
            if self.last_distribution.undistributed == 0 {
                self.status = FarmStatus::Ended;
            }
        } 
    }

    /// Claim user's unclaimed reward in this farm,
    /// return the new user RPS (reward per seed),  
    /// and amount of reward 
    pub fn claim_user_reward(
        &mut self, 
        user_rps: &RPS,
        user_seeds: &Balance, 
        total_seeds: &Balance, 
        silent: bool,
    ) -> (RPS, Balance) {

        self.distribute(total_seeds, silent);
        // if user_seeds == &0 {
        //     return (self.last_distribution.rps, 0);
        // }

        let claimed = (
            U256::from(*user_seeds) 
            * (U256::from_little_endian(&self.last_distribution.rps) - U256::from_little_endian(user_rps))
            / U256::from(DENOM)
        ).as_u128();

        if claimed > 0 {
            assert!(
                self.last_distribution.unclaimed >= claimed, 
                "{} unclaimed:{}, cur_claim:{}", 
                ERR500, self.last_distribution.unclaimed, claimed
            );
            self.last_distribution.unclaimed -= claimed;
            self.amount_of_claimed += claimed;
        }

        (self.last_distribution.rps, claimed)
    }

    /// Move an Ended farm to Cleared, if any unclaimed reward exists, go to beneficiary
    pub fn move_to_clear(&mut self, total_seeds: &Balance) -> bool {
        if let FarmStatus::Running = self.status {
            self.distribute(total_seeds, true);
        }
        if let FarmStatus::Ended = self.status {
            if self.last_distribution.unclaimed > 0 {
                self.amount_of_claimed += self.last_distribution.unclaimed;
                self.amount_of_beneficiary += self.last_distribution.unclaimed;
                self.last_distribution.unclaimed = 0;
            }
            self.status = FarmStatus::Cleared;
            true
        } else {
            false
        }
    }

    pub fn can_be_removed(&self, total_seeds: &Balance) -> bool {
        match self.status {
            FarmStatus::Ended => true,
            FarmStatus::Running => {
                if let Some(dis) = self.try_distribute(total_seeds) {
                    if dis.undistributed == 0 {
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            },
            _ => false,
        }
    }

    /// Returns seed id this farm accepted.
    pub fn get_seed_id(&self) -> SeedId {
        return self.terms.seed_id.clone();
    }

    /// Returns token contract id this farm used for reward.
    pub fn get_reward_token(&self) -> AccountId {
        return self.terms.reward_token.clone();
    }

    pub fn get_farm_id(&self) -> FarmId {
        return self.farm_id.clone();
    }
}
