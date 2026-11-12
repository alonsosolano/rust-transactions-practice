//! Stored transactions, their dispute state, and the transition function (ADR-0011 S9 to S11, ADR-0006).
//!
//! Domain module of the layout in ADR-0011: no I/O, no `csv`, no serde.
//!
//! The store holds one record for every `deposit` and `withdrawal` that was
//! applied or rejected, which is exactly the set of `tx` IDs that ADR-0010 V6
//! binds (ADR-0011 S10). One lookup answers both questions a row can ask: is
//! this ID already bound, and is it a deposit that can be disputed.
//!
//! The dispute state of a deposit moves only through [`DisputeState::next`],
//! the transition function of ADR-0011 S11.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use crate::amount::Amount;
use crate::transaction::{ClientId, TxId};

/// The dispute state of a stored deposit (ADR-0006, state machine).
///
/// `ChargedBack` is terminal (R6). A `Resolved` deposit can be disputed again,
/// which opens a new cycle (R5). Moves between states happen only through
/// [`DisputeState::next`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeState {
    /// Stored and never disputed.
    Processed,
    /// Under dispute: its amount is in `held`.
    Disputed,
    /// The last dispute was resolved; the amount is back in `available`.
    Resolved,
    /// Charged back. Final.
    ChargedBack,
}

/// A dispute-related row, as seen by the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeAction {
    /// A `dispute` row.
    Dispute,
    /// A `resolve` row.
    Resolve,
    /// A `chargeback` row.
    Chargeback,
}

/// Why a dispute-related row cannot move a deposit to a new state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionError {
    /// A dispute on a deposit already under dispute (ADR-0006 R4).
    AlreadyDisputed,
    /// Any row on a charged-back deposit (ADR-0006 R6).
    AlreadyChargedBack,
    /// A resolve or chargeback on a deposit not under dispute (ADR-0006 R7).
    NotDisputed,
}

impl DisputeState {
    /// The state after `action`, or why the action does not apply.
    ///
    /// One exhaustive `match` on (state, action) with no wildcard arm
    /// (ADR-0011 S11): each of the twelve pairs is decided here and mirrors
    /// the state machine of ADR-0006. A new state or action does not compile
    /// until every new pair is decided.
    ///
    /// # Errors
    ///
    /// Returns the [`TransitionError`] for every pair that ADR-0006 ignores.
    pub fn next(self, action: DisputeAction) -> Result<Self, TransitionError> {
        use DisputeAction::{Chargeback, Dispute, Resolve};
        use DisputeState::{ChargedBack, Disputed, Processed, Resolved};
        use TransitionError::{AlreadyChargedBack, AlreadyDisputed, NotDisputed};

        match (self, action) {
            (Processed, Dispute) => Ok(Disputed),
            (Processed, Resolve) => Err(NotDisputed),
            (Processed, Chargeback) => Err(NotDisputed),
            (Disputed, Dispute) => Err(AlreadyDisputed),
            (Disputed, Resolve) => Ok(Resolved),
            (Disputed, Chargeback) => Ok(ChargedBack),
            (Resolved, Dispute) => Ok(Disputed),
            (Resolved, Resolve) => Err(NotDisputed),
            (Resolved, Chargeback) => Err(NotDisputed),
            (ChargedBack, Dispute) => Err(AlreadyChargedBack),
            (ChargedBack, Resolve) => Err(AlreadyChargedBack),
            (ChargedBack, Chargeback) => Err(AlreadyChargedBack),
        }
    }
}

/// What a bound `tx` ID refers to (ADR-0011 S9).
///
/// Only deposits carry a dispute state, so only deposits can ever enter the
/// dispute lifecycle (invariant I5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredKind {
    /// An applied deposit.
    Deposit(DisputeState),
    /// An applied withdrawal. Never disputable (ADR-0006 R1).
    Withdrawal,
    /// A rejected withdrawal. It binds its ID (ADR-0010 V6) and is otherwise
    /// treated as never stored (ADR-0006 R7).
    RejectedWithdrawal,
}

/// One bound `tx` ID: its amount, owner, and kind (ADR-0011 S9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredTx {
    amount: Amount,
    client: ClientId,
    kind: StoredKind,
}

impl StoredTx {
    /// A record for a `deposit` or `withdrawal` of `amount` by `client`.
    #[must_use]
    pub fn new(client: ClientId, amount: Amount, kind: StoredKind) -> Self {
        Self {
            amount,
            client,
            kind,
        }
    }

    /// The client that owns the `tx` ID: the first row to use it (ADR-0010 V6).
    #[must_use]
    pub fn client(&self) -> ClientId {
        self.client
    }

    /// The amount of the `deposit` or `withdrawal` row.
    #[must_use]
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// What the `tx` ID refers to, with the dispute state of a deposit.
    #[must_use]
    pub fn kind(&self) -> StoredKind {
        self.kind
    }
}

/// Every bound `tx` ID and what it refers to (ADR-0011 S9, S10).
///
/// A `HashMap` with the standard library's keyed hasher, so crafted IDs cannot
/// flood one bucket (ADR-0011, Hostile input).
#[derive(Debug, Default)]
pub struct Store {
    records: HashMap<TxId, StoredTx>,
}

impl Store {
    /// The record bound to `tx`, if any.
    #[must_use]
    pub fn get(&self, tx: TxId) -> Option<&StoredTx> {
        self.records.get(&tx)
    }

    /// Binds `tx` to `record` if the ID is still free.
    ///
    /// An ID that is already bound keeps its first record: a later row can
    /// never overwrite a stored deposit or reset its state (ADR-0010 V6). The
    /// engine checks for a duplicate before it calls this, so the check here
    /// is a second line of defense.
    pub fn bind(&mut self, tx: TxId, record: StoredTx) {
        if let Entry::Vacant(slot) = self.records.entry(tx) {
            slot.insert(record);
        }
    }

    /// Sets the dispute state of the deposit bound to `tx`.
    ///
    /// Does nothing if `tx` is unbound or not a deposit: only a deposit has a
    /// dispute state (invariant I5). The engine reads the record and computes
    /// the new state with [`DisputeState::next`] before it calls this.
    pub fn set_dispute_state(&mut self, tx: TxId, state: DisputeState) {
        if let Some(StoredTx {
            kind: StoredKind::Deposit(current),
            ..
        }) = self.records.get_mut(&tx)
        {
            *current = state;
        }
    }

    /// The number of bound IDs.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    /// Every bound ID and its record, in no particular order.
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (TxId, &StoredTx)> {
        self.records.iter().map(|(tx, record)| (*tx, record))
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::{DisputeAction, DisputeState, Store, StoredKind, StoredTx, TransitionError};
    use crate::amount::Amount;
    use crate::transaction::{ClientId, TxId};

    fn amount(text: &str) -> Amount {
        Amount::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn a_bound_id_is_found_with_its_record() {
        let mut store = Store::default();
        let record = StoredTx::new(
            ClientId(1),
            amount("5"),
            StoredKind::Deposit(DisputeState::Processed),
        );
        store.bind(TxId(7), record);
        assert_eq!(store.get(TxId(7)), Some(&record));
        assert_eq!(store.get(TxId(8)), None);
    }

    /// I11: once bound, no later bind changes the client, amount, or kind.
    #[test]
    fn i11_a_bound_id_keeps_its_first_record() {
        let mut store = Store::default();
        let first = StoredTx::new(
            ClientId(1),
            amount("5"),
            StoredKind::Deposit(DisputeState::Processed),
        );
        store.bind(TxId(7), first);
        store.bind(
            TxId(7),
            StoredTx::new(ClientId(2), amount("9"), StoredKind::Withdrawal),
        );
        let stored = store.get(TxId(7)).unwrap();
        assert_eq!(stored, &first);
        assert_eq!(stored.client(), ClientId(1));
        assert_eq!(stored.amount, amount("5"));
        assert!(matches!(
            stored.kind,
            StoredKind::Deposit(DisputeState::Processed)
        ));
        assert_eq!(store.len(), 1);
    }

    // ADR-0006 state machine: one test per (state, row type) pair (I16).

    /// ADR-0006: a dispute moves a processed deposit to `Disputed`.
    #[test]
    fn i16_dispute_on_processed_opens_a_dispute() {
        assert_eq!(
            DisputeState::Processed.next(DisputeAction::Dispute),
            Ok(DisputeState::Disputed)
        );
    }

    /// ADR-0006 R7: a resolve on a deposit never disputed is ignored.
    #[test]
    fn r7_resolve_on_processed_is_not_disputed() {
        assert_eq!(
            DisputeState::Processed.next(DisputeAction::Resolve),
            Err(TransitionError::NotDisputed)
        );
    }

    /// ADR-0006 R7: a chargeback on a deposit never disputed is ignored.
    #[test]
    fn r7_chargeback_on_processed_is_not_disputed() {
        assert_eq!(
            DisputeState::Processed.next(DisputeAction::Chargeback),
            Err(TransitionError::NotDisputed)
        );
    }

    /// ADR-0006 R4: a second dispute while one is open is ignored.
    #[test]
    fn r4_dispute_on_disputed_is_already_disputed() {
        assert_eq!(
            DisputeState::Disputed.next(DisputeAction::Dispute),
            Err(TransitionError::AlreadyDisputed)
        );
    }

    /// ADR-0006: a resolve closes the open dispute.
    #[test]
    fn i16_resolve_on_disputed_resolves() {
        assert_eq!(
            DisputeState::Disputed.next(DisputeAction::Resolve),
            Ok(DisputeState::Resolved)
        );
    }

    /// ADR-0006: a chargeback closes the open dispute for good.
    #[test]
    fn i16_chargeback_on_disputed_charges_back() {
        assert_eq!(
            DisputeState::Disputed.next(DisputeAction::Chargeback),
            Ok(DisputeState::ChargedBack)
        );
    }

    /// ADR-0006 R5: a dispute on a resolved deposit opens a new cycle.
    #[test]
    fn r5_dispute_on_resolved_opens_a_new_cycle() {
        assert_eq!(
            DisputeState::Resolved.next(DisputeAction::Dispute),
            Ok(DisputeState::Disputed)
        );
    }

    /// ADR-0006 R7: a second resolve after the dispute closed is ignored.
    #[test]
    fn r7_resolve_on_resolved_is_not_disputed() {
        assert_eq!(
            DisputeState::Resolved.next(DisputeAction::Resolve),
            Err(TransitionError::NotDisputed)
        );
    }

    /// ADR-0006 R7: a chargeback after the dispute was resolved is ignored.
    #[test]
    fn r7_chargeback_on_resolved_is_not_disputed() {
        assert_eq!(
            DisputeState::Resolved.next(DisputeAction::Chargeback),
            Err(TransitionError::NotDisputed)
        );
    }

    /// ADR-0006 R6: `ChargedBack` is terminal for every row type.
    #[test]
    fn r6_every_row_on_charged_back_is_ignored() {
        for action in [
            DisputeAction::Dispute,
            DisputeAction::Resolve,
            DisputeAction::Chargeback,
        ] {
            assert_eq!(
                DisputeState::ChargedBack.next(action),
                Err(TransitionError::AlreadyChargedBack),
                "{action:?}"
            );
        }
    }

    /// Only a deposit has a dispute state, so setting one on a withdrawal or
    /// an unbound ID changes nothing (I5).
    #[test]
    fn i5_only_a_deposit_takes_a_dispute_state() {
        let mut store = Store::default();
        let withdrawal = StoredTx::new(ClientId(1), amount("5"), StoredKind::Withdrawal);
        store.bind(TxId(1), withdrawal);
        store.set_dispute_state(TxId(1), DisputeState::Disputed);
        store.set_dispute_state(TxId(2), DisputeState::Disputed);
        assert_eq!(store.get(TxId(1)), Some(&withdrawal));
        assert_eq!(store.get(TxId(2)), None);

        let deposit = StoredTx::new(
            ClientId(1),
            amount("5"),
            StoredKind::Deposit(DisputeState::Processed),
        );
        store.bind(TxId(3), deposit);
        store.set_dispute_state(TxId(3), DisputeState::Disputed);
        assert_eq!(
            store.get(TxId(3)).map(StoredTx::kind),
            Some(StoredKind::Deposit(DisputeState::Disputed))
        );
    }

    /// ADR-0011 S9 records this size for its memory estimate.
    #[test]
    fn a_stored_transaction_is_16_bytes() {
        assert_eq!(size_of::<StoredTx>(), 16);
    }
}
