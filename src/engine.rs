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
    use crate::test_support::{Checked, amount_of_units, any_row, client_of, row_from, snapshot};
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

    /// Applies each row in order and returns each row's result. `Checked`
    /// checks every per-row invariant, including the money ledger of I14 and
    /// I15, after every step (ADR-0013 C4).
    fn run(engine: &mut Checked, rows: &[Transaction]) -> Vec<Result<(), Rejection>> {
        rows.iter().map(|&row| engine.apply(row)).collect()
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
        assert_eq!(
            run(&mut engine, &[dispute(9, 7)]),
            [Err(Rejection::UnknownTx)]
        );
        assert_eq!(engine.accounts().count(), 0);
    }

    /// A7, ADR-0006 R4: a deposit is never held twice.
    #[test]
    fn a7_second_dispute_is_ignored() {
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
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
        let mut engine = Checked::new();
        run(&mut engine, &[deposit(1, 1, "1")]);
        let floor =
            Account::with_balances(Balance::from_units(i128::MIN), Balance::ZERO, false).unwrap();
        engine.set_account(ClientId(1), floor);
        let before = snapshot(&engine);
        assert_eq!(engine.apply(dispute(1, 1)), Err(Rejection::Overflow));
        assert_eq!(snapshot(&engine), before);
    }

    /// Balance units near either end of the `i128` range, or small.
    fn extreme_units() -> impl Strategy<Value = i128> {
        let offset = prop_oneof![0_u64..=50_000, any::<u64>()];
        prop_oneof![
            offset
                .clone()
                .prop_map(|k| i128::MAX.checked_sub(i128::from(k)).unwrap()),
            offset
                .clone()
                .prop_map(|k| i128::MIN.checked_add(i128::from(k)).unwrap()),
            offset.prop_map(i128::from),
        ]
    }

    /// An opening account with `available` near the end of the range. `held`
    /// is zero, because I4 ties it to the disputed deposits, and there are
    /// none yet.
    fn extreme_account() -> impl Strategy<Value = Account> {
        (extreme_units(), any::<bool>()).prop_map(|(units, locked)| {
            Account::with_balances(Balance::from_units(units), Balance::ZERO, locked).unwrap()
        })
    }

    /// Amounts from one unit to the maximum, so that the sum of a seeded
    /// balance and an amount often leaves the `i128` range.
    fn wide_amount() -> impl Strategy<Value = Amount> {
        prop_oneof![
            Just(Amount::MAX),
            (1_u64..=50_000).prop_map(amount_of_units),
            (1_u64..=u64::MAX).prop_map(amount_of_units),
        ]
    }

    /// Row types weighted toward disputes, resolves, and chargebacks over a
    /// handful of IDs, so most deposits go through several dispute cycles
    /// and many accounts get locked.
    fn dispute_heavy_row() -> impl Strategy<Value = Transaction> {
        let kind = prop_oneof![
            1 => Just(0_u8),
            1 => Just(1_u8),
            3 => Just(2_u8),
            2 => Just(3_u8),
            2 => Just(4_u8),
        ];
        row_from(
            kind,
            0_u16..2,
            0_u32..6,
            (1_u64..=50_000).prop_map(amount_of_units),
        )
    }

    proptest! {
        /// Every per-row invariant that `Checked` verifies, over random
        /// streams of all five row types.
        ///
        /// Covers: I1, I2, I3, I4, I5, I8, I11, I13, I14, I15, I16.
        #[test]
        fn state_invariants_hold_after_every_row(rows in prop::collection::vec(any_row(), 0..64)) {
            let mut engine = Checked::new();
            for row in rows {
                let _ = engine.apply(row);
            }
        }

        /// I4 on its own: `held` equals the sum of the client's disputed
        /// deposits, on streams where most rows are dispute-related, so
        /// deposits are disputed, resolved, disputed again, and charged back.
        /// The same streams lock most accounts, which exercises I8 (no
        /// withdrawal on a locked account) and I16 (legal moves only).
        ///
        /// Covers: I4, I8, I16.
        #[test]
        fn i4_held_equals_disputed_deposits(rows in prop::collection::vec(dispute_heavy_row(), 0..64)) {
            let mut engine = Checked::new();
            for row in rows {
                let _ = engine.apply(row);
            }
        }

        /// I9: a deposit, dispute, resolve, or chargeback has the same effect
        /// on balances and transaction states whether or not the account is
        /// locked. Before each such row, a second engine replays the same
        /// rows and then flips the lock of the row's client; the row must
        /// give both engines the same result and the same balances and
        /// store, and leave every other account identical.
        ///
        /// Covers: I9.
        #[test]
        fn i9_the_lock_does_not_change_other_rows(rows in prop::collection::vec(any_row(), 1..48)) {
            for (index, &row) in rows.iter().enumerate() {
                if matches!(row, Transaction::Withdrawal { .. }) {
                    continue;
                }
                let client = client_of(row);
                let earlier = &rows[..index];
                let mut original = Engine::new();
                let mut flipped = Engine::new();
                for &earlier_row in earlier {
                    let _ = original.apply(earlier_row);
                    let _ = flipped.apply(earlier_row);
                }
                let Some(account) = original.parts().0.get(&client).copied() else {
                    continue;
                };
                let toggled =
                    Account::with_balances(account.available(), account.held(), !account.is_locked())
                        .unwrap();
                flipped.set_account(client, toggled);

                let result = original.apply(row);
                prop_assert_eq!(flipped.apply(row), result);
                let (mut accounts, store) = snapshot(&original);
                let (mut flipped_accounts, flipped_store) = snapshot(&flipped);
                prop_assert_eq!(store, flipped_store);
                let mine = accounts.remove(&client).unwrap();
                let theirs = flipped_accounts.remove(&client).unwrap();
                prop_assert_eq!(accounts, flipped_accounts);
                prop_assert_eq!(
                    (mine.available(), mine.held(), mine.total()),
                    (theirs.available(), theirs.held(), theirs.total())
                );
                // The flipped lock stays flipped, unless the row is an
                // applied chargeback, which locks both.
                let charged_back = result.is_ok() && matches!(row, Transaction::Chargeback { .. });
                prop_assert_eq!(theirs.is_locked(), charged_back || !account.is_locked());
            }
        }

        /// I6: accounts open with `available` near either end of the `i128`
        /// range, and amounts go up to the maximum, so deposits often would
        /// overflow. Every such row is refused with no effect (I3, checked by
        /// `Checked`), and every applied row keeps the totals equal to the
        /// checked ledger (I14, I15), so no balance ever wraps. With `held`
        /// at zero, as I4 requires of an account with no disputed deposit,
        /// only a deposit can overflow: `available + amount` of a resolve is
        /// at most `total`. The other rows' overflows need a seeded `held`
        /// and are covered by the `i6_` unit tests in `account` and `engine`.
        ///
        /// Covers: I6.
        #[test]
        fn i6_overflow_is_refused_and_changes_nothing(
            opening in prop::collection::btree_map(0_u16..3, extreme_account(), 0..=3),
            rows in prop::collection::vec(row_from(0_u8..5, 0_u16..3, 0_u32..12, wide_amount()), 0..48),
        ) {
            let mut engine = Checked::new();
            for (client, account) in opening {
                engine.set_account(ClientId(client), account);
            }
            for row in rows {
                let _ = engine.apply(row);
            }
        }
    }
}
