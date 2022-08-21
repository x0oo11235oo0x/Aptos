// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

pub mod account_minter;
pub mod stats;
pub mod submission_worker;

use ::aptos_logger::*;
use again::RetryPolicy;
use anyhow::{anyhow, format_err, Result};
use aptos_rest_client::Client as RestClient;
use aptos_sdk::{
    move_types::account_address::AccountAddress,
    transaction_builder::TransactionFactory,
    types::{transaction::SignedTransaction, LocalAccount},
};
use futures::future::{try_join_all, FutureExt};
use itertools::zip;
use once_cell::sync::Lazy;
use rand::prelude::SliceRandom;
use rand_core::SeedableRng;
use std::{
    cmp::{max, min},
    collections::HashSet,
    num::NonZeroU64,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, task::JoinHandle, time};

use crate::{
    args::TransactionType,
    emitter::{account_minter::AccountMinter, submission_worker::SubmissionWorker},
    transaction_generator::{
        account_generator::AccountGeneratorCreator, nft_mint::NFTMintGeneratorCreator,
        p2p_transaction_generator::P2PTransactionGeneratorCreator, TransactionGeneratorCreator,
    },
};
use aptos_sdk::transaction_builder::aptos_stdlib;
use rand::rngs::StdRng;
use stats::{StatsAccumulator, TxnStats};

/// Max transactions per account in mempool
const MAX_TXN_BATCH_SIZE: usize = 100;
const TRANSACTIONS_PER_ACCOUNT: usize = 5;
const MAX_TXNS: u64 = 1_000_000;
const SEND_AMOUNT: u64 = 1;
const TXN_EXPIRATION_SECONDS: u64 = 180;
const TXN_MAX_WAIT: Duration = Duration::from_secs(TXN_EXPIRATION_SECONDS as u64 + 30);

// This retry policy is used for important client calls necessary for setting
// up the test (e.g. account creation) and collecting its results (e.g. checking
// account sequence numbers). If these fail, the whole test fails. We do not use
// this for submitting transactions, as we have a way to handle when that fails.
// This retry policy means an operation will take 8 seconds at most.
static RETRY_POLICY: Lazy<RetryPolicy> = Lazy::new(|| {
    RetryPolicy::exponential(Duration::from_millis(125))
        .with_max_retries(6)
        .with_jitter(true)
});

#[derive(Clone, Debug)]
pub struct EmitThreadParams {
    pub wait_millis: u64,
    pub wait_committed: bool,
    pub txn_expiration_time_secs: u64,
    pub check_stats_at_end: bool,
}

impl Default for EmitThreadParams {
    fn default() -> Self {
        Self {
            wait_millis: 0,
            wait_committed: true,
            txn_expiration_time_secs: 300,
            check_stats_at_end: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EmitJobRequest {
    rest_clients: Vec<RestClient>,
    mempool_backlog: usize,
    thread_params: EmitThreadParams,
    gas_price: u64,
    invalid_transaction_ratio: usize,
    pub duration: Duration,
    reuse_accounts: bool,
    transaction_type: TransactionType,
}

impl Default for EmitJobRequest {
    fn default() -> Self {
        Self {
            rest_clients: Vec::new(),
            mempool_backlog: 3000,
            thread_params: EmitThreadParams::default(),
            gas_price: 0,
            invalid_transaction_ratio: 0,
            duration: Duration::from_secs(300),
            reuse_accounts: false,
            transaction_type: TransactionType::P2P,
        }
    }
}

impl EmitJobRequest {
    pub fn new(rest_clients: Vec<RestClient>) -> Self {
        Self::default().rest_clients(rest_clients)
    }

    pub fn rest_clients(mut self, rest_clients: Vec<RestClient>) -> Self {
        self.rest_clients = rest_clients;
        self
    }

    pub fn thread_params(mut self, thread_params: EmitThreadParams) -> Self {
        self.thread_params = thread_params;
        self
    }

    pub fn gas_price(mut self, gas_price: u64) -> Self {
        self.gas_price = gas_price;
        self
    }

    pub fn invalid_transaction_ratio(mut self, invalid_transaction_ratio: usize) -> Self {
        self.invalid_transaction_ratio = invalid_transaction_ratio;
        self
    }

    pub fn transaction_type(mut self, transaction_type: TransactionType) -> Self {
        self.transaction_type = transaction_type;
        self
    }

    pub fn calculate_workers_per_endpoint(&self) -> usize {
        // The target mempool backlog is set to be 3x of the target TPS because of the on an average,
        // we can ~3 blocks in consensus queue. As long as we have 3x the target TPS as backlog,
        // it should be enough to produce the target TPS.
        let clients_count = self.rest_clients.len();
        let num_workers_per_endpoint = max(
            self.mempool_backlog / (clients_count * TRANSACTIONS_PER_ACCOUNT),
            1,
        );

        info!(
            " Transaction emitter target mempool backlog is {}",
            self.mempool_backlog
        );

        info!(
            " Will use {} clients and {} workers per client",
            clients_count, num_workers_per_endpoint
        );

        num_workers_per_endpoint
    }

    pub fn mempool_backlog(mut self, mempool_backlog: NonZeroU64) -> Self {
        self.mempool_backlog = mempool_backlog.get() as usize;
        self
    }

    pub fn reuse_accounts(mut self) -> Self {
        self.reuse_accounts = true;
        self
    }

    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }
}

#[derive(Debug)]
struct Worker {
    join_handle: JoinHandle<Vec<LocalAccount>>,
}

#[derive(Debug)]
pub struct EmitJob {
    workers: Vec<Worker>,
    stop: Arc<AtomicBool>,
    stats: Arc<StatsAccumulator>,
}

#[derive(Debug)]
pub struct TxnEmitter<'t> {
    accounts: Vec<LocalAccount>,
    txn_factory: TransactionFactory,
    client: RestClient,
    rng: StdRng,
    root_account: &'t mut LocalAccount,
}

impl<'t> TxnEmitter<'t> {
    pub fn new(
        root_account: &'t mut LocalAccount,
        client: RestClient,
        transaction_factory: TransactionFactory,
        rng: StdRng,
    ) -> Self {
        Self {
            accounts: vec![],
            txn_factory: transaction_factory,
            root_account,
            client,
            rng,
        }
    }

    pub fn take_account(&mut self) -> LocalAccount {
        self.accounts.remove(0)
    }

    pub fn clear(&mut self) {
        self.accounts.clear();
    }

    pub fn rng(&mut self) -> &mut StdRng {
        &mut self.rng
    }

    pub fn from_rng(&mut self) -> StdRng {
        StdRng::from_rng(self.rng()).unwrap()
    }

    pub async fn get_money_source(&mut self, coins_total: u64) -> Result<&mut LocalAccount> {
        let client = self.client.clone();
        info!("Creating and minting faucet account");
        let faucet_account = &mut self.root_account;
        let balance = client
            .get_account_balance(faucet_account.address())
            .await?
            .into_inner();
        info!(
            "Root account current balances are {}, requested {} coins",
            balance.get(),
            coins_total
        );
        Ok(faucet_account)
    }

    pub async fn start_job(&mut self, req: EmitJobRequest) -> Result<EmitJob> {
        let workers_per_endpoint = req.calculate_workers_per_endpoint();
        let num_accounts = req.rest_clients.len() * workers_per_endpoint;
        info!(
            "Will use {} workers per endpoint for a total of {} endpoint clients",
            workers_per_endpoint, num_accounts
        );
        info!("Will create a total of {} accounts", num_accounts);
        let mut account_minter = AccountMinter::new(
            self.root_account,
            self.txn_factory.clone(),
            self.rng.clone(),
        );
        let mut new_accounts = account_minter.mint_accounts(&req, num_accounts).await?;
        self.accounts.append(&mut new_accounts);
        let all_accounts = self.accounts.split_off(self.accounts.len() - num_accounts);
        let mut workers = vec![];
        let all_addresses: Vec<_> = all_accounts.iter().map(|d| d.address()).collect();
        let all_addresses = Arc::new(all_addresses);
        let mut all_accounts = all_accounts.into_iter();
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(StatsAccumulator::default());
        let tokio_handle = Handle::current();
        let txn_generator_creator: Box<dyn TransactionGeneratorCreator> = match req.transaction_type
        {
            TransactionType::P2P => Box::new(P2PTransactionGeneratorCreator::new(
                self.from_rng(),
                self.txn_factory.clone(),
                SEND_AMOUNT,
            )),
            TransactionType::AccountGeneration => {
                Box::new(AccountGeneratorCreator::new(self.txn_factory.clone()))
            }
            TransactionType::NftMint => Box::new(
                NFTMintGeneratorCreator::new(
                    self.from_rng(),
                    self.txn_factory.clone(),
                    self.root_account,
                    req.rest_clients[0].clone(),
                )
                .await,
            ),
        };
        for client in req.rest_clients {
            for _ in 0..workers_per_endpoint {
                let accounts = (&mut all_accounts).take(1).collect();
                let all_addresses = all_addresses.clone();
                let stop = stop.clone();
                let params = req.thread_params.clone();
                let stats = Arc::clone(&stats);

                let worker = SubmissionWorker::new(
                    accounts,
                    client.clone(),
                    all_addresses,
                    stop,
                    params,
                    stats,
                    txn_generator_creator.create_transaction_generator(),
                    req.invalid_transaction_ratio,
                    self.from_rng(),
                );
                let join_handle = tokio_handle.spawn(worker.run(req.gas_price).boxed());
                workers.push(Worker { join_handle });
            }
        }
        info!("Tx emitter workers started");
        Ok(EmitJob {
            workers,
            stop,
            stats,
        })
    }

    pub async fn stop_job(&mut self, job: EmitJob) -> TxnStats {
        job.stop.store(true, Ordering::Relaxed);
        for worker in job.workers {
            let mut accounts = worker
                .join_handle
                .await
                .expect("TxnEmitter worker thread failed");
            self.accounts.append(&mut accounts);
        }
        job.stats.accumulate()
    }

    pub fn peek_job_stats(&self, job: &EmitJob) -> TxnStats {
        job.stats.accumulate()
    }

    pub async fn periodic_stat(&mut self, job: &EmitJob, duration: Duration, interval_secs: u64) {
        let deadline = Instant::now() + duration;
        let mut prev_stats: Option<TxnStats> = None;
        let window = Duration::from_secs(min(interval_secs, 1));
        while Instant::now() < deadline {
            tokio::time::sleep(window).await;
            let stats = self.peek_job_stats(job);
            let delta = &stats - &prev_stats.unwrap_or_default();
            prev_stats = Some(stats);
            info!("{}", delta.rate(window));
        }
    }

    pub async fn emit_txn_for(&mut self, emit_job_request: EmitJobRequest) -> Result<TxnStats> {
        let duration = emit_job_request.duration;
        let job = self.start_job(emit_job_request).await?;
        info!("Starting emitting txns for {} secs", duration.as_secs());
        time::sleep(duration).await;
        info!("Ran for {} secs, stopping job...", duration.as_secs());
        let stats = self.stop_job(job).await;
        info!("Stopped job");
        Ok(stats)
    }

    pub async fn emit_txn_for_with_stats(
        &mut self,
        emit_job_request: EmitJobRequest,
        interval_secs: u64,
    ) -> Result<TxnStats> {
        let duration = emit_job_request.duration;
        info!("Starting emitting txns for {} secs", duration.as_secs());
        let job = self.start_job(emit_job_request).await?;
        self.periodic_stat(&job, duration, interval_secs).await;
        info!("Ran for {} secs, stopping job...", duration.as_secs());
        let stats = self.stop_job(job).await;
        info!("Stopped job");
        Ok(stats)
    }

    pub async fn submit_single_transaction(
        &self,
        client: &RestClient,
        sender: &mut LocalAccount,
        receiver: &AccountAddress,
        num_coins: u64,
    ) -> Result<Instant> {
        client
            .submit(&gen_transfer_txn_request(
                sender,
                receiver,
                num_coins,
                &self.txn_factory,
                1,
            ))
            .await?;
        let deadline = Instant::now() + TXN_MAX_WAIT;
        Ok(deadline)
    }
}

/// Waits for a single account to catch up to the expected sequence number
async fn wait_for_single_account_sequence(
    client: &RestClient,
    account: &LocalAccount,
    wait_timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + wait_timeout;
    while Instant::now() <= deadline {
        time::sleep(Duration::from_millis(1000)).await;
        match query_sequence_numbers(client, &[account.address()]).await {
            Ok(sequence_numbers) => {
                if sequence_numbers[0] >= account.sequence_number() {
                    return Ok(());
                }
            }
            Err(e) => {
                info!(
                    "Failed to query sequence number for account {:?} for instance {:?} : {:?}",
                    account, client, e
                );
            }
        }
    }
    Err(anyhow!(
        "Timed out waiting for single account {:?} sequence number for instance {:?}",
        account,
        client
    ))
}

/// This function waits for the submitted transactions to be committed, up to
/// a deadline. If some accounts still have uncommitted transactions when we
/// hit the deadline, we return a map of account to the info about the number
/// of committed transactions, based on the delta between the local sequence
/// number and the actual sequence number returned by the account. Note, this
/// can return possibly unexpected results if the emitter was emitting more
/// transactions per account than the mempool limit of the accounts on the node.
/// As it is now, the sequence number of the local account incrememnts regardless
/// of whether the transaction is accepted into the node's mempool or not. So the
/// local sequence number could be much higher than the real sequence number ever
/// will be, since not all of the submitted transactions were accepted.
/// TODO, investigate whether this behaviour is desirable.
async fn wait_for_accounts_sequence(
    client: &RestClient,
    accounts: &mut [LocalAccount],
    wait_timeout: Duration,
    rng: &mut StdRng,
) -> Result<(), HashSet<AccountAddress>> {
    let deadline = Instant::now() + wait_timeout;
    let addresses: Vec<_> = accounts.iter().map(|d| d.address()).collect();
    let mut uncommitted = addresses.clone().into_iter().collect::<HashSet<_>>();

    // Choose a random account and wait for its sequence number to be up to date. After that, we can
    // query the all the accounts. This will help us ensure we don't hammer the REST API with too many
    // query for all the accounts.
    let account = accounts.choose(rng).expect("accounts can't be empty");
    if wait_for_single_account_sequence(client, account, wait_timeout)
        .await
        .is_err()
    {
        return Err(uncommitted);
    }

    // Special case for single account
    if accounts.len() == 1 {
        return Ok(());
    }

    while Instant::now() <= deadline {
        match query_sequence_numbers(client, &addresses).await {
            Ok(sequence_numbers) => {
                for (account, sequence_number) in zip(accounts.iter(), &sequence_numbers) {
                    if account.sequence_number() == *sequence_number {
                        uncommitted.remove(&account.address());
                    }
                }

                if uncommitted.is_empty() {
                    return Ok(());
                }
            }
            Err(e) => {
                info!(
                    "Failed to query ledger info on accounts {:?} for instance {:?} : {:?}",
                    addresses, client, e
                );
            }
        }
        time::sleep(Duration::from_millis(1000)).await;
    }

    Err(uncommitted)
}

pub async fn query_sequence_numbers(
    client: &RestClient,
    addresses: &[AccountAddress],
) -> Result<Vec<u64>> {
    Ok(try_join_all(
        addresses
            .iter()
            .map(|address| RETRY_POLICY.retry(move || client.get_account(*address))),
    )
    .await
    .map_err(|e| format_err!("Get accounts failed: {}", e))?
    .into_iter()
    .map(|resp| resp.into_inner().sequence_number)
    .collect())
}

pub fn gen_transfer_txn_request(
    sender: &mut LocalAccount,
    receiver: &AccountAddress,
    num_coins: u64,
    txn_factory: &TransactionFactory,
    gas_price: u64,
) -> SignedTransaction {
    sender.sign_with_transaction_builder(
        txn_factory
            .payload(aptos_stdlib::aptos_coin_transfer(*receiver, num_coins))
            .gas_unit_price(gas_price),
    )
}
