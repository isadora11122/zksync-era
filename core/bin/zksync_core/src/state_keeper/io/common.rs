use std::time::Duration;

use vm::{
    vm_with_bootloader::{BlockContext, BlockContextMode},
    zk_evm::block_properties::BlockProperties,
};
use zksync_contracts::BaseSystemContracts;
use zksync_dal::StorageProcessor;
use zksync_types::{Address, L1BatchNumber, U256, ZKPORTER_IS_AVAILABLE};
use zksync_utils::h256_to_u256;

use super::{L1BatchParams, PendingBatchData};
use crate::state_keeper::extractors;

/// Returns the parameters required to initialize the VM for the next L1 batch.
pub(crate) fn l1_batch_params(
    current_l1_batch_number: L1BatchNumber,
    operator_address: Address,
    l1_batch_timestamp: u64,
    previous_block_hash: U256,
    l1_gas_price: u64,
    fair_l2_gas_price: u64,
    base_system_contracts: BaseSystemContracts,
) -> L1BatchParams {
    let block_properties = BlockProperties {
        default_aa_code_hash: h256_to_u256(base_system_contracts.default_aa.hash),
        zkporter_is_available: ZKPORTER_IS_AVAILABLE,
    };

    let context = BlockContext {
        block_number: current_l1_batch_number.0,
        block_timestamp: l1_batch_timestamp,
        l1_gas_price,
        fair_l2_gas_price,
        operator_address,
    };

    L1BatchParams {
        context_mode: BlockContextMode::NewBlock(context.into(), previous_block_hash),
        properties: block_properties,
        base_system_contracts,
    }
}

/// Returns the amount of iterations `delay_interval` fits into `max_wait`, rounding up.
pub(crate) fn poll_iters(delay_interval: Duration, max_wait: Duration) -> usize {
    let max_wait_millis = max_wait.as_millis() as u64;
    let delay_interval_millis = delay_interval.as_millis() as u64;
    assert!(delay_interval_millis > 0, "delay interval must be positive");

    ((max_wait_millis + delay_interval_millis - 1) / delay_interval_millis).max(1) as usize
}

/// Loads the pending L1 block data from the database.
pub(crate) async fn load_pending_batch(
    storage: &mut StorageProcessor<'_>,
    current_l1_batch_number: L1BatchNumber,
    fee_account: Address,
) -> Option<PendingBatchData> {
    // If pending miniblock doesn't exist, it means that there is no unsynced state (i.e. no transaction
    // were executed after the last sealed batch).
    let pending_miniblock_number = {
        let (_, last_miniblock_number_included_in_l1_batch) = storage
            .blocks_dal()
            .get_miniblock_range_of_l1_batch(current_l1_batch_number - 1)
            .await
            .unwrap();
        last_miniblock_number_included_in_l1_batch + 1
    };
    let pending_miniblock_header = storage
        .blocks_dal()
        .get_miniblock_header(pending_miniblock_number)
        .await?;

    vlog::info!("Getting previous batch hash");
    let (previous_l1_batch_hash, _) =
        extractors::wait_for_prev_l1_batch_params(storage, current_l1_batch_number).await;

    let base_system_contracts = storage
        .storage_dal()
        .get_base_system_contracts(
            pending_miniblock_header
                .base_system_contracts_hashes
                .bootloader,
            pending_miniblock_header
                .base_system_contracts_hashes
                .default_aa,
        )
        .await;

    vlog::info!("Previous l1_batch_hash: {}", previous_l1_batch_hash);
    let params = l1_batch_params(
        current_l1_batch_number,
        fee_account,
        pending_miniblock_header.timestamp,
        previous_l1_batch_hash,
        pending_miniblock_header.l1_gas_price,
        pending_miniblock_header.l2_fair_gas_price,
        base_system_contracts,
    );

    let txs = storage
        .transactions_dal()
        .get_transactions_to_reexecute()
        .await;

    Some(PendingBatchData { params, txs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[rustfmt::skip] // One-line formatting looks better here.
    fn test_poll_iters() {
        assert_eq!(poll_iters(Duration::from_millis(100), Duration::from_millis(0)), 1);
        assert_eq!(poll_iters(Duration::from_millis(100), Duration::from_millis(100)), 1);
        assert_eq!(poll_iters(Duration::from_millis(100), Duration::from_millis(101)), 2);
        assert_eq!(poll_iters(Duration::from_millis(100), Duration::from_millis(200)), 2);
        assert_eq!(poll_iters(Duration::from_millis(100), Duration::from_millis(201)), 3);
    }
}
