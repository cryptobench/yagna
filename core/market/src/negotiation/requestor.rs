use chrono::{DateTime, Utc};
use futures::stream::StreamExt;
use metrics::counter;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;

use ya_client::model::market::{event::RequestorEvent, NewProposal, Reason};
use ya_client::model::{node_id::ParseError, NodeId};
use ya_persistence::executor::DbExecutor;
use ya_service_api_web::middleware::Identity;

use crate::db::{
    dao::{AgreementDao, AgreementDaoError, SaveAgreementError},
    model::{Agreement, AgreementId, AgreementState, AppSessionId},
    model::{Demand, Issuer, Owner, ProposalId, SubscriptionId},
};
use crate::matcher::{store::SubscriptionStore, RawProposal};
use crate::protocol::negotiation::{error::*, messages::*, requestor::NegotiationApi};

use super::{common::*, error::*, notifier::NotifierError, EventNotifier};
use crate::config::Config;
use crate::utils::display::EnableDisplay;

#[derive(Clone, derive_more::Display, Debug, PartialEq)]
pub enum ApprovalStatus {
    #[display(fmt = "Approved")]
    Approved,
    #[display(fmt = "Cancelled")]
    Cancelled,
    #[display(fmt = "Rejected")]
    Rejected,
}

/// Requestor part of negotiation logic.
pub struct RequestorBroker {
    pub(crate) common: CommonBroker,
    api: NegotiationApi,
}

impl RequestorBroker {
    pub fn new(
        db: DbExecutor,
        store: SubscriptionStore,
        proposal_receiver: UnboundedReceiver<RawProposal>,
        session_notifier: EventNotifier<AppSessionId>,
        config: Arc<Config>,
    ) -> Result<RequestorBroker, NegotiationInitError> {
        let broker = CommonBroker::new(db.clone(), store, session_notifier, config);

        let broker1 = broker.clone();
        let broker2 = broker.clone();
        let broker_proposal_reject = broker.clone();
        let broker_terminated = broker.clone();

        let api = NegotiationApi::new(
            move |caller: String, msg: ProposalReceived| {
                broker1
                    .clone()
                    .on_proposal_received(msg, caller, Owner::Provider)
            },
            move |caller: String, msg: ProposalRejected| {
                broker_proposal_reject
                    .clone()
                    .on_proposal_rejected(msg, caller, Owner::Provider)
            },
            move |caller: String, msg: AgreementApproved| {
                on_agreement_approved(broker2.clone(), caller, msg)
            },
            move |_caller: String, _msg: AgreementRejected| async move {
                counter!("market.agreements.requestor.rejected", 1);
                unimplemented!()
            },
            move |caller: String, msg: AgreementTerminated| {
                broker_terminated
                    .clone()
                    .on_agreement_terminated(msg, caller, Owner::Provider)
            },
        );

        let engine = RequestorBroker {
            api,
            common: broker.clone(),
        };

        // Initialize counters to 0 value. Otherwise they won't appear on metrics endpoint
        // until first change to value will be made.
        counter!("market.agreements.events.queried", 0);
        counter!("market.agreements.requestor.approved", 0);
        counter!("market.agreements.requestor.cancelled", 0);
        counter!("market.agreements.requestor.confirmed", 0);
        counter!("market.agreements.requestor.created", 0);
        counter!("market.agreements.requestor.rejected", 0);
        counter!("market.agreements.requestor.terminated", 0);
        counter!("market.agreements.requestor.terminated.reason", 0, "reason" => "NotSpecified");
        counter!("market.agreements.requestor.terminated.reason", 0, "reason" => "Success");
        counter!("market.events.requestor.queried", 0);
        counter!("market.proposals.requestor.countered", 0);
        counter!("market.proposals.requestor.generated", 0);
        counter!("market.proposals.requestor.received", 0);
        counter!("market.proposals.requestor.rejected.initial", 0);
        counter!("market.proposals.requestor.rejected.by-them", 0);
        counter!("market.proposals.requestor.rejected.by-us", 0);
        counter!("market.proposals.self-reaction-attempt", 0);

        tokio::spawn(proposal_receiver_thread(broker, proposal_receiver));
        Ok(engine)
    }

    pub async fn bind_gsb(
        &self,
        public_prefix: &str,
        local_prefix: &str,
    ) -> Result<(), NegotiationInitError> {
        self.api.bind_gsb(public_prefix, local_prefix).await?;
        Ok(())
    }

    pub async fn subscribe_demand(&self, _demand: &Demand) -> Result<(), NegotiationError> {
        // TODO: Implement
        Ok(())
    }

    pub async fn unsubscribe_demand(&self, id: &SubscriptionId) -> Result<(), NegotiationError> {
        self.common.unsubscribe(id).await
    }

    pub async fn counter_proposal(
        &self,
        demand_id: &SubscriptionId,
        prev_proposal_id: &ProposalId,
        proposal: &NewProposal,
        id: &Identity,
    ) -> Result<ProposalId, ProposalError> {
        let (new_proposal, is_first) = self
            .common
            .counter_proposal(
                demand_id,
                prev_proposal_id,
                proposal,
                &id.identity,
                Owner::Requestor,
            )
            .await?;

        let proposal_id = new_proposal.body.id.clone();
        // Send Proposal to Provider. Note that it can be either our first communication with
        // Provider or we negotiated with him already, so we need to send different message in each
        // of these cases.
        match is_first {
            true => self.api.initial_proposal(new_proposal).await,
            false => self.api.counter_proposal(new_proposal).await,
        }
        .map_err(|e| ProposalError::Send(prev_proposal_id.clone(), e))?;

        counter!("market.proposals.requestor.countered", 1);
        log::info!(
            "Requestor {} countered Proposal [{}] with [{}]",
            id.display(),
            &prev_proposal_id,
            &proposal_id
        );
        Ok(proposal_id)
    }

    pub async fn reject_proposal(
        &self,
        demand_id: &SubscriptionId,
        proposal_id: &ProposalId,
        id: &Identity,
        reason: Option<Reason>,
    ) -> Result<(), ProposalError> {
        let proposal = self
            .common
            .reject_proposal(
                Some(demand_id),
                proposal_id,
                &id.identity,
                Owner::Requestor,
                &reason,
            )
            .await?;

        self.api
            .reject_proposal(id.identity, &proposal, reason.clone())
            .await?;

        counter!("market.proposals.requestor.rejected.by-us", 1);

        Ok(())
    }

    pub async fn query_events(
        &self,
        demand_id: &SubscriptionId,
        timeout: f32,
        max_events: Option<i32>,
    ) -> Result<Vec<RequestorEvent>, QueryEventsError> {
        let events = self
            .common
            .query_events(demand_id, timeout, max_events, Owner::Requestor)
            .await?;

        // Map model events to client RequestorEvent.
        let events = futures::stream::iter(events)
            .then(|event| event.into_client_requestor_event(&self.common.db))
            .inspect(|result| {
                if let Err(error) = result {
                    log::error!("Error converting event to client type: {}", error);
                }
            })
            .filter_map(|event| async move { event.ok() })
            .collect::<Vec<RequestorEvent>>()
            .await;

        counter!("market.events.requestor.queried", events.len() as u64);
        Ok(events)
    }

    /// Initiates the Agreement handshake phase.
    ///
    /// Formulates an Agreement artifact from the Proposal indicated by the
    /// received Proposal Id.
    ///
    /// The Approval Expiry Date is added to Agreement artifact and implies
    /// the effective timeout on the whole Agreement Confirmation sequence.
    ///
    /// A successful call to `create_agreement` shall immediately be followed
    /// by a `confirm_agreement` and `wait_for_approval` call in order to listen
    /// for responses from the Provider.
    pub async fn create_agreement(
        &self,
        id: Identity,
        proposal_id: &ProposalId,
        valid_to: DateTime<Utc>,
    ) -> Result<AgreementId, AgreementError> {
        let offer_proposal_id = proposal_id;
        let offer_proposal = self
            .common
            .get_proposal(None, offer_proposal_id)
            .await
            .map_err(|e| AgreementError::from_proposal(proposal_id, e))?;

        // We can promote only Proposals, that we got from Provider.
        // Can't promote our own Proposal.
        if offer_proposal.body.issuer != Issuer::Them {
            return Err(AgreementError::OwnProposal(proposal_id.clone()));
        }

        let demand_proposal_id = offer_proposal
            .body
            .prev_proposal_id
            .clone()
            .ok_or_else(|| AgreementError::NoNegotiations(offer_proposal_id.clone()))?;
        let demand_proposal = self
            .common
            .get_proposal(None, &demand_proposal_id)
            .await
            .map_err(|e| AgreementError::from_proposal(proposal_id, e))?;

        let agreement = Agreement::new(
            demand_proposal,
            offer_proposal,
            valid_to.naive_utc(),
            Owner::Requestor,
        );
        let agreement_id = agreement.id.clone();
        self.common
            .db
            .as_dao::<AgreementDao>()
            .save(agreement)
            .await
            .map_err(|e| match e {
                SaveAgreementError::Internal(e) => AgreementError::Save(proposal_id.clone(), e),
                SaveAgreementError::ProposalCountered(id) => AgreementError::ProposalCountered(id),
                SaveAgreementError::Exists(agreement_id, proposal_id) => {
                    AgreementError::AlreadyExists(agreement_id, proposal_id)
                }
            })?;

        counter!("market.agreements.requestor.created", 1);
        log::info!(
            "Requestor {} created Agreement [{}] from Proposal [{}].",
            id.display(),
            &agreement_id,
            &proposal_id
        );
        Ok(agreement_id)
    }

    pub async fn wait_for_approval(
        &self,
        id: &AgreementId,
        timeout: f32,
    ) -> Result<ApprovalStatus, WaitForApprovalError> {
        // TODO: Check if we are owner of Proposal
        // TODO: What to do with 2 simultaneous calls to wait_for_approval??
        //  should we reject one? And if so, how to discover, that two calls were made?
        let timeout = Duration::from_secs_f32(timeout.max(0.0));
        let mut notifier = self.common.agreement_notifier.listen(id);

        // Loop will wait for events notifications only one time. It doesn't have to be loop at all,
        // but it spares us doubled getting agreement and mapping statuses to return results.
        // So I think this simplification is worth confusion, that it cause.
        loop {
            let agreement = self
                .common
                .db
                .as_dao::<AgreementDao>()
                .select(id, None, Utc::now().naive_utc())
                .await
                .map_err(|e| WaitForApprovalError::Get(id.clone(), e))?
                .ok_or(WaitForApprovalError::NotFound(id.clone()))?;

            match agreement.state {
                AgreementState::Approved => {
                    return Ok(ApprovalStatus::Approved);
                }
                AgreementState::Rejected => {
                    return Ok(ApprovalStatus::Rejected);
                }
                AgreementState::Cancelled => {
                    return Ok(ApprovalStatus::Cancelled);
                }
                AgreementState::Expired => return Err(WaitForApprovalError::Expired(id.clone())),
                AgreementState::Proposal => {
                    return Err(WaitForApprovalError::NotConfirmed(id.clone()))
                }
                AgreementState::Terminated => {
                    return Err(WaitForApprovalError::Terminated(id.clone()))
                }
                AgreementState::Pending => (), // Still waiting for approval.
            };

            if let Err(error) = notifier.wait_for_event_with_timeout(timeout).await {
                return match error {
                    NotifierError::Timeout(_) => Err(WaitForApprovalError::Timeout(id.clone())),
                    NotifierError::ChannelClosed(_) => {
                        Err(WaitForApprovalError::Internal(error.to_string()))
                    }
                    NotifierError::Unsubscribed(_) => Ok(ApprovalStatus::Cancelled),
                };
            }
        }
    }

    /// Signs (not yet) Agreement self-created via `create_agreement`
    /// and sends it to the Provider.
    pub async fn confirm_agreement(
        &self,
        id: Identity,
        agreement_id: &AgreementId,
        app_session_id: AppSessionId,
    ) -> Result<(), AgreementError> {
        let dao = self.common.db.as_dao::<AgreementDao>();
        {
            // We won't be able to process `on_agreement_approved`, before we
            // finish execution under this lock. This avoids errors related to
            // Provider approving Agreement before we set proper state in database.
            let _hold = self.common.agreement_lock.lock(&agreement_id).await;

            let agreement = match dao
                .select(
                    agreement_id,
                    Some(id.identity.clone()),
                    Utc::now().naive_utc(),
                )
                .await
                .map_err(|e| AgreementError::Get(agreement_id.to_string(), e))?
            {
                None => return Err(AgreementError::NotFound(agreement_id.to_string())),
                Some(agreement) => agreement,
            };

            validate_transition(&agreement, AgreementState::Pending)?;

            // TODO : possible race condition here ISSUE#430
            // 1. this state check should be also `db.update_state`
            // 2. `dao.confirm` must be invoked after successful propose_agreement
            self.api.propose_agreement(&agreement).await?;
            dao.confirm(agreement_id, &app_session_id)
                .await
                .map_err(|e| AgreementError::UpdateState(agreement_id.clone(), e))?;
        }

        counter!("market.agreements.requestor.confirmed", 1);
        log::info!(
            "Requestor {} confirmed Agreement [{}] and sent to Provider.",
            id.display(),
            &agreement_id,
        );
        if let Some(session) = app_session_id {
            log::info!(
                "AppSession id [{}] set for Agreement [{}].",
                &session,
                &agreement_id
            );
        }
        return Ok(());
    }
}

async fn on_agreement_approved(
    broker: CommonBroker,
    caller: String,
    msg: AgreementApproved,
) -> Result<(), ApproveAgreementError> {
    let caller: NodeId =
        caller
            .parse()
            .map_err(|e: ParseError| ApproveAgreementError::CallerParseError {
                e: e.to_string(),
                caller,
                id: msg.agreement_id.clone(),
            })?;
    Ok(agreement_approved(broker, caller, msg)
        .await
        .map_err(|e| ApproveAgreementError::Remote(e))?)
}

async fn agreement_approved(
    broker: CommonBroker,
    caller: NodeId,
    msg: AgreementApproved,
) -> Result<(), RemoteAgreementError> {
    // TODO: We should check as many conditions here, as possible, because we want
    //  to return meaningful message to Provider, what is impossible from `commit_agreement`.
    let agreement = {
        // We aren't sure, if `confirm_agreement` execution is finished,
        // so we must lock, to avoid attempt to change database state before.
        let _hold = broker.agreement_lock.lock(&msg.agreement_id).await;

        let agreement = broker
            .db
            .as_dao::<AgreementDao>()
            .select(&msg.agreement_id, None, Utc::now().naive_utc())
            .await
            .map_err(|_e| RemoteAgreementError::NotFound(msg.agreement_id.clone()))?
            .ok_or(RemoteAgreementError::NotFound(msg.agreement_id.clone()))?;

        if agreement.provider_id != caller {
            // Don't reveal, that we know this Agreement id.
            Err(RemoteAgreementError::NotFound(msg.agreement_id.clone()))?
        }

        // TODO: Validate Agreement `valid_to` timestamp. In previous version we got
        //  error from database update, but know we want to escape early, because
        //  otherwise we can't response with this error to Provider.
        validate_transition(&agreement, AgreementState::Approving)?;

        // TODO: Validate agreement signature.
        // TODO: Sign Agreement and update `approved_signature`.
        // TODO: Update `approved_ts` Agreement field.
        // TODO: Update state to `AgreementState::Approving`.

        agreement
    };

    /// TODO: Commit Agreement. We must spawn committing later, because we need to
    ///  return from this function to provider.
    tokio::task::spawn_local(commit_agreement(broker, agreement.id));
    log::info!(
        "Agreement [{}] approved by [{}].",
        &agreement.id,
        &agreement.provider_id
    );
    Ok(())
}

async fn commit_agreement(broker: CommonBroker, agreement_id: AgreementId) {
    // Note: in this scenario, we update database after Provider already
    // got `AgreementCommitted` and updated Agreement state to `Approved`, so we will
    // wake up `wait_for_agreement` after Provider.
    let agreement = match {
        let _hold = broker.agreement_lock.lock(&agreement_id).await;

        let dao = broker.db.as_dao::<AgreementDao>();
        let agreement = dao
            .select(&msg.agreement_id, None, Utc::now().naive_utc())
            .await
            .map_err(|_e| RemoteAgreementError::NotFound(msg.agreement_id.clone()))?
            .ok_or(RemoteAgreementError::NotFound(msg.agreement_id.clone()))?;

        // TODO: Send `AgreementCommited` message to Provider.
        // TODO: We have problem to make GSB call here, since we don't have `api` object.
        //  we must place this function in `protocol/negotiation/common`

        // Note: This GSB call is racing with potential `cancel_agreement` call.
        // In this case Provider code will decide, which call won the race.

        // Note: session must be None, because either we already set this value in ConfirmAgreement,
        // or we purposely left it None.
        dao.approve(&agreement_id, &None)
            .await
            .map_err(|err| match err {
                AgreementDaoError::InvalidTransition { from, .. } => {
                    match from {
                        // Expired Agreement could be InvalidState either, but we want to explicit
                        // say to provider, that Agreement has expired.
                        AgreementState::Expired => {
                            RemoteAgreementError::Expired(agreement_id.clone())
                        }
                        _ => RemoteAgreementError::InvalidState(agreement_id.clone(), from),
                    }
                }
                e => {
                    // Log our internal error, but don't reveal error message to Provider.
                    log::warn!(
                        "Approve Agreement [{}] internal error: {}",
                        &agreement_id,
                        e
                    );
                    RemoteAgreementError::InternalError(agreement_id.clone())
                }
            })?;
        Ok(agreement)
    } {
        Ok(agreement) => agreement,
        Err(e) => {
            // TODO: Return to pending state here.
        }
    };

    broker.notify_agreement(&agreement).await;

    counter!("market.agreements.requestor.approved", 1);
    log::info!(
        "Agreement [{}] approved (committed) by [{}].",
        &agreement.id,
        &agreement.provider_id
    );
}

pub async fn proposal_receiver_thread(
    broker: CommonBroker,
    mut proposal_receiver: UnboundedReceiver<RawProposal>,
) {
    while let Some(proposal) = proposal_receiver.recv().await {
        let broker = broker.clone();
        match async move {
            log::debug!("Got matching Offer-Demand pair; emitting as Proposal to Requestor.");
            broker.generate_proposal(proposal).await
        }
        .await
        {
            Err(error) => log::warn!("Failed to add proposal. Error: {}", error),
            Ok(_) => (),
        }
    }
}
