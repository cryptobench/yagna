/*
    ZksyncDriver to handle payments on the zksync network.

    Please limit the logic in this file, use local mods to handle the calls.
*/
// Extrnal crates
use chrono::{Duration, TimeZone, Utc};
use lazy_static::lazy_static;
use maplit::hashmap;
use serde_json;
use std::collections::HashMap;
use uuid::Uuid;

// Workspace uses
use ya_payment_driver::{
    account::{Accounts, AccountsRc},
    bus,
    cron::PaymentDriverCron,
    dao::DbExecutor,
    db::models::PaymentEntity,
    driver::{async_trait, BigDecimal, IdentityError, IdentityEvent, Network, PaymentDriver},
    model::*,
    utils,
};
use ya_utils_futures::timeout::IntoTimeoutFuture;

// Local uses
use crate::{
    dao::ZksyncDao, zksync::wallet, DEFAULT_NETWORK, DEFAULT_PLATFORM, DEFAULT_TOKEN, DRIVER_NAME,
};

lazy_static! {
    static ref TX_SUMBIT_TIMEOUT: Duration = Duration::minutes(15);
}

pub struct ZksyncDriver {
    active_accounts: AccountsRc,
    dao: ZksyncDao,
}

impl ZksyncDriver {
    pub fn new(db: DbExecutor) -> Self {
        Self {
            active_accounts: Accounts::new_rc(),
            dao: ZksyncDao::new(db),
        }
    }

    pub async fn load_active_accounts(&self) {
        log::debug!("load_active_accounts");
        let mut accounts = self.active_accounts.borrow_mut();
        let unlocked_accounts = bus::list_unlocked_identities().await.unwrap();
        for account in unlocked_accounts {
            log::debug!("account={}", account);
            accounts.add_account(account)
        }
    }

    fn is_account_active(&self, address: &str) -> bool {
        self.active_accounts
            .as_ref()
            .borrow()
            .get_node_id(address)
            .is_some()
    }

    async fn process_payments_for_account(&self, node_id: &str) {
        log::trace!("Processing payments for node_id={}", node_id);
        let payments: Vec<PaymentEntity> = self.dao.get_pending_payments(node_id).await;
        let mut nonce = 0;
        if !payments.is_empty() {
            log::info!(
                "Processing {} Payments for node_id={}",
                payments.len(),
                node_id
            );
            nonce = wallet::get_nonce(node_id).await;
            log::debug!("Payments: nonce={}, details={:?}", &nonce, payments);
        }
        for payment in payments {
            self.handle_payment(payment, &mut nonce).await;
        }
    }

    async fn handle_payment(&self, payment: PaymentEntity, nonce: &mut u32) {
        let details = utils::db_to_payment_details(&payment);
        let tx_nonce = nonce.to_owned();

        match wallet::make_transfer(&details, tx_nonce).await {
            Ok(tx_hash) => {
                let tx_id = self.dao.insert_transaction(&details, Utc::now()).await;
                self.dao
                    .transaction_sent(&tx_id, &tx_hash, &payment.order_id)
                    .await;
                *nonce += 1;
            }
            Err(e) => {
                let deadline =
                    Utc.from_utc_datetime(&payment.payment_due_date) + *TX_SUMBIT_TIMEOUT;
                if Utc::now() > deadline {
                    log::error!("Failed to submit zkSync transaction. Retry deadline reached. details={:?} error={}", payment, e);
                    self.dao.payment_failed(&payment.order_id).await;
                } else {
                    log::warn!(
                        "Failed to submit zkSync transaction. Payment will be retried until {}. details={:?} error={}",
                        deadline, payment, e
                    );
                };
            }
        };
    }
}

#[async_trait(?Send)]
impl PaymentDriver for ZksyncDriver {
    async fn account_event(
        &self,
        _db: DbExecutor,
        _caller: String,
        msg: IdentityEvent,
    ) -> Result<(), IdentityError> {
        self.active_accounts.borrow_mut().handle_event(msg);
        Ok(())
    }

    async fn get_account_balance(
        &self,
        _db: DbExecutor,
        _caller: String,
        msg: GetAccountBalance,
    ) -> Result<BigDecimal, GenericError> {
        log::debug!("get_account_balance: {:?}", msg);

        let balance = wallet::account_balance(&msg.address()).await?;

        log::debug!("get_account_balance - result: {}", &balance);
        Ok(balance)
    }

    fn get_name(&self) -> String {
        DRIVER_NAME.to_string()
    }

    fn get_default_network(&self) -> String {
        DEFAULT_NETWORK.to_string()
    }

    fn get_networks(&self) -> HashMap<String, Network> {
        // TODO: Implement multi-network support

        hashmap! {
            DEFAULT_NETWORK.to_string() => Network {
                default_token: DEFAULT_TOKEN.to_string(),
                tokens: hashmap! {
                    DEFAULT_TOKEN.to_string() => DEFAULT_PLATFORM.to_string()
                }
            }
        }
    }

    fn recv_init_required(&self) -> bool {
        false
    }

    async fn get_transaction_balance(
        &self,
        _db: DbExecutor,
        _caller: String,
        msg: GetTransactionBalance,
    ) -> Result<BigDecimal, GenericError> {
        log::debug!("get_transaction_balance: {:?}", msg);
        //todo!()
        // TODO: Get real transaction balance
        Ok(BigDecimal::from(1_000_000_000_000_000_000u64))
    }

    async fn init(&self, _db: DbExecutor, _caller: String, msg: Init) -> Result<Ack, GenericError> {
        log::debug!("init: {:?}", msg);
        let address = msg.address().clone();

        // TODO: payment_api fails to start due to provider account not unlocked
        // if !self.is_account_active(&address) {
        //     return Err(GenericError::new("Can not init, account not active"));
        // }

        wallet::init_wallet(&msg)
            .timeout(Some(180))
            .await
            .map_err(GenericError::new)??;

        let mode = msg.mode();
        let network = DEFAULT_NETWORK; // TODO: Implement multi-network support
        let token = DEFAULT_TOKEN; // TODO: Implement multi-network support
        bus::register_account(self, &address, network, token, mode).await?;

        log::info!(
            "Initialised payment account. mode={:?}, address={}, driver={}, network={}, token={}",
            mode,
            &address,
            DRIVER_NAME,
            network,
            token
        );
        Ok(Ack {})
    }

    async fn schedule_payment(
        &self,
        _db: DbExecutor,
        _caller: String,
        msg: SchedulePayment,
    ) -> Result<String, GenericError> {
        log::debug!("schedule_payment: {:?}", msg);

        let sender = msg.sender().to_owned();
        if !self.is_account_active(&sender) {
            return Err(GenericError::new(
                "Can not schedule_payment, account not active",
            ));
        }

        let order_id = Uuid::new_v4().to_string();
        self.dao.insert_payment(&order_id, &msg).await;
        Ok(order_id)
    }

    async fn verify_payment(
        &self,
        _db: DbExecutor,
        _caller: String,
        msg: VerifyPayment,
    ) -> Result<PaymentDetails, GenericError> {
        log::debug!("verify_payment: {:?}", msg);
        // TODO: Get transaction details from zksync
        // let tx_hash = hex::encode(msg.confirmation().confirmation);
        // match wallet::check_tx(&tx_hash).await {
        //     Some(true) => Ok(wallet::build_payment_details(tx_hash)),
        //     Some(false) => Err(GenericError::new("Payment did not succeed")),
        //     None => Err(GenericError::new("Payment not ready to be checked")),
        // }
        from_confirmation(msg.confirmation())
    }

    async fn validate_allocation(
        &self,
        _db: DbExecutor,
        _caller: String,
        msg: ValidateAllocation,
    ) -> Result<bool, GenericError> {
        let account_balance = wallet::account_balance(&msg.address).await?;
        let total_allocated_amount: BigDecimal = msg
            .existing_allocations
            .into_iter()
            .map(|allocation| allocation.remaining_amount)
            .sum();
        Ok(msg.amount <= (account_balance - total_allocated_amount))
    }
}

#[async_trait(?Send)]
impl PaymentDriverCron for ZksyncDriver {
    async fn confirm_payments(&self) {
        let txs = self.dao.get_unconfirmed_txs().await;
        log::trace!("confirm_payments {:?}", txs);

        for tx in txs {
            log::trace!("checking tx {:?}", &tx);
            let tx_hash = match &tx.tx_hash {
                None => continue,
                Some(tx_hash) => tx_hash,
            };
            let tx_success = match wallet::check_tx(&tx_hash).await {
                None => continue, // Check_tx returns None when the result is unknown
                Some(tx_success) => tx_success,
            };

            let payments = self.dao.transaction_confirmed(&tx.tx_id).await;
            let order_ids: Vec<String> = payments
                .iter()
                .map(|payment| payment.order_id.clone())
                .collect();

            if let Err(err) = tx_success {
                log::error!(
                    "ZkSync transaction verification failed. tx_details={:?} error={}",
                    tx,
                    err
                );
                self.dao.transaction_failed(&tx.tx_id).await;
                for order_id in order_ids.iter() {
                    self.dao.payment_failed(order_id).await;
                }
                return;
            }

            // Create bespoke payment details:
            // - Sender + receiver are the same
            // - Date is always now
            // - Amount needs to be updated to total of all PaymentEntity's
            let mut details = utils::db_to_payment_details(&payments.first().unwrap());
            details.amount = payments
                .into_iter()
                .map(|payment| utils::db_amount_to_big_dec(payment.amount))
                .sum::<BigDecimal>();
            let tx_hash = to_confirmation(&details).unwrap();
            let platform = DEFAULT_PLATFORM; // TODO: Implement multi-network support
            if let Err(e) =
                bus::notify_payment(&self.get_name(), platform, order_ids, &details, tx_hash).await
            {
                log::error!("{}", e)
            };
        }
    }

    async fn process_payments(&self) {
        for node_id in self.active_accounts.borrow().list_accounts() {
            self.process_payments_for_account(&node_id).await;
        }
    }
}

// Used by the DummyDriver to have a 2 way conversion between details & confirmation
fn to_confirmation(details: &PaymentDetails) -> Result<Vec<u8>, GenericError> {
    Ok(serde_json::to_string(details)
        .map_err(GenericError::new)?
        .into_bytes())
}

fn from_confirmation(confirmation: PaymentConfirmation) -> Result<PaymentDetails, GenericError> {
    let json_str =
        std::str::from_utf8(confirmation.confirmation.as_slice()).map_err(GenericError::new)?;
    let details = serde_json::from_str(&json_str).map_err(GenericError::new)?;
    Ok(details)
}
