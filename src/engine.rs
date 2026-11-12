//! `Engine::apply` and `Rejection`: applies one transaction to the accounts and the store (ADR-0011 S6, S7).
//!
//! Domain module of the layout in ADR-0011: no I/O, no `csv`, no serde.

use std::collections::BTreeMap;

use crate::account::{Account, WithdrawalError};
use crate::amount::Amount;
use crate::store::{DisputeState, Store, StoredKind, StoredTx};
use crate::transaction::{ClientId, Transaction, TxId};

/// Why a valid row changed no balance (ADR-0010, condition table).
///
/// Every variant is a distinct reason that the diagnostics count. A
/// rejected withdrawal still opens a zero-balance account and binds its ID
/// (ADR-0006 R8, ADR-0010 V6); every other variant leaves the engine
/// untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// A withdrawal above `available` (`insufficient_funds`). Rejected.
    InsufficientFunds,
    /// A withdrawal on a locked account (`account_locked`, ADR-0008 L3, L4).
    /// Rejected.
    AccountLocked,
    /// A `deposit` or `withdrawal` reusing a bound `tx` ID (`duplicate_tx`,
    /// ADR-0010 V6). Ignored.
    DuplicateTx {
        /// The client that owns the ID.
        owner: ClientId,
    },
    /// A row whose effect would overflow a balance (`overflow`, ADR-0007 M7).
    /// Ignored.
    Overflow,
    /// A `dispute`, `resolve`, or `chargeback` row. The dispute lifecycle is
    /// not built yet (E4, LYA-52), so these rows are ignored and reported.
    /// E4 replaces this variant with the reasons of ADR-0006.
    NotYetSupported,
}

/// The ledger: every account and every bound `tx` ID (ADR-0011).
///
/// The engine performs no I/O and never writes diagnostics; it returns a
/// [`Rejection`] and the caller reports it (ADR-0011 S6).
#[derive(Debug, Default)]
pub struct Engine {
    /// Ordered by client, which is the output order (ADR-0009 O3).
    accounts: BTreeMap<ClientId, Account>,
    store: Store,
}

// ADR-0011 S13: an engine can move to another thread or task.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Engine>();
};

impl Engine {
    /// An engine with no accounts and no bound IDs.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one transaction (ADR-0011 S6).
    ///
    /// Every new value is computed before anything changes (S7), so a row
    /// that returns an error leaves the engine as it was, except for the two
    /// effects of a rejected withdrawal that invariant I3 allows.
    ///
    /// # Errors
    ///
    /// Returns the [`Rejection`] that says why the row changed no balance.
    pub fn apply(&mut self, transaction: Transaction) -> Result<(), Rejection> {
        match transaction {
            Transaction::Deposit { client, tx, amount } => self.deposit(client, tx, amount),
            Transaction::Withdrawal { client, tx, amount } => self.withdraw(client, tx, amount),
            Transaction::Dispute { .. }
            | Transaction::Resolve { .. }
            | Transaction::Chargeback { .. } => Err(Rejection::NotYetSupported),
        }
    }

    /// Every account in ascending client order (ADR-0009 O3).
    pub fn accounts(&self) -> impl Iterator<Item = (ClientId, &Account)> {
        self.accounts
            .iter()
            .map(|(client, account)| (*client, account))
    }

    /// Fails with `DuplicateTx` if `tx` is already bound (ADR-0010 V6).
    fn check_unbound(&self, tx: TxId) -> Result<(), Rejection> {
        match self.store.get(tx) {
            Some(stored) => Err(Rejection::DuplicateTx {
                owner: stored.client(),
            }),
            None => Ok(()),
        }
    }

    /// The client's account, or a new zero account if the client is unseen.
    fn account(&self, client: ClientId) -> Account {
        self.accounts.get(&client).copied().unwrap_or_default()
    }

    fn deposit(&mut self, client: ClientId, tx: TxId, amount: Amount) -> Result<(), Rejection> {
        self.check_unbound(tx)?;
        let updated = self
            .account(client)
            .deposited(amount)
            .ok_or(Rejection::Overflow)?;

        self.accounts.insert(client, updated);
        let kind = StoredKind::Deposit(DisputeState::Processed);
        self.store.bind(tx, StoredTx::new(client, amount, kind));
        Ok(())
    }

    fn withdraw(&mut self, client: ClientId, tx: TxId, amount: Amount) -> Result<(), Rejection> {
        self.check_unbound(tx)?;
        let current = self.account(client);
        let rejection = match current.withdrawn(amount) {
            Ok(updated) => {
                self.accounts.insert(client, updated);
                let record = StoredTx::new(client, amount, StoredKind::Withdrawal);
                self.store.bind(tx, record);
                return Ok(());
            }
            Err(WithdrawalError::Overflow) => return Err(Rejection::Overflow),
            Err(WithdrawalError::InsufficientFunds) => Rejection::InsufficientFunds,
            Err(WithdrawalError::Locked) => Rejection::AccountLocked,
        };

        // A rejected withdrawal opens the account of an unseen client with
        // zero balances (ADR-0006 R8) and binds its ID (ADR-0010 V6).
        self.accounts.entry(client).or_insert(current);
        let record = StoredTx::new(client, amount, StoredKind::RejectedWithdrawal);
        self.store.bind(tx, record);
        Err(rejection)
    }

    /// The engine's account and store state, for tests (C4).
    #[cfg(test)]
    pub(crate) fn parts(&self) -> (&BTreeMap<ClientId, Account>, &Store) {
        (&self.accounts, &self.store)
    }

    /// Replaces one account, for tests that need states no row can reach yet.
    #[cfg(test)]
    pub(crate) fn set_account(&mut self, client: ClientId, account: Account) {
        self.accounts.insert(client, account);
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{Engine, Rejection};
    use crate::account::Account;
    use crate::amount::{Amount, Balance};
    use crate::store::{DisputeState, StoredKind, StoredTx};
    use crate::test_support::{assert_invariants, snapshot};
    use crate::transaction::{ClientId, Transaction, TxId};

    fn amount(text: &str) -> Amount {
        Amount::parse(text.as_bytes()).unwrap()
    }

    fn deposit(client: u16, tx: u32, value: &str) -> Transaction {
        Transaction::Deposit {
            client: ClientId(client),
            tx: TxId(tx),
            amount: amount(value),
        }
    }

    fn withdrawal(client: u16, tx: u32, value: &str) -> Transaction {
        Transaction::Withdrawal {
            client: ClientId(client),
            tx: TxId(tx),
            amount: amount(value),
        }
    }

    /// Applies each row in order, checking the invariants after every step
    /// (ADR-0013 C4), and returns each row's result.
    fn run(engine: &mut Engine, rows: &[Transaction]) -> Vec<Result<(), Rejection>> {
        rows.iter()
            .map(|&row| {
                let result = engine.apply(row);
                assert_invariants(engine);
                result
            })
            .collect()
    }

    /// `available / held / total / locked` of one client, as in the catalog.
    fn state(engine: &Engine, client: u16) -> Option<(String, String, String, bool)> {
        let (accounts, _) = engine.parts();
        accounts.get(&ClientId(client)).map(|account| {
            (
                account.available().to_string(),
                account.held().to_string(),
                account.total().to_string(),
                account.is_locked(),
            )
        })
    }

    fn row(
        available: &str,
        held: &str,
        total: &str,
        locked: bool,
    ) -> Option<(String, String, String, bool)> {
        Some((available.into(), held.into(), total.into(), locked))
    }

    #[test]
    fn deposits_and_withdrawals_of_interleaved_clients_apply_per_client() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[
                deposit(2, 1, "5"),
                deposit(1, 2, "10"),
                withdrawal(2, 3, "1.5"),
                deposit(2, 4, "0.0001"),
                withdrawal(1, 5, "10"),
            ],
        );
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(state(&engine, 1), row("0.0000", "0.0000", "0.0000", false));
        assert_eq!(state(&engine, 2), row("3.5001", "0.0000", "3.5001", false));
    }

    #[test]
    fn accounts_iterate_in_ascending_client_order() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[
                deposit(9, 1, "1"),
                deposit(0, 2, "1"),
                deposit(65535, 3, "1"),
                deposit(4, 4, "1"),
            ],
        );
        let clients: Vec<u16> = engine.accounts().map(|(client, _)| client.0).collect();
        assert_eq!(clients, [0, 4, 9, 65535]);
    }

    #[test]
    fn deposits_and_withdrawals_are_stored_with_their_kind() {
        let mut engine = Engine::new();
        run(&mut engine, &[deposit(1, 1, "5"), withdrawal(1, 2, "2")]);
        let (_, store) = engine.parts();
        assert_eq!(
            store.get(TxId(1)),
            Some(&StoredTx::new(
                ClientId(1),
                amount("5"),
                StoredKind::Deposit(DisputeState::Processed)
            ))
        );
        assert_eq!(
            store.get(TxId(2)),
            Some(&StoredTx::new(
                ClientId(1),
                amount("2"),
                StoredKind::Withdrawal
            ))
        );
    }

    /// I3: a rejected withdrawal has exactly two effects: a zero account for
    /// an unseen client (R8) and its bound ID (V6).
    #[test]
    fn i3_a_rejected_withdrawal_opens_a_zero_account_and_binds_its_id() {
        let mut engine = Engine::new();
        let results = run(&mut engine, &[withdrawal(3, 1, "1")]);
        assert_eq!(results, [Err(Rejection::InsufficientFunds)]);
        assert_eq!(state(&engine, 3), row("0.0000", "0.0000", "0.0000", false));
        let (_, store) = engine.parts();
        assert_eq!(
            store.get(TxId(1)),
            Some(&StoredTx::new(
                ClientId(3),
                amount("1"),
                StoredKind::RejectedWithdrawal
            ))
        );
    }

    /// I3: on a known client, a rejected withdrawal leaves every balance as
    /// it was.
    #[test]
    fn i3_a_rejected_withdrawal_leaves_balances_unchanged() {
        let mut engine = Engine::new();
        run(&mut engine, &[deposit(1, 1, "5")]);
        let (accounts_before, _) = snapshot(&engine);
        let results = run(&mut engine, &[withdrawal(1, 2, "5.0001")]);
        assert_eq!(results, [Err(Rejection::InsufficientFunds)]);
        let (accounts_after, _) = snapshot(&engine);
        assert_eq!(accounts_before, accounts_after);
    }

    /// I3: a duplicate changes nothing at all.
    #[test]
    fn i3_a_duplicate_changes_nothing() {
        let mut engine = Engine::new();
        run(&mut engine, &[deposit(1, 1, "5")]);
        let before = snapshot(&engine);
        let results = run(&mut engine, &[deposit(2, 1, "50"), withdrawal(2, 1, "1")]);
        assert_eq!(
            results,
            [
                Err(Rejection::DuplicateTx { owner: ClientId(1) }),
                Err(Rejection::DuplicateTx { owner: ClientId(1) }),
            ]
        );
        assert_eq!(snapshot(&engine), before);
        assert_eq!(state(&engine, 2), None);
    }

    /// The owner named by a duplicate is the first row's client.
    #[test]
    fn a_duplicate_names_the_owner_of_the_id() {
        let mut engine = Engine::new();
        run(&mut engine, &[withdrawal(4, 9, "1")]);
        match engine.apply(deposit(1, 9, "1")) {
            Err(Rejection::DuplicateTx { owner }) => assert_eq!(owner, ClientId(4)),
            other => panic!("expected a duplicate, got {other:?}"),
        }
    }

    /// A29: the rejected withdrawal binds tx2, so a later withdrawal reusing
    /// tx2 cannot succeed after a deposit.
    #[test]
    fn a29_rejected_withdrawal_binds_its_id() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[
                deposit(1, 1, "5"),
                withdrawal(1, 2, "10"),
                deposit(1, 3, "10"),
                withdrawal(1, 2, "10"),
            ],
        );
        assert_eq!(
            results,
            [
                Ok(()),
                Err(Rejection::InsufficientFunds),
                Ok(()),
                Err(Rejection::DuplicateTx { owner: ClientId(1) }),
            ]
        );
        assert_eq!(
            state(&engine, 1),
            row("15.0000", "0.0000", "15.0000", false)
        );
    }

    /// A40: an applied withdrawal binds tx2; the replay and a deposit reusing
    /// tx2 are both ignored.
    #[test]
    fn a40_applied_withdrawal_binds_its_id() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[
                deposit(1, 1, "20"),
                withdrawal(1, 2, "5"),
                withdrawal(1, 2, "5"),
                deposit(1, 2, "100"),
            ],
        );
        assert_eq!(
            results[2],
            Err(Rejection::DuplicateTx { owner: ClientId(1) })
        );
        assert_eq!(
            results[3],
            Err(Rejection::DuplicateTx { owner: ClientId(1) })
        );
        assert_eq!(
            state(&engine, 1),
            row("15.0000", "0.0000", "15.0000", false)
        );
    }

    /// ADR-0008 L3, L4: on a locked account every withdrawal is rejected as
    /// locked, even one that also lacks funds, and it still binds its ID.
    #[test]
    fn a_withdrawal_on_a_locked_account_is_rejected_as_locked() {
        let mut engine = Engine::new();
        let locked =
            Account::with_balances(Balance::from(amount("5")), Balance::ZERO, true).unwrap();
        engine.set_account(ClientId(1), locked);
        let results = run(
            &mut engine,
            &[withdrawal(1, 3, "100"), withdrawal(1, 4, "1")],
        );
        assert_eq!(
            results,
            [Err(Rejection::AccountLocked), Err(Rejection::AccountLocked)]
        );
        assert_eq!(state(&engine, 1), row("5.0000", "0.0000", "5.0000", true));
        let (_, store) = engine.parts();
        assert_eq!(store.len(), 2);
    }

    /// ADR-0008 L2: a deposit on a locked account is applied.
    #[test]
    fn a_deposit_on_a_locked_account_is_applied() {
        let mut engine = Engine::new();
        let locked = Account::with_balances(Balance::ZERO, Balance::ZERO, true).unwrap();
        engine.set_account(ClientId(1), locked);
        assert_eq!(run(&mut engine, &[deposit(1, 1, "2")]), [Ok(())]);
        assert_eq!(state(&engine, 1), row("2.0000", "0.0000", "2.0000", true));
    }

    /// I6: a deposit that would overflow is ignored: no balance change, no
    /// bound ID, and no account for an unseen client.
    #[test]
    fn i6_an_overflowing_deposit_changes_nothing() {
        let mut engine = Engine::new();
        let near_max =
            Account::with_balances(Balance::from_units(i128::MAX), Balance::ZERO, false).unwrap();
        engine.set_account(ClientId(1), near_max);
        let before = snapshot(&engine);
        assert_eq!(engine.apply(deposit(1, 1, "1")), Err(Rejection::Overflow));
        assert_eq!(snapshot(&engine), before);
        assert_eq!(engine.apply(deposit(1, 2, "1")), Err(Rejection::Overflow));
        assert_eq!(snapshot(&engine), before);
    }

    /// Valid input cannot overflow a balance (ADR-0007, Range).
    #[test]
    fn maximum_deposits_do_not_overflow() {
        let mut engine = Engine::new();
        let rows: Vec<Transaction> = (0..1000)
            .map(|tx| Transaction::Deposit {
                client: ClientId(1),
                tx: TxId(tx),
                amount: Amount::MAX,
            })
            .collect();
        assert!(run(&mut engine, &rows).iter().all(Result::is_ok));
    }

    /// Dispute-related rows are not handled yet (E4) and change nothing.
    #[test]
    fn dispute_related_rows_are_not_yet_supported() {
        let mut engine = Engine::new();
        run(&mut engine, &[deposit(1, 1, "5")]);
        let before = snapshot(&engine);
        let rows = [
            Transaction::Dispute {
                client: ClientId(1),
                tx: TxId(1),
            },
            Transaction::Resolve {
                client: ClientId(1),
                tx: TxId(1),
            },
            Transaction::Chargeback {
                client: ClientId(1),
                tx: TxId(1),
            },
            Transaction::Dispute {
                client: ClientId(9),
                tx: TxId(7),
            },
        ];
        for result in run(&mut engine, &rows) {
            assert_eq!(result, Err(Rejection::NotYetSupported));
        }
        assert_eq!(snapshot(&engine), before);
    }

    /// One generated row: a deposit or withdrawal over small pools of
    /// clients, IDs, and amounts, so duplicates and refusals are common.
    fn any_row() -> impl Strategy<Value = Transaction> {
        (any::<bool>(), 0_u16..4, 0_u32..24, 1_u64..=50_000).prop_map(
            |(is_deposit, client, tx, units)| {
                let text = format!("{}.{:04}", units / 10_000, units % 10_000);
                let amount = Amount::parse(text.as_bytes()).unwrap();
                let (client, tx) = (ClientId(client), TxId(tx));
                if is_deposit {
                    Transaction::Deposit { client, tx, amount }
                } else {
                    Transaction::Withdrawal { client, tx, amount }
                }
            },
        )
    }

    proptest! {
        /// I1 and I3 over random deposit and withdrawal streams.
        ///
        /// Covers: I1, I3, I11, I13.
        #[test]
        fn i1_i3_hold_after_every_row(rows in prop::collection::vec(any_row(), 0..64)) {
            let mut engine = Engine::new();
            for row in rows {
                let (accounts_before, store_before) = snapshot(&engine);
                let result = engine.apply(row);
                assert_invariants(&engine);
                let (accounts_after, store_after) = snapshot(&engine);
                match (result, row) {
                    (Ok(()), _) => {}
                    (
                        Err(Rejection::InsufficientFunds | Rejection::AccountLocked),
                        Transaction::Withdrawal { client, tx, .. },
                    ) => {
                        // Exactly two effects: a zero account for an unseen
                        // client and the bound ID.
                        let mut expected = accounts_before.clone();
                        expected.entry(client).or_insert_with(Account::default);
                        prop_assert_eq!(&accounts_after, &expected);
                        prop_assert!(!store_before.contains_key(&tx));
                        prop_assert_eq!(Some(store_after.len()), store_before.len().checked_add(1));
                        prop_assert_eq!(
                            store_after.get(&tx).map(|(owner, _)| *owner),
                            Some(client)
                        );
                    }
                    (Err(_), _) => {
                        prop_assert_eq!(&accounts_after, &accounts_before);
                        prop_assert_eq!(&store_after, &store_before);
                    }
                }
            }
        }
    }
}
