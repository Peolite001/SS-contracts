//! Data types for the platform pause contract.

use soroban_sdk::{contracttype, Address, BytesN, Vec};

/// Storage key enum for instance and persistent storage.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageKey {
    /// Instance: contract admin.
    Admin,
    /// Instance: whether the platform is currently paused.
    Paused,
    /// Instance: unix timestamp a queued resume becomes executable at, if
    /// any resume is currently queued.
    ResumeEta,
    /// Persistent: per-invoice pause override, independent of the
    /// platform-wide pause.
    InvoicePaused(BytesN<32>),
    /// Persistent: registered critical contracts for freeze propagation.
    RegisteredContract(Address),
    /// Instance: list of registered contract addresses.
    RegisteredContracts,
    /// Instance: max number of contracts that can be registered.
    MaxRegisteredContracts,
}

/// Status of freeze propagation for a single contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreezeStatus {
    /// Contract address.
    pub contract: Address,
    /// Whether freeze was successfully propagated.
    pub success: bool,
    /// Error message if propagation failed.
    pub error: Option<soroban_sdk::String>,
}
