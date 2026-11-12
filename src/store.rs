//! Stored transactions, their dispute state, and the transition function (ADR-0011 S9 to S11, ADR-0006).
//!
//! Domain module of the layout in ADR-0011: no I/O, no `csv`, no serde.
//!
//! The store holds one record for every `deposit` and `withdrawal` that was
//! applied or rejected, which is exactly the set of `tx` IDs that ADR-0010 V6
//! binds (ADR-0011 S10). One lookup answers both questions a row can ask: is
//! this ID already bound, and is it a deposit that can be disputed.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use crate::amount::Amount;
use crate::transaction::{ClientId, TxId};

/// The dispute state of a stored deposit (ADR-0006).
///
/// Only `Processed` exists so far; the dispute lifecycle (E4, LYA-52) adds
/// `Disputed`, `Resolved`, and `ChargedBack` with the transition function of
/// ADR-0011 S11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeState {
    /// Stored and not under dispute.
    Processed,
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

    use super::{DisputeState, Store, StoredKind, StoredTx};
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

    /// ADR-0011 S9 records this size for its memory estimate.
    #[test]
    fn a_stored_transaction_is_16_bytes() {
        assert_eq!(size_of::<StoredTx>(), 16);
    }
}
