//! The `Transaction` enum and the `ClientId` and `TxId` newtypes (ADR-0011 S5).
//!
//! Domain module of the layout in ADR-0011: no I/O, no `csv`, no serde.

use std::fmt::{self, Display, Formatter};

use crate::amount::Amount;

/// A client identifier: any `u16` (requirements §2).
///
/// A newtype, so a client ID cannot be passed where a transaction ID is
/// expected (ADR-0011 S5). Ordering is numeric, which gives the output its
/// ascending client order (ADR-0009 O3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientId(pub u16);

impl Display for ClientId {
    /// Prints the plain decimal ID, as ADR-0009 O4 requires.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A transaction identifier: any `u32`, globally unique in the input
/// (requirements §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TxId(pub u32);

/// One valid input row (ADR-0011 S5).
///
/// Only `Deposit` and `Withdrawal` carry an [`Amount`], so a dispute with an
/// amount or a deposit without one cannot be built. The amount field of a
/// `dispute`, `resolve`, or `chargeback` row is ignored by the input adapter
/// (ADR-0010 V5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transaction {
    /// Credits `amount` to the client's account.
    Deposit {
        client: ClientId,
        tx: TxId,
        amount: Amount,
    },
    /// Debits `amount` from the client's account, if the funds are available.
    Withdrawal {
        client: ClientId,
        tx: TxId,
        amount: Amount,
    },
    /// Opens a dispute on the deposit `tx`.
    Dispute { client: ClientId, tx: TxId },
    /// Resolves the open dispute on the deposit `tx`.
    Resolve { client: ClientId, tx: TxId },
    /// Charges back the disputed deposit `tx`.
    Chargeback { client: ClientId, tx: TxId },
}
