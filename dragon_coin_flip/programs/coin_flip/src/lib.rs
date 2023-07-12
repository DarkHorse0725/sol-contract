use anchor_lang::prelude::*;
use chainlink_solana as chainlink;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{self, Mint, Token, TokenAccount, Transfer},
};
use solana_program::{program::invoke, program::invoke_signed, system_instruction};
use std::mem::size_of;

pub mod account;
pub mod constants;
pub mod errors;

use account::*;
use constants::*;
use errors::*;

declare_id!("AmTdGr7b7cissZ2fVPnKuDnJieHhTFpNntdsSEbu7qFK");

#[program]
pub mod coin_flip {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>, woof_mint: Pubkey, ticket_mint: Pubkey, bet_amounts: [u64; 10], reward_rates: [u32; 10], percentages: [u32; 10], item_count: u32) -> Result<()> {
        let accts = ctx.accounts;
        accts.global_state.admin = accts.admin.key();
        accts.global_state.woof_mint = woof_mint;
        accts.global_state.ticket_mint = ticket_mint;
        accts.global_state.vault = accts.vault.key();

        for i in 0..item_count {
            accts.global_state.bet_amounts[i as usize] = bet_amounts[i as usize];
            accts.global_state.reward_rates[i as usize] = reward_rates[i as usize];
            accts.global_state.percentages[i as usize] = percentages[i as usize];
        }

        accts.global_state.item_count = item_count;

        let rent = Rent::default();
        let required_lamports = rent
            .minimum_balance(0)
            .max(1)
            .saturating_sub(accts.vault.to_account_info().lamports());
        msg!("required lamports = {:?}", required_lamports);
        invoke(
            &system_instruction::transfer(
                &accts.admin.key(),
                &accts.vault.key(),
                required_lamports,
            ),
            &[
                accts.admin.to_account_info().clone(),
                accts.vault.clone(),
                accts.system_program.to_account_info().clone(),
            ],
        )?;

        Ok(())
    }

    pub fn coinflip(ctx: Context<CoinFlip>, item_id: u8, game_mode: u8) -> Result<()> {
        let accts = ctx.accounts;
        let amount = accts.global_state.bet_amounts[item_id as usize];
        let pay_amount = amount.checked_add(amount.checked_mul(2).unwrap().checked_div(100).unwrap()).unwrap();

        accts.user_state.user = accts.user.key();

        // pay to play
        if game_mode == 0 { // pay in woof token
            let cpi_ctx = CpiContext::new(
                accts.token_program.to_account_info(),
                anchor_spl::token::Transfer {
                    from: accts.source_account.to_account_info(),
                    to: accts.dest_account.to_account_info(),
                    authority: accts.user.to_account_info(),
                },
            );

            anchor_spl::token::transfer(cpi_ctx, pay_amount)?;
        } else { // pay in sol
            invoke(
                &system_instruction::transfer(&accts.user.key(), &accts.vault.key(), pay_amount),
                &[
                    accts.user.to_account_info().clone(),
                    accts.vault.clone(),
                    accts.system_program.to_account_info().clone(),
                ],
            )?;
        }

        // flip coin
        let round = chainlink::latest_round_data(
            accts.chainlink_program.to_account_info(),
            accts.chainlink_feed.to_account_info(),
        )?;

        let ctime = Clock::get().unwrap();
        let c = ctime.unix_timestamp.checked_mul(round.answer as i64).unwrap();
        let percentage = accts.global_state.percentages[item_id as usize];
        let r = (c % 101 as i64) as u32;

        let reward_rate = accts.global_state.reward_rates[item_id as usize];
        let reward = amount
                        .checked_mul(reward_rate as u64).unwrap()
                        .checked_div(RATE_DECIMAL as u64).unwrap();
        msg!(
            "Calling the token program to transfer reward {} to the user",
            reward
        );
        accts.user_state.game_mode = game_mode as u32;
        if r <= percentage { // win case
            accts.user_state.reward_amount = reward;
        } else { // lose case
            accts.user_state.reward_amount = 0;
        }

        Ok(())
    }

    pub fn claim_reward(ctx: Context<ClaimReward>) -> Result<()> {

        let accts = ctx.accounts;
        let amount = accts.user_state.reward_amount;
        accts.user_state.reward_amount = 0;

        if amount > 0 {
            if accts.user_state.game_mode == 0 { // return ticket token
                // Transfer rewards from the pool reward vaults to user reward vaults.
                let (_pool_account_seed, _bump) =
                    Pubkey::find_program_address(&[GLOBAL_STATE_SEED.as_bytes()], ctx.program_id);
                let pool_seeds = &[GLOBAL_STATE_SEED.as_bytes(), &[_bump]];
                let signer = &[&pool_seeds[..]];

                let token_program = accts.token_program.to_account_info().clone();
                let token_accounts = anchor_spl::token::Transfer {
                    from: accts.source_account.to_account_info().clone(),
                    to: accts.dest_account.to_account_info().clone(),
                    authority: accts.global_state.to_account_info().clone(),
                };
                let cpi_ctx = CpiContext::new(token_program, token_accounts);

                anchor_spl::token::transfer(cpi_ctx.with_signer(signer), amount)?;
            } else { // sol
                let bump = ctx.bumps.get("vault").unwrap();
                invoke_signed(
                    &system_instruction::transfer(&accts.vault.key(), &accts.user.key(), amount),
                    &[
                        accts.vault.to_account_info().clone(),
                        accts.user.to_account_info().clone(),
                        accts.system_program.to_account_info().clone(),
                    ],
                    &[&[VAULT_SEED, &[*bump]]],
                )?;
            }
        }

        Ok(())
    }

    pub fn deposit_reward(ctx: Context<DepositReward>, amount: u64) -> Result<()> {
        // Transfer reward tokens into the vault.
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token::Transfer {
                from: ctx.accounts.source_account.to_account_info(),
                to: ctx.accounts.dest_account.to_account_info(),
                authority: ctx.accounts.funder.to_account_info(),
            },
        );

        anchor_spl::token::transfer(cpi_ctx, amount)?;

        Ok(())
    }

    pub fn buy_woof_token(ctx: Context<BuyWoofToken>, amount: u64) -> Result<()> {
        let accts = ctx.accounts;

        // pay in sol
        invoke(
            &system_instruction::transfer(&accts.buyer.key(), &accts.vault.key(), amount),
            &[
                accts.buyer.to_account_info().clone(),
                accts.vault.clone(),
                accts.system_program.to_account_info().clone(),
            ],
        )?;

        // woof token
        let woof_amount = amount.checked_mul(WOOFS_PER_SOL as u64).unwrap();
        let (_pool_account_seed, _bump) =
            Pubkey::find_program_address(&[GLOBAL_STATE_SEED.as_bytes()], ctx.program_id);
        let pool_seeds = &[GLOBAL_STATE_SEED.as_bytes(), &[_bump]];
        let signer = &[&pool_seeds[..]];

        let token_program = accts.token_program.to_account_info().clone();
        let token_accounts = anchor_spl::token::Transfer {
            from: accts.source_account.to_account_info().clone(),
            to: accts.dest_account.to_account_info().clone(),
            authority: accts.global_state.to_account_info().clone(),
        };
        let cpi_ctx = CpiContext::new(token_program, token_accounts);

        anchor_spl::token::transfer(cpi_ctx.with_signer(signer), woof_amount)?;

        Ok(())
    }

    pub fn withdraw_all(ctx: Context<WithdrawAll>, sol_amount: u64, woof_amount: u64, ticket_amount: u64) -> Result<()> {
        let accts = ctx.accounts;

        // withdraw sol
        let bump = ctx.bumps.get("vault").unwrap();
        if sol_amount > 0 {
            invoke_signed(
                &system_instruction::transfer(&accts.vault.key(), &accts.admin.key(), sol_amount),
                &[
                    accts.vault.to_account_info().clone(),
                    accts.admin.to_account_info().clone(),
                    accts.system_program.to_account_info().clone(),
                ],
                &[&[VAULT_SEED, &[*bump]]],
            )?;
        }

        let (_pool_account_seed, _bump) = Pubkey::find_program_address(&[GLOBAL_STATE_SEED.as_bytes()], ctx.program_id);
        let pool_seeds = &[GLOBAL_STATE_SEED.as_bytes(), &[_bump]];
        let signer = &[&pool_seeds[..]];

        // withdraw all woof tokens
        if woof_amount > 0 {
            // transfer
            let token_program = accts.token_program.to_account_info().clone();
            let token_accounts = anchor_spl::token::Transfer {
                from: accts.source_woof_account.to_account_info().clone(),
                to: accts.dest_woof_account.to_account_info().clone(),
                authority: accts.global_state.to_account_info().clone(),
            };
            let cpi_ctx = CpiContext::new(token_program, token_accounts);

            anchor_spl::token::transfer(cpi_ctx.with_signer(signer), woof_amount)?;
        }

        // withdraw all ticket tokens
        if ticket_amount > 0 {
            // transfer
            let token_program = accts.token_program.to_account_info().clone();
            let token_accounts = anchor_spl::token::Transfer {
                from: accts.source_ticket_account.to_account_info().clone(),
                to: accts.dest_ticket_account.to_account_info().clone(),
                authority: accts.global_state.to_account_info().clone(),
            };
            let cpi_ctx = CpiContext::new(token_program, token_accounts);

            anchor_spl::token::transfer(cpi_ctx.with_signer(signer), ticket_amount)?;
        }

        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        seeds = [GLOBAL_STATE_SEED.as_bytes()],
        bump,
        space = 8 + size_of::<GlobalState>(),
        payer = admin,
    )]
    pub global_state: Account<'info, GlobalState>,

    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump
    )]
    /// CHECK: this should be checked with vault address
    pub vault: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct CoinFlip<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    /// CHECK: We're reading data from this chainlink feed account
    pub chainlink_feed: AccountInfo<'info>,

    /// CHECK: This is the Chainlink program library
    pub chainlink_program: AccountInfo<'info>,

    #[account(
        mut,
        seeds = [GLOBAL_STATE_SEED.as_bytes()],
        bump,
    )]
    pub global_state: Account<'info, GlobalState>,

    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
        address = global_state.vault
    )]
    /// CHECK: this should be checked with vault address
    pub vault: AccountInfo<'info>,

    /// CHECK: This is not dangerous because we don't read or write from this account
    #[account(mut,owner=spl_token::id())]
    pub source_account : AccountInfo<'info>,

    /// CHECK: This is not dangerous because we don't read or write from this account
    #[account(mut,owner=spl_token::id())]
    pub dest_account : AccountInfo<'info>,

    #[account(
        init_if_needed,
        seeds = [USER_STATE_SEED, user.key().as_ref()],
        bump,
        payer = user,
        space = 8 + size_of::<UserState>()
    )]
    pub user_state: Account<'info, UserState>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}


#[derive(Accounts)]
pub struct DepositReward<'info> {
    #[account(mut)]
    pub funder: Signer<'info>,

    #[account(
        mut,
        seeds = [GLOBAL_STATE_SEED.as_bytes()],
        bump,
    )]
    pub global_state: Account<'info, GlobalState>,

    /// CHECK: This is not dangerous because we don't read or write from this account
    #[account(mut,owner=spl_token::id())]
    pub source_account : AccountInfo<'info>,

    /// CHECK: This is not dangerous because we don't read or write from this account
    #[account(mut,owner=spl_token::id())]
    pub dest_account : AccountInfo<'info>,

    // The Token Program
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ClaimReward<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [GLOBAL_STATE_SEED.as_bytes()],
        bump,
    )]
    pub global_state: Account<'info, GlobalState>,

    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump
    )]
    /// CHECK: this should be checked with address in global_state
    pub vault: AccountInfo<'info>,

    #[account(
        mut,
        seeds = [USER_STATE_SEED, user.key().as_ref()],
        bump,
        constraint = user_state.user == user.key()
    )]
    pub user_state: Account<'info, UserState>,

    /// CHECK: This is not dangerous because we don't read or write from this account
    #[account(mut,owner=spl_token::id())]
    pub source_account : AccountInfo<'info>,

    /// CHECK: This is not dangerous because we don't read or write from this account
    #[account(mut,owner=spl_token::id())]
    pub dest_account : AccountInfo<'info>,

    // The Token Program
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct WithdrawAll<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [GLOBAL_STATE_SEED.as_bytes()],
        bump,
        constraint = global_state.admin == admin.key()
    )]
    pub global_state: Account<'info, GlobalState>,

    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump
    )]
    /// CHECK: this should be checked with address in global_state
    pub vault: AccountInfo<'info>,

    /// CHECK: This is not dangerous because we don't read or write from this account
    #[account(mut,owner=spl_token::id())]
    pub source_woof_account : AccountInfo<'info>,

    /// CHECK: This is not dangerous because we don't read or write from this account
    #[account(mut,owner=spl_token::id())]
    pub dest_woof_account : AccountInfo<'info>,

    /// CHECK: This is not dangerous because we don't read or write from this account
    #[account(mut,owner=spl_token::id())]
    pub source_ticket_account : AccountInfo<'info>,

    /// CHECK: This is not dangerous because we don't read or write from this account
    #[account(mut,owner=spl_token::id())]
    pub dest_ticket_account : AccountInfo<'info>,

    // The Token Program
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct BuyWoofToken<'info> {
    #[account(mut)]
    buyer: Signer<'info>,

    #[account(
        mut,
        seeds = [GLOBAL_STATE_SEED.as_bytes()],
        bump,
    )]
    pub global_state: Account<'info, GlobalState>,

    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump
    )]
    /// CHECK: this should be checked with address in global_state
    pub vault: AccountInfo<'info>,

    #[account(mut)]
    /// CHECK: this should be checked with address in global_state
    pub source_account: AccountInfo<'info>,

    // funder account
    #[account(mut)]
    /// CHECK: this should be checked with address in global_state
    pub dest_account: AccountInfo<'info>,

    // The Token Program
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}
