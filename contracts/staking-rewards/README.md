# Staking rewards

Rewards are distributed in fixed 30-day epochs (`EPOCH_DURATION`). The
registered invoice issuer starts an epoch. At the start, the contract snapshots
the total stake and the funded reward pool, including carry-over from the
previous epoch. Funding received after that point is reserved for a later epoch
and cannot increase the active epoch's rewards.

An eligible stake's reward is its pro-rata share of the snapshotted pool,
rounded down to the token's smallest unit. The stake must exist when the epoch
starts. Rewards can be claimed only while the epoch is active and before its
end timestamp; an eligible stake can also claim its current-epoch reward as
part of unstaking. At or after the end timestamp, claims are closed. Anyone may
then close the epoch, and its unpaid balance—including rounding dust—is
carried into the next epoch. Closing is one-way, so a closed epoch cannot be
claimed or closed again.

Use `epoch_status(invoice_id, epoch_id)` to inspect the immutable stake/pool
snapshots, timestamps, status, and amount already claimed. Use
`claimable_amount(stake_id, epoch_id)` to read an individual stake's currently
claimable reward. Epoch starts and closes emit `staking_epoch_started` and
`staking_epoch_closed`.

The existing `Invoice::bonus_bps` field is retained for interface
compatibility but does not affect epoch rewards; rewards are entirely funded
from the staking pool. This accounting requires a fresh deployment or a
migration that initializes the per-invoice total-stake state for existing
stakes.
