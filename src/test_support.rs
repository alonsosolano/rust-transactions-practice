//! Test-only helpers shared by the in-crate tests (ADR-0014 T1).
//!
//! * [`assert_invariants`] checks the invariants of requirements §6 that can
//!   be read from one state of the engine.
//! * [`Checked`] wraps an engine, keeps the applied-row ledger of ADR-0013
//!   C5, and checks every per-row invariant after every row it applies.
//! * [`any_row`], [`amount_of_units`], and [`csv_line`] are the strategies
//!   and helpers the property tests share (ADR-0014 T11).
//!
//! The invariants are never checked at run time (ADR-0013 C4).

use std::collections::BTreeMap;
use std::ops::Deref;

use proptest::prelude::*;

use crate::account::Account;
use crate::amount::{Amount, Balance};
use crate::engine::{Engine, Rejection};
use crate::store::{DisputeState, StoredKind, StoredTx};
use crate::transaction::{ClientId, Transaction, TxId};

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

/// An engine that checks every per-row invariant after every row.
///
/// [`Checked::apply`] applies a row to the inner engine and then panics if
/// any of these is broken:
///
/// * I1, I4, I13 through [`assert_invariants`];
/// * I2: no account goes from locked back to unlocked;
/// * I3: a row that returns an error changes nothing, except the two effects
///   of a rejected withdrawal (a zero account for an unseen client and its
///   bound ID);
/// * I8: no withdrawal is applied to a locked account;
/// * I5, I11, I16: a bound ID keeps its client and amount, only the row's
///   own `tx` can change kind, and only along the legal dispute transitions;
/// * I14, I15: each client's `total`, and the sum over all accounts, equal
///   the ledger of applied rows (ADR-0013 C5).
///
/// The ledger is computed from the rows the engine reported as applied
/// (`apply` returned `Ok`), never from the engine's balances: a deposit adds
/// its amount, a withdrawal subtracts its amount, and a chargeback subtracts
/// the amount of the deposit it names. Disputes and resolves only move money
/// between `available` and `held`, so they do not change it.
///
/// It dereferences to the inner engine for reads.
#[derive(Debug, Default)]
pub(crate) struct Checked {
    engine: Engine,
    /// Expected `total` per client: opening balance plus applied rows.
    ledger: BTreeMap<ClientId, Balance>,
}

impl Deref for Checked {
    type Target = Engine;

    fn deref(&self) -> &Engine {
        &self.engine
    }
}

impl Checked {
    /// A checked engine with no accounts and no bound IDs.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Replaces one account, for tests that need states no row can reach.
    /// The account's `total` becomes the client's opening balance in the
    /// ledger, so later rows are still checked against it.
    pub(crate) fn set_account(&mut self, client: ClientId, account: Account) {
        self.engine.set_account(client, account);
        self.ledger.insert(client, account.total());
    }

    /// Applies one row to the engine and checks every per-row invariant.
    pub(crate) fn apply(&mut self, row: Transaction) -> Result<(), Rejection> {
        let before = snapshot(&self.engine);
        let result = self.engine.apply(row);
        let after = snapshot(&self.engine);

        assert_invariants(&self.engine);
        assert_lock_never_reverts(&before, &after);
        assert_effects(row, result, &before, &after);
        assert_record_changes(row, result, &before, &after);
        if result.is_ok() {
            self.record(row, &before);
        }
        self.assert_ledger();
        result
    }

    /// Adds an applied row to the ledger (ADR-0013 C5).
    fn record(&mut self, row: Transaction, before: &Snapshot) {
        let (client, change) = match row {
            Transaction::Deposit { client, amount, .. } => (client, Balance::from(amount)),
            Transaction::Withdrawal { client, amount, .. } => {
                (client, negated(Balance::from(amount)))
            }
            Transaction::Chargeback { client, tx } => {
                let (_, record) = before.1.get(&tx).expect("a chargeback names a stored tx");
                (client, negated(Balance::from(record.amount())))
            }
            Transaction::Dispute { .. } | Transaction::Resolve { .. } => return,
        };
        let total = self.ledger.entry(client).or_insert(Balance::ZERO);
        *total = total
            .checked_add(change)
            .expect("the ledger of applied rows fits in a balance");
    }

    /// I14 per client and I15 over all accounts.
    fn assert_ledger(&self) {
        let (accounts, _) = self.engine.parts();
        for (client, account) in accounts {
            assert_eq!(
                account.total(),
                self.ledger.get(client).copied().unwrap_or(Balance::ZERO),
                "I14 broken for client {client}"
            );
        }
        for client in self.ledger.keys() {
            assert!(
                accounts.contains_key(client),
                "I14 broken: client {client} has applied rows but no account"
            );
        }
        let sum_of_totals = WideSum::of(accounts.values().map(Account::total));
        let sum_of_ledger = WideSum::of(self.ledger.values().copied());
        assert_eq!(sum_of_totals, sum_of_ledger, "I15 broken");
    }
}

/// An exact sum of balances that may leave the `i128` range: the wrapped sum
/// and the number of times it wrapped. Tests that open accounts near the end
/// of the range (I6) would otherwise overflow the I15 check itself.
#[derive(Debug, Default, PartialEq, Eq)]
struct WideSum {
    wrapped: i128,
    carries: i64,
}

impl WideSum {
    fn of(balances: impl IntoIterator<Item = Balance>) -> Self {
        balances.into_iter().fold(Self::default(), |sum, balance| {
            let units = balance.units();
            let (wrapped, overflowed) = sum.wrapped.overflowing_add(units);
            let carry = match (overflowed, units > 0) {
                (false, _) => 0,
                (true, true) => 1,
                (true, false) => -1,
            };
            Self {
                wrapped,
                carries: sum
                    .carries
                    .checked_add(carry)
                    .expect("fewer than 2^63 terms"),
            }
        })
    }
}

/// `-value`, as `0 - value`.
fn negated(value: Balance) -> Balance {
    Balance::ZERO
        .checked_sub(value)
        .expect("the negation of an amount fits in a balance")
}

/// I2: every account that was locked before the row is still locked.
fn assert_lock_never_reverts(before: &Snapshot, after: &Snapshot) {
    for (client, account) in &before.0 {
        if account.is_locked() {
            assert!(
                after.0.get(client).is_some_and(Account::is_locked),
                "I2 broken for client {client}"
            );
        }
    }
}

/// I3 for every row that returns an error, and I8 for every applied
/// withdrawal.
fn assert_effects(
    row: Transaction,
    result: Result<(), Rejection>,
    before: &Snapshot,
    after: &Snapshot,
) {
    match (result, row) {
        (Ok(()), Transaction::Withdrawal { client, .. }) => {
            assert!(
                !before.0.get(&client).is_some_and(Account::is_locked),
                "I8 broken: a withdrawal was applied to locked client {client}"
            );
        }
        (Ok(()), _) => {}
        (
            Err(Rejection::InsufficientFunds | Rejection::AccountLocked),
            Transaction::Withdrawal { client, tx, amount },
        ) => {
            let mut accounts = before.0.clone();
            accounts.entry(client).or_default();
            let mut records = before.1.clone();
            let rejected = StoredTx::new(client, amount, StoredKind::RejectedWithdrawal);
            assert!(
                records.insert(tx, (client, rejected)).is_none(),
                "I3 broken: a rejected withdrawal reused a bound ID"
            );
            assert_eq!(after, &(accounts, records), "I3 broken by {row:?}");
        }
        (Err(rejection), _) => {
            assert_eq!(after, before, "I3 broken by {row:?} ({rejection:?})");
        }
    }
}

/// The `tx` ID a row names.
pub(crate) fn tx_of(row: Transaction) -> TxId {
    match row {
        Transaction::Deposit { tx, .. }
        | Transaction::Withdrawal { tx, .. }
        | Transaction::Dispute { tx, .. }
        | Transaction::Resolve { tx, .. }
        | Transaction::Chargeback { tx, .. } => tx,
    }
}

/// The client a row names.
pub(crate) fn client_of(row: Transaction) -> ClientId {
    match row {
        Transaction::Deposit { client, .. }
        | Transaction::Withdrawal { client, .. }
        | Transaction::Dispute { client, .. }
        | Transaction::Resolve { client, .. }
        | Transaction::Chargeback { client, .. } => client,
    }
}

/// The dispute state a deposit moves to when `row` applies to it, written
/// out from the state machine of ADR-0006 independently of
/// `DisputeState::next`, or `None` if the pair is not a legal move (I16).
fn legal_move(state: DisputeState, row: Transaction) -> Option<DisputeState> {
    use DisputeState::{ChargedBack, Disputed, Processed, Resolved};
    match (state, row) {
        (Processed | Resolved, Transaction::Dispute { .. }) => Some(Disputed),
        (Disputed, Transaction::Resolve { .. }) => Some(Resolved),
        (Disputed, Transaction::Chargeback { .. }) => Some(ChargedBack),
        _ => None,
    }
}

/// I5, I11, and I16 on the store.
fn assert_record_changes(
    row: Transaction,
    result: Result<(), Rejection>,
    before: &Snapshot,
    after: &Snapshot,
) {
    let row_tx = tx_of(row);
    for (tx, (client, record)) in &before.1 {
        let (new_client, new_record) = after
            .1
            .get(tx)
            .unwrap_or_else(|| panic!("I11 broken: tx {tx:?} was unbound"));
        assert_eq!(new_client, client, "I11 broken: tx {tx:?} changed client");
        assert_eq!(
            new_record.amount(),
            record.amount(),
            "I11 broken: tx {tx:?} changed amount"
        );
        if new_record.kind() == record.kind() {
            continue;
        }
        // Only the row's own `tx` can change kind, only when the row
        // applied, and only along a legal dispute move of a deposit.
        let legal = match record.kind() {
            StoredKind::Deposit(state) if *tx == row_tx && result.is_ok() => {
                legal_move(state, row).map(StoredKind::Deposit)
            }
            // I5: a withdrawal or rejected withdrawal never changes kind.
            _ => None,
        };
        assert_eq!(
            Some(new_record.kind()),
            legal,
            "I16 broken: tx {tx:?} moved from {:?} to {:?} on {row:?}",
            record.kind(),
            new_record.kind()
        );
    }
    let new_ids: Vec<&TxId> = after
        .1
        .keys()
        .filter(|tx| !before.1.contains_key(tx))
        .collect();
    let expected_new = match (row, result) {
        (Transaction::Deposit { .. }, Ok(())) => Some(StoredKind::Deposit(DisputeState::Processed)),
        (Transaction::Withdrawal { .. }, Ok(())) => Some(StoredKind::Withdrawal),
        (
            Transaction::Withdrawal { .. },
            Err(Rejection::InsufficientFunds | Rejection::AccountLocked),
        ) => Some(StoredKind::RejectedWithdrawal),
        _ => None,
    };
    match expected_new {
        Some(kind) => {
            assert_eq!(new_ids, [&row_tx], "I11 broken: wrong IDs bound by {row:?}");
            let (client, record) = &after.1[&row_tx];
            assert_eq!(*client, client_of(row), "I11 broken: wrong owner");
            assert_eq!(record.kind(), kind, "I5 broken: wrong kind for {row:?}");
        }
        None => assert!(new_ids.is_empty(), "I11 broken: {row:?} bound an ID"),
    }
}

/// The amount of `units` units of `0.0001`. `units` must not be zero.
pub(crate) fn amount_of_units(units: u64) -> Amount {
    let text = format!("{}.{:04}", units / 10_000, units % 10_000);
    Amount::parse(text.as_bytes()).expect("a non-zero unit count is a valid amount")
}

/// One generated row of any of the five types over small pools of clients,
/// IDs, and amounts, so duplicates, refusals, cross-client references, and
/// every dispute transition are common (ADR-0014 T11).
pub(crate) fn any_row() -> impl Strategy<Value = Transaction> {
    row_from(
        0_u8..5,
        0_u16..4,
        0_u32..24,
        (1_u64..=50_000).prop_map(amount_of_units),
    )
}

/// A row built from the given strategies for its type (`0` deposit, `1`
/// withdrawal, `2` dispute, `3` resolve, anything else chargeback), client,
/// `tx`, and amount. The amount is dropped for the three dispute types.
pub(crate) fn row_from(
    kind: impl Strategy<Value = u8>,
    client: impl Strategy<Value = u16>,
    tx: impl Strategy<Value = u32>,
    amount: impl Strategy<Value = Amount>,
) -> impl Strategy<Value = Transaction> {
    (kind, client, tx, amount).prop_map(|(kind, client, tx, amount)| {
        let (client, tx) = (ClientId(client), TxId(tx));
        match kind {
            0 => Transaction::Deposit { client, tx, amount },
            1 => Transaction::Withdrawal { client, tx, amount },
            2 => Transaction::Dispute { client, tx },
            3 => Transaction::Resolve { client, tx },
            _ => Transaction::Chargeback { client, tx },
        }
    })
}

/// The input line for a row, as the brief writes it: padded fields, and a
/// trailing empty amount on the three dispute types.
pub(crate) fn csv_line(row: Transaction) -> String {
    match row {
        Transaction::Deposit { client, tx, amount } => {
            format!("deposit, {client}, {}, {amount}", tx.0)
        }
        Transaction::Withdrawal { client, tx, amount } => {
            format!("withdrawal, {client}, {}, {amount}", tx.0)
        }
        Transaction::Dispute { client, tx } => format!("dispute, {client}, {},", tx.0),
        Transaction::Resolve { client, tx } => format!("resolve, {client}, {},", tx.0),
        Transaction::Chargeback { client, tx } => format!("chargeback, {client}, {},", tx.0),
    }
}
