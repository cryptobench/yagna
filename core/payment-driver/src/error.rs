use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum PaymentDriverError {
    #[error("Insufficient gas")]
    InsufficientGas,
    #[error("Insufficient funds")]
    InsufficientFunds,
    #[error("Payment already scheduled")]
    AlreadyScheduled,
    #[error("Payment not found")]
    NotFound,
    #[error("Connection refused")]
    ConnectionRefused,
    #[error("Library error")]
    LibraryError { msg: String },
}
