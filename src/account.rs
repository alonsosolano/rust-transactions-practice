//! `Account` and its checked balance changes; `total` is derived and the lock never reverts (ADR-0011 S8, ADR-0015 P9, ADR-0008).
//!
//! Domain module of the layout in ADR-0011: no I/O, no `csv`, no serde.
//!
//! An [`Account`] is a value. Each balance change returns a new account and
//! leaves the old one untouched, so the engine can compute every new value
//! before it commits anything (ADR-0011 S7).

use crate::amount::{Amount, Balance};

/// One client's asset account (requirements §4).
///
/// The fields are private and are set only by [`Account::from_parts`], which
/// derives `total` as `available + held` with checked arithmetic and refuses
/// any state where a value would overflow (ADR-0015 P9). So
/// `total = available + held` (invariant I1) holds for every account that can
/// exist. Only [`Account::charged_back`] sets `locked`, and no method sets it
/// back to `false` (I2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Account {
    available: Balance,
    held: Balance,
    total: Balance,
    locked: bool,
}

/// Why a withdrawal was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalError {
    /// The account is locked. Checked before the funds (ADR-0008 L3, L4).
    Locked,
    /// `available` is lower than the amount (requirements §5).
    InsufficientFunds,
    /// The new balance would overflow (ADR-0007 M7).
    Overflow,
}

impl Account {
    /// Builds an account from its balances, deriving `total`.
    ///
    /// Returns `None` if `available + held` overflows. This is the only place
    /// that sets the fields, which is what makes I1 hold by construction.
    fn from_parts(available: Balance, held: Balance, locked: bool) -> Option<Self> {
        let total = available.checked_add(held)?;
        Some(Self {
            available,
            held,
            total,
            locked,
        })
    }

    /// Funds available for withdrawal. May be negative (ADR-0006 R2).
    #[must_use]
    pub fn available(&self) -> Balance {
        self.available
    }

    /// Funds held by open disputes.
    #[must_use]
    pub fn held(&self) -> Balance {
        self.held
    }

    /// `available + held`, as computed when the balances last changed.
    #[must_use]
    pub fn total(&self) -> Balance {
        self.total
    }

    /// Whether a chargeback has locked the account (ADR-0008 L1).
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// The account after a deposit of `amount` (requirements §5).
    ///
    /// Deposits are accepted on a locked account too (ADR-0008 L2). Returns
    /// `None` if a balance would overflow (ADR-0007 M7).
    #[must_use]
    pub fn deposited(&self, amount: Amount) -> Option<Self> {
        let available = self.available.checked_add(Balance::from(amount))?;
        Self::from_parts(available, self.held, self.locked)
    }

    /// The account after a withdrawal of `amount` (requirements §5).
    ///
    /// # Errors
    ///
    /// The lock is checked first, then the funds (ADR-0008 L4): a locked
    /// account gives [`WithdrawalError::Locked`] whatever its balance. An
    /// amount above `available` gives [`WithdrawalError::InsufficientFunds`];
    /// an amount equal to `available` is applied. Held funds cannot be
    /// withdrawn.
    pub fn withdrawn(&self, amount: Amount) -> Result<Self, WithdrawalError> {
        if self.locked {
            return Err(WithdrawalError::Locked);
        }
        let amount = Balance::from(amount);
        if self.available < amount {
            return Err(WithdrawalError::InsufficientFunds);
        }
        self.available
            .checked_sub(amount)
            .and_then(|available| Self::from_parts(available, self.held, self.locked))
            .ok_or(WithdrawalError::Overflow)
    }

    /// The account after a dispute on a deposit of `amount` (ADR-0006).
    ///
    /// Moves the full amount from `available` to `held`, even if `available`
    /// goes negative (R2); `total` is unchanged. Applies the same way on a
    /// locked account (ADR-0008 L5). Returns `None` if a balance would
    /// overflow (ADR-0007 M7).
    #[must_use]
    pub fn disputed(&self, amount: Amount) -> Option<Self> {
        let amount = Balance::from(amount);
        let available = self.available.checked_sub(amount)?;
        let held = self.held.checked_add(amount)?;
        Self::from_parts(available, held, self.locked)
    }

    /// The account after a resolve on a disputed deposit of `amount`
    /// (ADR-0006).
    ///
    /// Moves the amount from `held` back to `available`; `total` is
    /// unchanged. A resolve never unlocks the account (ADR-0008 L1). Returns
    /// `None` if a balance would overflow (ADR-0007 M7).
    #[must_use]
    pub fn resolved(&self, amount: Amount) -> Option<Self> {
        let amount = Balance::from(amount);
        let available = self.available.checked_add(amount)?;
        let held = self.held.checked_sub(amount)?;
        Self::from_parts(available, held, self.locked)
    }

    /// The account after a chargeback on a disputed deposit of `amount`
    /// (ADR-0006).
    ///
    /// Removes the amount from `held`, so `total` falls by the same amount and
    /// may go negative (R2). The account is locked; an account that is
    /// already locked stays locked (ADR-0008 L1, L5). Returns `None` if a
    /// balance would overflow (ADR-0007 M7).
    #[must_use]
    pub fn charged_back(&self, amount: Amount) -> Option<Self> {
        let held = self.held.checked_sub(Balance::from(amount))?;
        Self::from_parts(self.available, held, true)
    }

    /// An account with the given balances, for tests that need states no
    /// deposit or withdrawal alone can reach (held funds, a lock, or balances
    /// near overflow).
    #[cfg(test)]
    pub(crate) fn with_balances(available: Balance, held: Balance, locked: bool) -> Option<Self> {
        Self::from_parts(available, held, locked)
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::{Account, WithdrawalError};
    use crate::amount::{Amount, Balance};

    fn amount(text: &str) -> Amount {
        Amount::parse(text.as_bytes()).unwrap()
    }

    fn balance(text: &str) -> Balance {
        Balance::from(amount(text))
    }

    /// Checks I1 on one account: `total = available + held`.
    fn assert_total(account: &Account) {
        assert_eq!(
            account.available().checked_add(account.held()),
            Some(account.total())
        );
    }

    #[test]
    fn a_new_account_is_zero_and_unlocked() {
        let account = Account::default();
        assert_eq!(account.available(), Balance::ZERO);
        assert_eq!(account.held(), Balance::ZERO);
        assert_eq!(account.total(), Balance::ZERO);
        assert!(!account.is_locked());
    }

    #[test]
    fn a_deposit_raises_available_and_total() {
        let account = Account::default().deposited(amount("2.5")).unwrap();
        assert_eq!(account.available(), balance("2.5"));
        assert_eq!(account.held(), Balance::ZERO);
        assert_eq!(account.total(), balance("2.5"));
        assert_total(&account);
    }

    #[test]
    fn a_withdrawal_lowers_available_and_total() {
        let account = Account::default().deposited(amount("10")).unwrap();
        let account = account.withdrawn(amount("2.5")).unwrap();
        assert_eq!(account.available(), balance("7.5"));
        assert_eq!(account.total(), balance("7.5"));
        assert_total(&account);
    }

    #[test]
    fn a_withdrawal_of_exactly_available_is_applied() {
        let account = Account::default().deposited(amount("1.2345")).unwrap();
        let account = account.withdrawn(amount("1.2345")).unwrap();
        assert_eq!(account.available(), Balance::ZERO);
        assert_eq!(account.total(), Balance::ZERO);
    }

    #[test]
    fn a_withdrawal_above_available_is_refused() {
        let account = Account::default().deposited(amount("1")).unwrap();
        assert_eq!(
            account.withdrawn(amount("1.0001")),
            Err(WithdrawalError::InsufficientFunds)
        );
        assert_eq!(
            Account::default().withdrawn(amount("0.0001")),
            Err(WithdrawalError::InsufficientFunds)
        );
    }

    #[test]
    fn held_funds_cannot_be_withdrawn() {
        let account = Account::with_balances(balance("5"), balance("100"), false).unwrap();
        assert_eq!(
            account.withdrawn(amount("5.0001")),
            Err(WithdrawalError::InsufficientFunds)
        );
    }

    /// ADR-0008 L3 and L4: the lock is checked before the funds.
    #[test]
    fn a_locked_account_refuses_every_withdrawal_as_locked() {
        let account = Account::with_balances(balance("100"), Balance::ZERO, true).unwrap();
        assert_eq!(account.withdrawn(amount("1")), Err(WithdrawalError::Locked));
        assert_eq!(
            account.withdrawn(amount("1000")),
            Err(WithdrawalError::Locked)
        );
    }

    /// ADR-0008 L2: a locked account still takes deposits and stays locked.
    #[test]
    fn a_locked_account_accepts_deposits() {
        let account = Account::with_balances(balance("1"), Balance::ZERO, true).unwrap();
        let account = account.deposited(amount("2")).unwrap();
        assert_eq!(account.available(), balance("3"));
        assert!(account.is_locked());
    }

    /// I6: a change that would overflow is refused, not wrapped.
    #[test]
    fn i6_a_deposit_that_would_overflow_is_refused() {
        let near_max = Balance::from_units(i128::MAX);
        let account = Account::with_balances(near_max, Balance::ZERO, false).unwrap();
        assert_eq!(account.deposited(Amount::MIN), None);
    }

    /// I6: `total` itself can overflow even when `available` does not.
    #[test]
    fn i6_a_total_that_would_overflow_is_refused() {
        let half = Balance::from_units(i128::MAX / 2 + 1);
        assert_eq!(Account::with_balances(half, half, false), None);
        let account =
            Account::with_balances(half, Balance::from_units(i128::MAX / 2), false).unwrap();
        assert_eq!(account.deposited(Amount::MIN), None);
    }

    /// I6: a withdrawal whose result would go below `i128::MIN` is refused.
    #[test]
    fn i6_a_withdrawal_that_would_overflow_is_refused() {
        let floor = Balance::from_units(i128::MIN);
        let account = Account::with_balances(floor, Balance::ZERO, false).unwrap();
        // `available` is far below the amount, so the funds check refuses it
        // first; overflow on a withdrawal is unreachable while that check
        // holds, and `withdrawn` still maps it to an error.
        assert_eq!(
            account.withdrawn(Amount::MIN),
            Err(WithdrawalError::InsufficientFunds)
        );
    }

    /// ADR-0006: a dispute moves the amount from `available` to `held` and
    /// keeps `total`.
    #[test]
    fn a_dispute_moves_funds_to_held() {
        let account = Account::default().deposited(amount("10")).unwrap();
        let account = account.disputed(amount("4")).unwrap();
        assert_eq!(account.available(), balance("6"));
        assert_eq!(account.held(), balance("4"));
        assert_eq!(account.total(), balance("10"));
        assert!(!account.is_locked());
        assert_total(&account);
    }

    /// ADR-0006 R2: the full amount is held even when `available` goes
    /// negative.
    #[test]
    fn r2_a_dispute_holds_the_full_amount_even_below_zero() {
        let account = Account::default().disputed(amount("100")).unwrap();
        assert_eq!(
            account.available(),
            Balance::ZERO.checked_sub(balance("100")).unwrap()
        );
        assert_eq!(account.held(), balance("100"));
        assert_eq!(account.total(), Balance::ZERO);
        assert_total(&account);
    }

    /// ADR-0006: a resolve moves the amount back from `held` to `available`.
    #[test]
    fn a_resolve_releases_held_funds() {
        let account = Account::default().deposited(amount("10")).unwrap();
        let account = account.disputed(amount("10")).unwrap();
        let account = account.resolved(amount("10")).unwrap();
        assert_eq!(account, Account::default().deposited(amount("10")).unwrap());
    }

    /// ADR-0008 L1: a resolve on a locked account leaves it locked.
    #[test]
    fn l1_a_resolve_never_unlocks() {
        let account = Account::with_balances(Balance::ZERO, balance("5"), true).unwrap();
        let account = account.resolved(amount("5")).unwrap();
        assert_eq!(account.available(), balance("5"));
        assert!(account.is_locked());
    }

    /// ADR-0006: a chargeback removes the held amount from `total` and locks.
    #[test]
    fn a_chargeback_debits_held_and_locks() {
        let account = Account::default().deposited(amount("10")).unwrap();
        let account = account.disputed(amount("10")).unwrap();
        let account = account.charged_back(amount("10")).unwrap();
        assert_eq!(account.available(), Balance::ZERO);
        assert_eq!(account.held(), Balance::ZERO);
        assert_eq!(account.total(), Balance::ZERO);
        assert!(account.is_locked());
        assert_total(&account);
    }

    /// ADR-0008 L5: a chargeback on a locked account applies and the lock
    /// stays.
    #[test]
    fn l5_a_chargeback_on_a_locked_account_applies() {
        let account = Account::with_balances(Balance::ZERO, balance("7"), true).unwrap();
        let account = account.charged_back(amount("7")).unwrap();
        assert_eq!(account.held(), Balance::ZERO);
        assert_eq!(account.total(), Balance::ZERO);
        assert!(account.is_locked());
    }

    /// I6: a dispute that would push `available` below `i128::MIN` is
    /// refused, not wrapped.
    #[test]
    fn i6_a_dispute_that_would_overflow_is_refused() {
        let floor = Balance::from_units(i128::MIN);
        let account = Account::with_balances(floor, Balance::ZERO, false).unwrap();
        assert_eq!(account.disputed(Amount::MIN), None);
    }

    /// I6: a resolve that would push `available` above `i128::MAX` is
    /// refused.
    #[test]
    fn i6_a_resolve_that_would_overflow_is_refused() {
        let near_max = Balance::from_units(i128::MAX);
        let account = Account::with_balances(near_max, Balance::ZERO, false).unwrap();
        assert_eq!(account.resolved(Amount::MIN), None);
    }

    /// I6: a chargeback that would push `held` below `i128::MIN` is refused.
    #[test]
    fn i6_a_chargeback_that_would_overflow_is_refused() {
        let floor = Balance::from_units(i128::MIN);
        let account = Account::with_balances(Balance::ZERO, floor, false).unwrap();
        assert_eq!(account.charged_back(Amount::MIN), None);
    }

    /// ADR-0015 P9 records this size for the memory estimate of ADR-0011.
    #[test]
    fn an_account_is_64_bytes() {
        assert_eq!(size_of::<Account>(), 64);
    }
}
