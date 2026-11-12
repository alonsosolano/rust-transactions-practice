//! Test-only helpers shared by the in-crate tests (ADR-0014 T1).
//!
//! `assert_invariants` checks every invariant of requirements §6 that can be
//! read from the engine's state. Tests call it after every row (ADR-0013 C4);
//! the invariants are never checked at run time.

use std::collections::BTreeMap;

use crate::account::Account;
use crate::amount::Balance;
use crate::engine::Engine;
use crate::store::{DisputeState, StoredKind, StoredTx};
use crate::transaction::{ClientId, TxId};

/// Every account and every bound `tx` ID, in a form tests can compare.
pub(crate) type Snapshot = (
    BTreeMap<ClientId, Account>,
    BTreeMap<TxId, (ClientId, StoredTx)>,
);

/// A copy of the engine's state.
pub(crate) fn snapshot(engine: &Engine) -> Snapshot {
    let (accounts, store) = engine.parts();
    let records = store
        .iter()
        .map(|(tx, record)| (tx, (record.client(), *record)))
        .collect();
    (accounts.clone(), records)
}

/// Panics if any invariant readable from the engine's state is broken.
///
/// * I1: `total = available + held` for every account.
/// * I13: `held` is never negative.
/// * I4: `held` equals the sum of the amounts of the client's deposits in
///   `Disputed`.
pub(crate) fn assert_invariants(engine: &Engine) {
    let (accounts, store) = engine.parts();
    let mut disputed: BTreeMap<ClientId, Balance> = BTreeMap::new();
    for (_, record) in store.iter() {
        if record.kind() == StoredKind::Deposit(DisputeState::Disputed) {
            let sum = disputed.entry(record.client()).or_insert(Balance::ZERO);
            *sum = sum
                .checked_add(Balance::from(record.amount()))
                .expect("the sum of disputed deposits fits in a balance");
        }
    }
    for (client, account) in accounts {
        assert_eq!(
            account.available().checked_add(account.held()),
            Some(account.total()),
            "I1 broken for client {client}"
        );
        assert!(
            account.held() >= Balance::ZERO,
            "I13 broken for client {client}"
        );
        assert_eq!(
            account.held(),
            disputed.get(client).copied().unwrap_or(Balance::ZERO),
            "I4 broken for client {client}"
        );
    }
}
