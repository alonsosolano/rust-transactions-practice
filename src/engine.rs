//! `Engine::apply` and `Rejection`: applies one transaction to the accounts and the store (ADR-0011 S6, S7).
//!
//! Domain module of the layout in ADR-0011: no I/O, no `csv`, no serde.

use std::collections::BTreeMap;

use crate::account::{Account, WithdrawalError};
use crate::amount::Amount;
use crate::store::{DisputeAction, DisputeState, Store, StoredKind, StoredTx, TransitionError};
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
    /// A dispute-related row whose `tx` is unbound or names a rejected
    /// withdrawal, which is treated as never stored (`unknown_tx`, ADR-0006
    /// R7, ADR-0011 S10). Ignored.
    UnknownTx,
    /// A dispute-related row from a client that does not own the `tx`
    /// (`client_mismatch`, ADR-0006 R3). Ignored.
    ClientMismatch {
        /// The client that owns the ID.
        owner: ClientId,
    },
    /// A dispute-related row on an applied withdrawal (`not_a_deposit`,
    /// ADR-0006 R1). Ignored.
    NotADeposit,
    /// A dispute on a deposit already under dispute (`already_disputed`,
    /// ADR-0006 R4). Ignored.
    AlreadyDisputed,
    /// A dispute-related row on a charged-back deposit
    /// (`already_charged_back`, ADR-0006 R6). Ignored.
    AlreadyChargedBack,
    /// A resolve or chargeback on a deposit not under dispute
    /// (`not_disputed`, ADR-0006 R7). Ignored.
    NotDisputed,
}

impl From<TransitionError> for Rejection {
    fn from(error: TransitionError) -> Self {
        match error {
            TransitionError::AlreadyDisputed => Self::AlreadyDisputed,
            TransitionError::AlreadyChargedBack => Self::AlreadyChargedBack,
            TransitionError::NotDisputed => Self::NotDisputed,
        }
    }
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
            Transaction::Dispute { client, tx } => {
                self.dispute_row(client, tx, DisputeAction::Dispute)
            }
            Transaction::Resolve { client, tx } => {
                self.dispute_row(client, tx, DisputeAction::Resolve)
            }
            Transaction::Chargeback { client, tx } => {
                self.dispute_row(client, tx, DisputeAction::Chargeback)
            }
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

    /// Applies a `dispute`, `resolve`, or `chargeback` row (ADR-0006).
    ///
    /// The checks run in a fixed order, and the first that fails is the
    /// reason (ADR-0016 G2): the `tx` must be stored, then owned by the row's
    /// client, then be a deposit, then be in a state the row can move. The
    /// ownership check comes right after the lookup, so a row from another
    /// client learns nothing about the transaction's kind or state.
    ///
    /// The lock is never read here: these rows behave the same on a locked
    /// account (ADR-0008 L5). Only a chargeback sets it.
    fn dispute_row(
        &mut self,
        client: ClientId,
        tx: TxId,
        action: DisputeAction,
    ) -> Result<(), Rejection> {
        let stored = self.store.get(tx).ok_or(Rejection::UnknownTx)?;
        let state = match stored.kind() {
            StoredKind::RejectedWithdrawal => return Err(Rejection::UnknownTx),
            StoredKind::Deposit(_) | StoredKind::Withdrawal if stored.client() != client => {
                return Err(Rejection::ClientMismatch {
                    owner: stored.client(),
                });
            }
            StoredKind::Withdrawal => return Err(Rejection::NotADeposit),
            StoredKind::Deposit(state) => state,
        };
        let next = state.next(action)?;

        // A stored deposit always has an account: `deposit` inserts the
        // account before it binds the ID.
        let current = self.account(client);
        let amount = stored.amount();
        let updated = match action {
            DisputeAction::Dispute => current.disputed(amount),
            DisputeAction::Resolve => current.resolved(amount),
            DisputeAction::Chargeback => current.charged_back(amount),
        }
        .ok_or(Rejection::Overflow)?;

        self.accounts.insert(client, updated);
        self.store.set_dispute_state(tx, next);
        Ok(())
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

    fn dispute(client: u16, tx: u32) -> Transaction {
        Transaction::Dispute {
            client: ClientId(client),
            tx: TxId(tx),
        }
    }

    fn resolve(client: u16, tx: u32) -> Transaction {
        Transaction::Resolve {
            client: ClientId(client),
            tx: TxId(tx),
        }
    }

    fn chargeback(client: u16, tx: u32) -> Transaction {
        Transaction::Chargeback {
            client: ClientId(client),
            tx: TxId(tx),
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

    // Dispute lifecycle (ADR-0006) and locked accounts (ADR-0008). Each test
    // reproduces the catalog row it is named after (requirements §6) and
    // states the rules it covers.

    /// A1, ADR-0006 R2: the debt of a withdrawn and charged-back deposit
    /// stays visible.
    #[test]
    fn a1_withdraw_then_chargeback() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[deposit(1, 1, "100"), withdrawal(1, 2, "100"), dispute(1, 1)],
        );
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(
            state(&engine, 1),
            row("-100.0000", "100.0000", "0.0000", false)
        );
        assert_eq!(run(&mut engine, &[chargeback(1, 1)]), [Ok(())]);
        assert_eq!(
            state(&engine, 1),
            row("-100.0000", "0.0000", "-100.0000", true)
        );
    }

    /// A2, ADR-0006 R2: no withdrawal while `available` is negative; the
    /// resolve restores the account.
    #[test]
    fn a2_withdrawal_rejected_while_available_is_negative() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[deposit(1, 1, "100"), withdrawal(1, 2, "100"), dispute(1, 1)],
        );
        assert_eq!(
            run(&mut engine, &[withdrawal(1, 3, "10")]),
            [Err(Rejection::InsufficientFunds)]
        );
        assert_eq!(
            state(&engine, 1),
            row("-100.0000", "100.0000", "0.0000", false)
        );
        assert_eq!(run(&mut engine, &[resolve(1, 1)]), [Ok(())]);
        assert_eq!(state(&engine, 1), row("0.0000", "0.0000", "0.0000", false));
    }

    /// A3, ADR-0006 R1: a withdrawal is never disputable.
    #[test]
    fn a3_rows_on_a_withdrawal_are_ignored() {
        let mut engine = Engine::new();
        run(&mut engine, &[deposit(1, 1, "100"), withdrawal(1, 2, "40")]);
        let results = run(
            &mut engine,
            &[dispute(1, 2), resolve(1, 2), chargeback(1, 2)],
        );
        assert_eq!(results, [Err(Rejection::NotADeposit); 3]);
        assert_eq!(
            state(&engine, 1),
            row("60.0000", "0.0000", "60.0000", false)
        );
    }

    /// A4, ADR-0006 R3, R8: another client cannot dispute the deposit, and
    /// the ignored row opens no account.
    #[test]
    fn a4_cross_client_dispute_is_ignored() {
        let mut engine = Engine::new();
        run(&mut engine, &[deposit(1, 1, "10")]);
        assert_eq!(
            run(&mut engine, &[dispute(2, 1)]),
            [Err(Rejection::ClientMismatch { owner: ClientId(1) })]
        );
        assert_eq!(
            state(&engine, 1),
            row("10.0000", "0.0000", "10.0000", false)
        );
        assert_eq!(state(&engine, 2), None);
    }

    /// A5, ADR-0006 R3: another client can neither resolve nor charge back
    /// an open dispute.
    #[test]
    fn a5_cross_client_resolve_and_chargeback_are_ignored() {
        let mut engine = Engine::new();
        run(&mut engine, &[deposit(1, 1, "10"), dispute(1, 1)]);
        let results = run(&mut engine, &[resolve(2, 1), chargeback(2, 1)]);
        assert_eq!(
            results,
            [Err(Rejection::ClientMismatch { owner: ClientId(1) }); 2]
        );
        assert_eq!(
            state(&engine, 1),
            row("0.0000", "10.0000", "10.0000", false)
        );
        assert_eq!(state(&engine, 2), None);
    }

    /// A6, ADR-0006 R7, R8: a dispute on an unknown `tx` is ignored and opens
    /// no account.
    #[test]
    fn a6_dispute_on_unknown_tx_opens_no_account() {
        let mut engine = Engine::new();
        assert_eq!(
            run(&mut engine, &[dispute(9, 7)]),
            [Err(Rejection::UnknownTx)]
        );
        assert_eq!(engine.accounts().count(), 0);
    }

    /// A7, ADR-0006 R4: a deposit is never held twice.
    #[test]
    fn a7_second_dispute_is_ignored() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[deposit(1, 1, "10"), dispute(1, 1), dispute(1, 1)],
        );
        assert_eq!(results[2], Err(Rejection::AlreadyDisputed));
        assert_eq!(
            state(&engine, 1),
            row("0.0000", "10.0000", "10.0000", false)
        );
    }

    /// A8, ADR-0006 R5: a dispute after a resolve opens a new cycle that can
    /// be charged back.
    #[test]
    fn a8_redispute_after_resolve_opens_a_new_cycle() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[
                deposit(1, 1, "10"),
                dispute(1, 1),
                resolve(1, 1),
                dispute(1, 1),
            ],
        );
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(
            state(&engine, 1),
            row("0.0000", "10.0000", "10.0000", false)
        );
        assert_eq!(run(&mut engine, &[chargeback(1, 1)]), [Ok(())]);
        assert_eq!(state(&engine, 1), row("0.0000", "0.0000", "0.0000", true));
    }

    /// A9, ADR-0006 R6: a charged-back deposit is final; only the first
    /// chargeback applies.
    #[test]
    fn a9_charged_back_is_terminal() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[
                deposit(1, 1, "10"),
                deposit(1, 2, "5"),
                dispute(1, 1),
                chargeback(1, 1),
            ],
        );
        let results = run(
            &mut engine,
            &[chargeback(1, 1), dispute(1, 1), resolve(1, 1)],
        );
        assert_eq!(results, [Err(Rejection::AlreadyChargedBack); 3]);
        assert_eq!(state(&engine, 1), row("5.0000", "0.0000", "5.0000", true));
    }

    /// A10, ADR-0006 R7: rows on a rejected withdrawal, an unknown `tx`, or a
    /// deposit not under dispute are ignored.
    #[test]
    fn a10_rows_without_a_disputable_target_are_ignored() {
        let mut engine = Engine::new();
        run(&mut engine, &[deposit(1, 1, "5"), withdrawal(1, 2, "10")]);
        let results = run(
            &mut engine,
            &[
                dispute(1, 2),
                dispute(1, 99),
                resolve(1, 1),
                chargeback(1, 1),
            ],
        );
        assert_eq!(
            results,
            [
                Err(Rejection::UnknownTx),
                Err(Rejection::UnknownTx),
                Err(Rejection::NotDisputed),
                Err(Rejection::NotDisputed),
            ]
        );
        assert_eq!(state(&engine, 1), row("5.0000", "0.0000", "5.0000", false));
    }

    /// A11, ADR-0006 R9, ADR-0008 L2, L3: a locked account accepts a deposit
    /// and rejects a withdrawal.
    #[test]
    fn a11_locked_account_accepts_deposits_and_rejects_withdrawals() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[
                deposit(1, 1, "10"),
                deposit(1, 2, "5"),
                dispute(1, 1),
                chargeback(1, 1),
            ],
        );
        assert_eq!(state(&engine, 1), row("5.0000", "0.0000", "5.0000", true));
        let results = run(&mut engine, &[deposit(1, 3, "20"), withdrawal(1, 4, "5")]);
        assert_eq!(results, [Ok(()), Err(Rejection::AccountLocked)]);
        assert_eq!(state(&engine, 1), row("25.0000", "0.0000", "25.0000", true));
    }

    /// A12, ADR-0006 R9, ADR-0008 L5: dispute-related rows keep working on a
    /// locked account.
    #[test]
    fn a12_resolve_on_a_locked_account_applies() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[
                deposit(1, 1, "10"),
                deposit(1, 2, "10"),
                dispute(1, 1),
                dispute(1, 2),
                chargeback(1, 1),
            ],
        );
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(state(&engine, 1), row("0.0000", "10.0000", "10.0000", true));
        assert_eq!(run(&mut engine, &[resolve(1, 2)]), [Ok(())]);
        assert_eq!(state(&engine, 1), row("10.0000", "0.0000", "10.0000", true));
    }

    /// A13, ADR-0006 R2, R9, ADR-0008 L1: a deposit repays the debt, but the
    /// lock stays and withdrawals stay rejected.
    #[test]
    fn a13_repaying_the_debt_never_unlocks() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[
                deposit(1, 1, "100"),
                withdrawal(1, 2, "100"),
                dispute(1, 1),
                chargeback(1, 1),
            ],
        );
        let results = run(&mut engine, &[deposit(1, 3, "150"), withdrawal(1, 4, "10")]);
        assert_eq!(results, [Ok(()), Err(Rejection::AccountLocked)]);
        assert_eq!(state(&engine, 1), row("50.0000", "0.0000", "50.0000", true));
    }

    /// A14, ADR-0006 R2, R5: a second cycle after a resolve and a withdrawal
    /// records the debt and locks.
    #[test]
    fn a14_redispute_after_withdrawal_records_the_debt() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[
                deposit(1, 1, "100"),
                dispute(1, 1),
                resolve(1, 1),
                withdrawal(1, 2, "100"),
            ],
        );
        assert_eq!(state(&engine, 1), row("0.0000", "0.0000", "0.0000", false));
        assert_eq!(run(&mut engine, &[dispute(1, 1)]), [Ok(())]);
        assert_eq!(
            state(&engine, 1),
            row("-100.0000", "100.0000", "0.0000", false)
        );
        assert_eq!(run(&mut engine, &[chargeback(1, 1)]), [Ok(())]);
        assert_eq!(
            state(&engine, 1),
            row("-100.0000", "0.0000", "-100.0000", true)
        );
    }

    /// A23, ADR-0008 L5, ADR-0006 R2: every chargeback after the first still
    /// applies, so the full debt shows.
    #[test]
    fn a23_later_chargebacks_apply_on_a_locked_account() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[
                deposit(1, 1, "100"),
                deposit(1, 2, "100"),
                deposit(1, 3, "100"),
                withdrawal(1, 4, "300"),
                dispute(1, 1),
                dispute(1, 2),
                dispute(1, 3),
                chargeback(1, 1),
            ],
        );
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(
            state(&engine, 1),
            row("-300.0000", "200.0000", "-100.0000", true)
        );
        let results = run(&mut engine, &[chargeback(1, 2), chargeback(1, 3)]);
        assert_eq!(results, [Ok(()), Ok(())]);
        assert_eq!(
            state(&engine, 1),
            row("-300.0000", "0.0000", "-300.0000", true)
        );
    }

    /// A24, ADR-0008 L1, L2, L5: a deposit made after the lock is applied and
    /// stays disputable; its chargeback leaves the lock in place.
    #[test]
    fn a24_deposit_after_the_lock_is_disputable() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[deposit(1, 1, "10"), dispute(1, 1), chargeback(1, 1)],
        );
        assert_eq!(state(&engine, 1), row("0.0000", "0.0000", "0.0000", true));
        assert_eq!(run(&mut engine, &[deposit(1, 2, "50")]), [Ok(())]);
        assert_eq!(state(&engine, 1), row("50.0000", "0.0000", "50.0000", true));
        assert_eq!(run(&mut engine, &[dispute(1, 2)]), [Ok(())]);
        assert_eq!(state(&engine, 1), row("0.0000", "50.0000", "50.0000", true));
        assert_eq!(run(&mut engine, &[chargeback(1, 2)]), [Ok(())]);
        assert_eq!(state(&engine, 1), row("0.0000", "0.0000", "0.0000", true));
    }

    /// A25, ADR-0008 L3, L4, L7, ADR-0006 R7: every withdrawal on a locked
    /// account is rejected as locked, even one that also lacks funds, and is
    /// never stored as disputable.
    #[test]
    fn a25_locked_withdrawals_report_the_lock() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[
                deposit(1, 1, "10"),
                deposit(1, 2, "5"),
                dispute(1, 1),
                chargeback(1, 1),
            ],
        );
        assert_eq!(state(&engine, 1), row("5.0000", "0.0000", "5.0000", true));
        let results = run(
            &mut engine,
            &[
                withdrawal(1, 3, "100"),
                withdrawal(1, 4, "1"),
                dispute(1, 3),
            ],
        );
        assert_eq!(
            results,
            [
                Err(Rejection::AccountLocked),
                Err(Rejection::AccountLocked),
                Err(Rejection::UnknownTx),
            ]
        );
        assert_eq!(state(&engine, 1), row("5.0000", "0.0000", "5.0000", true));
    }

    /// A27, ADR-0010 V6, ADR-0006 R3: a duplicate deposit cannot take over
    /// the ID, so the other client's dispute is ignored.
    #[test]
    fn a27_duplicate_deposit_cannot_take_over_the_id() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[deposit(1, 1, "10"), deposit(2, 1, "50"), dispute(2, 1)],
        );
        assert_eq!(
            results,
            [
                Ok(()),
                Err(Rejection::DuplicateTx { owner: ClientId(1) }),
                Err(Rejection::ClientMismatch { owner: ClientId(1) }),
            ]
        );
        assert_eq!(
            state(&engine, 1),
            row("10.0000", "0.0000", "10.0000", false)
        );
        assert_eq!(state(&engine, 2), None);
    }

    /// A28, ADR-0010 V6, ADR-0006 R6: a repeated deposit cannot reset a
    /// charged-back deposit.
    #[test]
    fn a28_repeated_deposit_cannot_reset_a_chargeback() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[deposit(1, 1, "10"), dispute(1, 1), chargeback(1, 1)],
        );
        let results = run(&mut engine, &[deposit(1, 1, "10"), dispute(1, 1)]);
        assert_eq!(
            results,
            [
                Err(Rejection::DuplicateTx { owner: ClientId(1) }),
                Err(Rejection::AlreadyChargedBack),
            ]
        );
        assert_eq!(state(&engine, 1), row("0.0000", "0.0000", "0.0000", true));
    }

    /// A38, ADR-0006 R7: once resolved, a second resolve and a chargeback are
    /// ignored and the account stays unlocked.
    #[test]
    fn a38_resolve_and_chargeback_after_resolve_are_ignored() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[deposit(1, 1, "10"), dispute(1, 1), resolve(1, 1)],
        );
        assert_eq!(
            state(&engine, 1),
            row("10.0000", "0.0000", "10.0000", false)
        );
        let results = run(&mut engine, &[resolve(1, 1), chargeback(1, 1)]);
        assert_eq!(results, [Err(Rejection::NotDisputed); 2]);
        assert_eq!(
            state(&engine, 1),
            row("10.0000", "0.0000", "10.0000", false)
        );
    }

    /// A39, requirements §5: held funds cannot be withdrawn; a withdrawal of
    /// exactly `available` is applied.
    #[test]
    fn a39_held_funds_cannot_be_withdrawn() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[deposit(1, 1, "100"), deposit(1, 2, "50"), dispute(1, 1)],
        );
        assert_eq!(
            state(&engine, 1),
            row("50.0000", "100.0000", "150.0000", false)
        );
        let results = run(
            &mut engine,
            &[withdrawal(1, 3, "50.0001"), withdrawal(1, 4, "50")],
        );
        assert_eq!(results, [Err(Rejection::InsufficientFunds), Ok(())]);
        assert_eq!(
            state(&engine, 1),
            row("0.0000", "100.0000", "100.0000", false)
        );
    }

    /// A41, ADR-0010 V6, ADR-0006 R3: the first row in the file owns the ID.
    #[test]
    fn a41_first_row_owns_the_id() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[deposit(2, 1, "50"), deposit(1, 1, "10"), dispute(1, 1)],
        );
        assert_eq!(
            results,
            [
                Ok(()),
                Err(Rejection::DuplicateTx { owner: ClientId(2) }),
                Err(Rejection::ClientMismatch { owner: ClientId(2) }),
            ]
        );
        assert_eq!(
            state(&engine, 2),
            row("50.0000", "0.0000", "50.0000", false)
        );
        assert_eq!(state(&engine, 1), None);
    }

    /// A46, ADR-0016 G1 to G3, ADR-0006 R3: ownership is checked right after
    /// the lookup, so another client learns nothing about the kind or state
    /// of a transaction; a rejected withdrawal is unknown to everyone.
    #[test]
    fn a46_ownership_is_checked_before_kind_and_state() {
        let mut engine = Engine::new();
        let results = run(
            &mut engine,
            &[
                deposit(1, 1, "10"),
                withdrawal(1, 2, "1"),
                deposit(1, 3, "1"),
                dispute(1, 3),
                chargeback(1, 3),
                withdrawal(1, 4, "50"),
            ],
        );
        assert_eq!(results[5], Err(Rejection::AccountLocked));
        let before = snapshot(&engine);
        let results = run(
            &mut engine,
            &[dispute(2, 2), resolve(2, 1), dispute(2, 3), dispute(2, 4)],
        );
        let mismatch = Err(Rejection::ClientMismatch { owner: ClientId(1) });
        assert_eq!(
            results,
            [mismatch, mismatch, mismatch, Err(Rejection::UnknownTx)]
        );
        assert_eq!(snapshot(&engine), before);
        assert_eq!(state(&engine, 1), row("9.0000", "0.0000", "9.0000", true));
        assert_eq!(state(&engine, 2), None);
    }

    /// I3: an ignored dispute-related row leaves every balance and every
    /// transaction state unchanged.
    #[test]
    fn i3_an_ignored_dispute_row_changes_nothing() {
        let mut engine = Engine::new();
        run(
            &mut engine,
            &[deposit(1, 1, "10"), withdrawal(1, 2, "1"), dispute(1, 1)],
        );
        let before = snapshot(&engine);
        let results = run(
            &mut engine,
            &[
                dispute(1, 1),
                dispute(1, 2),
                dispute(2, 1),
                dispute(1, 9),
                resolve(1, 2),
            ],
        );
        assert!(results.iter().all(Result::is_err));
        assert_eq!(snapshot(&engine), before);
    }

    /// I6, ADR-0007 M7: a dispute whose effect would overflow is ignored and
    /// leaves the deposit's state unchanged.
    #[test]
    fn i6_an_overflowing_dispute_changes_nothing() {
        let mut engine = Engine::new();
        run(&mut engine, &[deposit(1, 1, "1")]);
        let floor =
            Account::with_balances(Balance::from_units(i128::MIN), Balance::ZERO, false).unwrap();
        engine.set_account(ClientId(1), floor);
        let before = snapshot(&engine);
        assert_eq!(engine.apply(dispute(1, 1)), Err(Rejection::Overflow));
        assert_eq!(snapshot(&engine), before);
    }

    /// One generated row of any of the five types over small pools of
    /// clients, IDs, and amounts, so duplicates, refusals, cross-client
    /// references, and every dispute transition are common (ADR-0014 T11).
    fn any_row() -> impl Strategy<Value = Transaction> {
        (0_u8..5, 0_u16..4, 0_u32..24, 1_u64..=50_000).prop_map(|(kind, client, tx, units)| {
            let text = format!("{}.{:04}", units / 10_000, units % 10_000);
            let amount = Amount::parse(text.as_bytes()).unwrap();
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

    proptest! {
        /// I1, I2, and I3 over random streams of all five row types.
        ///
        /// Covers: I1, I2, I3, I4, I11, I13.
        #[test]
        fn i1_i2_i3_hold_after_every_row(rows in prop::collection::vec(any_row(), 0..64)) {
            let mut engine = Engine::new();
            for row in rows {
                let (accounts_before, store_before) = snapshot(&engine);
                let result = engine.apply(row);
                assert_invariants(&engine);
                let (accounts_after, store_after) = snapshot(&engine);
                // I2: no account goes from locked back to unlocked.
                for (client, before) in &accounts_before {
                    if before.is_locked() {
                        prop_assert!(accounts_after.get(client).is_some_and(Account::is_locked));
                    }
                }
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
