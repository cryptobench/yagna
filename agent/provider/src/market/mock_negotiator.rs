use ya_agent_offer_model::OfferDefinition;
use ya_model::market::{Agreement, Offer, Proposal};

use super::negotiator::Negotiator;
use crate::market::negotiator::{AgreementResponse, ProposalResponse};
use crate::payments::LinearPricingOffer;

use anyhow::Result;

#[derive(Debug)]
pub struct AcceptAllNegotiator;

impl Negotiator for AcceptAllNegotiator {
    fn create_offer(&mut self, offer: &OfferDefinition) -> Result<Offer> {
        let com_info = LinearPricingOffer::new()
            .add_coefficient("golem.usage.duration_sec", 0.01)
            .add_coefficient("golem.usage.cpu_sec", 0.2)
            .initial_cost(1.0)
            .interval(6.0)
            .build();

        let mut offer = offer.clone();
        offer.com_info = com_info;

        Ok(Offer::new(
            offer.clone().into_json(),
            "()"
                //r#"(&
                //    (golem.srv.comp.wasm.task_package=http://34.244.4.185:8000/rust-wasi-tutorial.zip)
                //)"#
                .into(),
        ))
    }

    fn react_to_proposal(
        &mut self,
        _offer: &Offer,
        _demand: &Proposal,
    ) -> Result<ProposalResponse> {
        Ok(ProposalResponse::AcceptProposal)
    }

    fn react_to_agreement(&mut self, _agreement: &Agreement) -> Result<AgreementResponse> {
        Ok(AgreementResponse::ApproveAgreement)
    }
}

impl AcceptAllNegotiator {
    pub fn new() -> AcceptAllNegotiator {
        AcceptAllNegotiator {}
    }
}