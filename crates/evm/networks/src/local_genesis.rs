//! Profile-owned local genesis state application.

#[cfg(feature = "hashkey")]
use super::h20_addresses::{H20_ACTIVATION_REGISTRY, H20_FACTORY, H20_POLICY_REGISTRY};
use alloy_primitives::{Address, Bytes, U256};
#[cfg(feature = "hashkey")]
use hsk_h20_precompiles::ActivationFeature;

/// The backing state established by a lifecycle caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileGenesisTarget {
    /// A new local state with no remote fallback.
    FreshStandalone,
    /// A state backed by a remote fork.
    RemoteFork,
}

/// The minimal state mutation capabilities required by profile-owned genesis.
pub trait LocalGenesisState {
    /// The adapter's source error.
    type Error;

    /// Patches an account's code and nonce while preserving unrelated account state.
    fn patch_account(
        &mut self,
        address: Address,
        code: Bytes,
        nonce: u64,
    ) -> Result<(), Self::Error>;

    /// Writes one storage slot while preserving unrelated account state and storage.
    fn set_storage(&mut self, address: Address, slot: U256, value: U256)
    -> Result<(), Self::Error>;
}

impl super::ResolvedNetworkProfile {
    /// Applies this profile's local genesis recipe to the supplied state adapter.
    ///
    /// The caller invokes this exactly once when establishing a fresh backing-state epoch. A
    /// remote fork and profiles without portable local genesis rules are successful no-ops.
    pub fn apply_profile_genesis<S: LocalGenesisState>(
        self,
        target: ProfileGenesisTarget,
        state: &mut S,
    ) -> Result<(), S::Error> {
        #[cfg(not(feature = "hashkey"))]
        let _ = state;

        #[cfg(feature = "hashkey")]
        if self.is_hashkey() && matches!(target, ProfileGenesisTarget::FreshStandalone) {
            Self::apply_hashkey_genesis(state)?;
        }

        let _ = target;
        Ok(())
    }

    #[cfg(feature = "hashkey")]
    fn apply_hashkey_genesis<S: LocalGenesisState>(state: &mut S) -> Result<(), S::Error> {
        let marker_code = Bytes::from_static(&[0xef]);
        for address in [H20_FACTORY, H20_ACTIVATION_REGISTRY, H20_POLICY_REGISTRY] {
            state.patch_account(address, marker_code.clone(), 1)?;
        }

        let activation_root = erc7201_namespace_root(b"hsk.activation_registry");
        for feature in [
            ActivationFeature::PolicyRegistry,
            ActivationFeature::H20Stablecoin,
            ActivationFeature::H20Asset,
        ] {
            let mut encoded = [0u8; 64];
            encoded[..32].copy_from_slice(feature.id().as_slice());
            encoded[32..].copy_from_slice(&activation_root.to_be_bytes::<32>());
            let slot = U256::from_be_bytes(alloy_primitives::keccak256(encoded).0);
            state.set_storage(H20_ACTIVATION_REGISTRY, slot, U256::from(1))?;
        }

        Ok(())
    }
}

#[cfg(feature = "hashkey")]
fn erc7201_namespace_root(namespace: &[u8]) -> U256 {
    let namespace_hash = U256::from_be_bytes(alloy_primitives::keccak256(namespace).0);
    let root_hash =
        alloy_primitives::keccak256((namespace_hash - U256::from(1)).to_be_bytes::<32>());
    U256::from_be_bytes(root_hash.0) & !U256::from(0xff)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "hashkey")]
    use alloy_primitives::{address, uint};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Operation {
        Account(Address),
        Storage(Address, U256),
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct RecordingError(u8);

    #[derive(Default)]
    struct RecordingState {
        accounts: Vec<(Address, Bytes, u64)>,
        storage: Vec<(Address, U256, U256)>,
        attempted: Vec<Operation>,
        fail_on: Option<Operation>,
    }

    impl RecordingState {
        fn maybe_fail(&mut self, operation: Operation) -> Result<(), RecordingError> {
            self.attempted.push(operation);
            if self.fail_on == Some(operation) {
                Err(RecordingError(self.attempted.len() as u8))
            } else {
                Ok(())
            }
        }

        #[cfg(feature = "hashkey")]
        fn normalized_accounts(&self) -> Vec<(Address, Bytes, u64)> {
            let mut accounts = self.accounts.clone();
            accounts.sort_unstable();
            accounts
        }

        #[cfg(feature = "hashkey")]
        fn normalized_storage(&self) -> Vec<(Address, U256, U256)> {
            let mut storage = self.storage.clone();
            storage.sort_unstable();
            storage
        }
    }

    impl LocalGenesisState for RecordingState {
        type Error = RecordingError;

        fn patch_account(
            &mut self,
            address: Address,
            code: Bytes,
            nonce: u64,
        ) -> Result<(), Self::Error> {
            self.maybe_fail(Operation::Account(address))?;
            self.accounts.push((address, code, nonce));
            Ok(())
        }

        fn set_storage(
            &mut self,
            address: Address,
            slot: U256,
            value: U256,
        ) -> Result<(), Self::Error> {
            self.maybe_fail(Operation::Storage(address, slot))?;
            self.storage.push((address, slot, value));
            Ok(())
        }
    }

    #[test]
    fn profiles_without_portable_genesis_are_noop_for_all_targets() {
        let profiles = [
            super::super::NetworkConfigs::default().resolve(),
            super::super::NetworkConfigs::with_optimism().resolve(),
            super::super::NetworkConfigs::with_tempo().resolve(),
            super::super::NetworkConfigs::with_celo().resolve(),
        ];
        for target in [ProfileGenesisTarget::FreshStandalone, ProfileGenesisTarget::RemoteFork] {
            for profile in profiles {
                let mut state = RecordingState::default();
                profile.apply_profile_genesis(target, &mut state).unwrap();
                assert!(state.attempted.is_empty());
                assert!(state.accounts.is_empty());
                assert!(state.storage.is_empty());
            }
        }
    }

    #[cfg(feature = "hashkey")]
    #[test]
    fn canonical_hashkey_genesis_is_normalized_and_target_gated() {
        let profile = super::super::NetworkConfigs::with_hashkey().resolve();
        let mut state = RecordingState::default();
        profile.apply_profile_genesis(ProfileGenesisTarget::FreshStandalone, &mut state).unwrap();

        let mut expected_accounts = vec![
            (address!("0177FF0000000000000000000000000000000000"), Bytes::from_static(&[0xef]), 1),
            (address!("0177FF0000000000000000000000000000000001"), Bytes::from_static(&[0xef]), 1),
            (address!("0177FF0000000000000000000000000000000002"), Bytes::from_static(&[0xef]), 1),
        ];
        expected_accounts.sort_unstable();
        assert_eq!(state.normalized_accounts(), expected_accounts);

        let mut expected_storage = vec![
            (
                address!("0177FF0000000000000000000000000000000001"),
                uint!(0x8b392998db41c7a56188d244d46e8d66c0e0050b53b0bc8c714ab53aedfe76c7_U256),
                U256::from(1),
            ),
            (
                address!("0177FF0000000000000000000000000000000001"),
                uint!(0x4730f9e7e976620ebfcb1cdf39c7bb118e5041f58590168ef9bf4a81e5793967_U256),
                U256::from(1),
            ),
            (
                address!("0177FF0000000000000000000000000000000001"),
                uint!(0xccefe162b92620aa4539cf19b2b49990ae6c328fdc91d82c89e41c203b682d55_U256),
                U256::from(1),
            ),
        ];
        expected_storage.sort_unstable();
        assert_eq!(state.normalized_storage(), expected_storage);

        let mut fork_state = RecordingState::default();
        profile.apply_profile_genesis(ProfileGenesisTarget::RemoteFork, &mut fork_state).unwrap();
        assert!(fork_state.attempted.is_empty());
        assert!(fork_state.accounts.is_empty());
        assert!(fork_state.storage.is_empty());
    }

    #[cfg(feature = "hashkey")]
    #[test]
    fn adapter_failure_is_transparent_and_stops_application() {
        let profile = super::super::NetworkConfigs::with_hashkey().resolve();
        let account_failure_point =
            Operation::Account(address!("0177FF0000000000000000000000000000000001"));
        let mut account_failure =
            RecordingState { fail_on: Some(account_failure_point), ..Default::default() };
        let error = profile
            .apply_profile_genesis(ProfileGenesisTarget::FreshStandalone, &mut account_failure)
            .unwrap_err();
        assert_eq!(error, RecordingError(2));
        assert_eq!(account_failure.attempted.last(), Some(&account_failure_point));
        assert_eq!(account_failure.accounts.len(), 1);
        assert!(account_failure.storage.is_empty());

        let storage_failure_point = Operation::Storage(
            address!("0177FF0000000000000000000000000000000001"),
            uint!(0x8b392998db41c7a56188d244d46e8d66c0e0050b53b0bc8c714ab53aedfe76c7_U256),
        );
        let mut storage_failure =
            RecordingState { fail_on: Some(storage_failure_point), ..Default::default() };
        let error = profile
            .apply_profile_genesis(ProfileGenesisTarget::FreshStandalone, &mut storage_failure)
            .unwrap_err();
        assert_eq!(error, RecordingError(4));
        assert_eq!(storage_failure.attempted.last(), Some(&storage_failure_point));
        assert_eq!(storage_failure.accounts.len(), 3);
        assert!(storage_failure.storage.is_empty());
    }
}
