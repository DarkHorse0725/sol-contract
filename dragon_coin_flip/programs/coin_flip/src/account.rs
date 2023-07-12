use anchor_lang::prelude::*;



#[account]
#[derive(Default)]
pub struct GlobalState {
    pub admin: Pubkey,
    pub woof_mint: Pubkey,
    pub ticket_mint: Pubkey,
    pub vault: Pubkey,
    pub bet_amounts: [u64; 10],
    pub reward_rates: [u32; 10],
    pub percentages: [u32; 10],
    pub item_count: u32,
}


#[account]
#[derive(Default)]
pub struct UserState {
    pub user: Pubkey,
    pub reward_amount: u64,
    pub game_mode: u32,
}
