# Storage Key Schema and Migration Compatibility

This document details the storage key schema layout for all Soroban smart contracts in `SS-contracts` to ensure backward compatibility across version upgrades.

## Storage Hierarchy

### Instance Storage
- `Admin`: Primary contract administrator address.
- `Config`: Global configuration parameters (timelock delays, multi-sig signer sets, threshold, pause states).
- `Logic`: Current logic implementation address for proxy contracts.

### Persistent Storage
- `Invoice(u64)`: Segregated per-invoice vault records (`DefaultedInvoice`, `collateral_amount`, status).
- `Holders(u64)`: Vector of fractional investor positions per invoice (`FractionHolder`).
- `Proposal(u64)`: Upgrade or restructuring proposals, tracking queued timestamps, execution ETAs, and signatures.

## Migration Compatibility Rules
1. **Never mutate key discriminant enum ordering**: Storage keys must maintain stable numeric tag serialization.
2. **Nullable extensions**: New storage fields must be wrapped in `Option<T>` or assigned explicit defaults on read.
3. **Segregated vaults**: Accounting keys for invoices are isolated per ID to prevent cross-invoice fund mixing.
