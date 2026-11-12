//! Large-input generator for the efficiency measurements (ADR-0014 T12).
//!
//! ```sh
//! cargo run --release --example generate -- <rows> <seed> <profile> > input.csv
//! ```
//!
//! Writes a header and `<rows>` data rows to stdout. The same arguments always
//! give the same bytes. Memory is constant whatever `<rows>` is: no row is
//! kept, and every value a later row needs is recomputed from a counter.
//!
//! Profiles:
//!
//! * `mixed`: a realistic mix of all five row types over all 65,536 clients,
//!   plus a few invalid rows and duplicates (see [`MIXED`]). Disputes name
//!   earlier deposits and their real owners, and resolves and chargebacks
//!   name deposits under dispute, so every rule path is exercised.
//! * `stateless`: one deposit, then only rows that bind no `tx` ID (ADR-0010
//!   V6, ADR-0011 S10): disputes, resolves, and chargebacks on unknown IDs,
//!   invalid rows, and duplicates of the one bound ID. Memory of the engine
//!   on this profile must not grow with `<rows>`.
//!
//! Random numbers come from SplitMix64, written here with `wrapping_*`
//! arithmetic, so the generator needs no crate (ADR-0012).

use std::env;
use std::io::{self, BufWriter, Write};
use std::process::ExitCode;

const USAGE: &str = "usage: generate <rows> <seed> <mixed|stateless>";

/// Number of client IDs: every `u16`.
const CLIENTS: u64 = 1 << 16;

/// Of every 9 money rows (`deposit` or `withdrawal`), the first 5 are
/// deposits, so the type of money row `k` is known from `k` alone.
const MONEY_CYCLE: u32 = 9;
const DEPOSITS_PER_CYCLE: u32 = 5;

/// Open disputes remembered for later resolves and chargebacks. A fixed
/// bound keeps the generator's memory constant.
const OPEN_DISPUTES: usize = 256;

/// The share of each row kind, in rows per 1,000.
#[derive(Debug, Clone, Copy)]
struct Mix {
    money: u64,
    dispute: u64,
    resolve: u64,
    chargeback: u64,
    invalid: u64,
    duplicate: u64,
}

impl Mix {
    /// The sum of all shares; every profile must total 1,000.
    const fn total(self) -> u64 {
        self.money
            .saturating_add(self.dispute)
            .saturating_add(self.resolve)
            .saturating_add(self.chargeback)
            .saturating_add(self.invalid)
            .saturating_add(self.duplicate)
    }
}

// The last kind of each profile takes whatever the others leave, so a share
// that does not add up would silently change the mix.
const _: () = assert!(MIXED.total() == 1000 && STATELESS.total() == 1000);

/// The `mixed` profile: 50% deposits and 40% withdrawals (split by
/// [`MONEY_CYCLE`]), 5% disputes, 3% resolves, 0.5% chargebacks, 1% invalid
/// rows, and 0.5% duplicates. Chargebacks stay below 1% of rows, the order of
/// the card networks' monitoring thresholds, so most accounts stay unlocked.
const MIXED: Mix = Mix {
    money: 900,
    dispute: 50,
    resolve: 30,
    chargeback: 5,
    invalid: 10,
    duplicate: 5,
};

/// The `stateless` profile: rows that bind no `tx` ID only.
const STATELESS: Mix = Mix {
    money: 0,
    dispute: 300,
    resolve: 150,
    chargeback: 150,
    invalid: 250,
    duplicate: 150,
};

/// Which rows to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profile {
    Mixed,
    Stateless,
}

/// The SplitMix64 generator (Steele, Lea, and Flood, 2014).
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.state)
    }

    /// A number in `0..bound`. `bound` must not be zero; a zero bound gives 0.
    fn below(&mut self, bound: u64) -> u64 {
        self.next().checked_rem(bound).unwrap_or(0)
    }
}

/// The SplitMix64 output function: a bijective mix of the bits of `z`.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The upper 32 bits of `value`.
fn high_u32(value: u64) -> u32 {
    u32::try_from(value >> 32).unwrap_or(0)
}

/// The upper 16 bits of `value`.
fn high_u16(value: u64) -> u16 {
    u16::try_from(value >> 48).unwrap_or(0)
}

/// Derives the `tx` ID and the owning client of money row `k` from `k`.
///
/// Multiplying by an odd constant is a bijection on `u32`, and so is the XOR
/// with a key, so the first 2^32 money rows get distinct IDs. The IDs are not
/// ordered and span the full `u32` range, as real inputs may.
struct Ids {
    tx_key: u32,
    client_key: u64,
}

impl Ids {
    fn new(seed: u64) -> Self {
        Self {
            tx_key: high_u32(mix(seed ^ 0x7478)),
            client_key: mix(seed ^ 0x636C),
        }
    }

    fn tx(&self, k: u32) -> u32 {
        k.wrapping_mul(0x9E37_79B1) ^ self.tx_key
    }

    fn client(&self, k: u32) -> u16 {
        high_u16(mix(self.client_key ^ u64::from(k)))
    }
}

/// Whether money row `k` is a deposit.
fn is_deposit(k: u32) -> bool {
    k.checked_rem(MONEY_CYCLE)
        .is_some_and(|slot| slot < DEPOSITS_PER_CYCLE)
}

/// Writes `rows` data rows of `profile` after the header.
fn generate<W: Write>(rows: u64, seed: u64, profile: Profile, output: W) -> io::Result<()> {
    let mut out = BufWriter::new(output);
    writeln!(out, "type,client,tx,amount")?;
    let mut state = State::new(seed);
    match profile {
        Profile::Mixed => {
            for _ in 0..rows {
                state.mixed_row(&mut out)?;
            }
        }
        Profile::Stateless => {
            // The one bound ID that every duplicate reuses.
            if rows > 0 {
                writeln!(out, "deposit,1,{},1.0", state.ids.tx(0))?;
            }
            for _ in 1..rows {
                state.stateless_row(&mut out)?;
            }
        }
    }
    out.flush()
}

/// What the generator remembers between rows. Its size is fixed.
struct State {
    rng: SplitMix64,
    ids: Ids,
    /// The number of money rows written so far.
    money_rows: u32,
    /// Up to [`OPEN_DISPUTES`] deposits under dispute, as (client, tx).
    open: Vec<(u16, u32)>,
}

impl State {
    fn new(seed: u64) -> Self {
        Self {
            rng: SplitMix64::new(seed),
            ids: Ids::new(seed),
            money_rows: 0,
            open: Vec::with_capacity(OPEN_DISPUTES),
        }
    }

    fn mixed_row<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        let mut pick = self.rng.below(1000);
        // Rows that name an earlier money row need one to exist.
        if self.money_rows == 0 {
            pick = 0;
        }
        let mix = MIXED;
        let mut limit = mix.money;
        if pick < limit {
            return self.money_row(out);
        }
        limit = limit.saturating_add(mix.dispute);
        if pick < limit {
            return self.dispute(out);
        }
        limit = limit.saturating_add(mix.resolve);
        if pick < limit {
            return self.close_dispute(out, "resolve");
        }
        limit = limit.saturating_add(mix.chargeback);
        if pick < limit {
            return self.close_dispute(out, "chargeback");
        }
        limit = limit.saturating_add(mix.invalid);
        if pick < limit {
            return self.invalid(out);
        }
        let earlier = self.earlier_money_row();
        self.duplicate(out, self.ids.tx(earlier))
    }

    fn stateless_row<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        let pick = self.rng.below(1000);
        let mix = STATELESS;
        let mut limit = mix.dispute;
        if pick < limit {
            return self.unknown_reference(out, "dispute");
        }
        limit = limit.saturating_add(mix.resolve);
        if pick < limit {
            return self.unknown_reference(out, "resolve");
        }
        limit = limit.saturating_add(mix.chargeback);
        if pick < limit {
            return self.unknown_reference(out, "chargeback");
        }
        limit = limit.saturating_add(mix.invalid);
        if pick < limit {
            return self.invalid(out);
        }
        self.duplicate(out, self.ids.tx(0))
    }

    /// A deposit or withdrawal with a new ID. Withdrawals are smaller than
    /// deposits, so most of them succeed.
    fn money_row<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        let k = self.money_rows;
        self.money_rows = k.wrapping_add(1);
        let client = self.ids.client(k);
        let tx = self.ids.tx(k);
        let fraction = self.rng.below(10_000);
        if is_deposit(k) {
            let whole = self.rng.below(999).saturating_add(1);
            writeln!(out, "deposit,{client},{tx},{whole}.{fraction:04}")
        } else {
            let whole = self.rng.below(100);
            writeln!(out, "withdrawal,{client},{tx},{whole}.{fraction:04}")
        }
    }

    /// A dispute on an earlier deposit, by its owner.
    fn dispute<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        let k = self.earlier_deposit();
        let client = self.ids.client(k);
        let tx = self.ids.tx(k);
        if self.open.len() < OPEN_DISPUTES {
            self.open.push((client, tx));
        } else {
            let slot = self.slot(self.open.len());
            if let Some(entry) = self.open.get_mut(slot) {
                *entry = (client, tx);
            }
        }
        writeln!(out, "dispute,{client},{tx},")
    }

    /// A resolve or chargeback on a deposit under dispute, or a dispute when
    /// none is open.
    fn close_dispute<W: Write>(&mut self, out: &mut W, kind: &str) -> io::Result<()> {
        let slot = self.slot(self.open.len());
        if slot < self.open.len() {
            let (client, tx) = self.open.swap_remove(slot);
            writeln!(out, "{kind},{client},{tx},")
        } else {
            self.dispute(out)
        }
    }

    /// A dispute-related row on an ID that no row binds.
    fn unknown_reference<W: Write>(&mut self, out: &mut W, kind: &str) -> io::Result<()> {
        let client = self.client();
        let mut tx = high_u32(self.rng.next());
        if tx == self.ids.tx(0) {
            tx ^= 1;
        }
        writeln!(out, "{kind},{client},{tx},")
    }

    /// A deposit or withdrawal that reuses the bound ID `tx` (ADR-0010 V6).
    fn duplicate<W: Write>(&mut self, out: &mut W, tx: u32) -> io::Result<()> {
        let client = self.client();
        let kind = if self.rng.below(2) == 0 {
            "deposit"
        } else {
            "withdrawal"
        };
        writeln!(out, "{kind},{client},{tx},1.0")
    }

    /// A row that ADR-0010 classifies as invalid. It binds no ID.
    fn invalid<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        let client = self.client();
        let tx = high_u32(self.rng.next());
        match self.rng.below(5) {
            0 => writeln!(out, "transfer,{client},{tx},1.0"),
            1 => writeln!(out, "deposit,{client},{tx}"),
            2 => writeln!(out, "deposit,{client},{tx},1.00001"),
            3 => writeln!(out, "deposit,70000,{tx},1.0"),
            _ => writeln!(out, "withdrawal,{client},-{tx},1.0"),
        }
    }

    /// A uniformly random client.
    fn client(&mut self) -> u64 {
        self.rng.below(CLIENTS)
    }

    /// A uniformly random index in `0..len`, or 0 when `len` is 0.
    fn slot(&mut self, len: usize) -> usize {
        let len = u64::try_from(len).unwrap_or(0);
        usize::try_from(self.rng.below(len)).unwrap_or(0)
    }

    /// A uniformly random money row written so far. There must be one.
    fn earlier_money_row(&mut self) -> u32 {
        u32::try_from(self.rng.below(u64::from(self.money_rows))).unwrap_or(0)
    }

    /// A random deposit written so far. Money row 0 is always a deposit, so
    /// the search ends; it is bounded anyway.
    fn earlier_deposit(&mut self) -> u32 {
        for _ in 0..64 {
            let k = self.earlier_money_row();
            if is_deposit(k) {
                return k;
            }
        }
        0
    }
}

fn parse_args() -> Option<(u64, u64, Profile)> {
    let mut args = env::args().skip(1);
    let rows = args.next()?.parse().ok()?;
    let seed = args.next()?.parse().ok()?;
    let profile = match args.next()?.as_str() {
        "mixed" => Profile::Mixed,
        "stateless" => Profile::Stateless,
        _ => return None,
    };
    if args.next().is_some() {
        return None;
    }
    Some((rows, seed, profile))
}

fn main() -> ExitCode {
    let Some((rows, seed, profile)) = parse_args() else {
        let _ = writeln!(io::stderr(), "{USAGE}");
        return ExitCode::from(64);
    };
    match generate(rows, seed, profile, io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "generate: {error}");
            ExitCode::from(74)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::{Profile, generate};

    fn bytes(rows: u64, seed: u64, profile: Profile) -> Vec<u8> {
        let mut out = Vec::new();
        generate(rows, seed, profile, &mut out).unwrap();
        out
    }

    /// Runs `input` through the engine and returns (stdout, stderr).
    fn engine(input: &[u8]) -> (String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        rust_transactions_practice::run(input, &mut stdout, &mut stderr).unwrap();
        (
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }

    fn summary_count(stderr: &str, key: &str) -> u64 {
        let line = stderr
            .lines()
            .find(|l| l.starts_with("summary: skipped="))
            .unwrap();
        let field = line
            .split(' ')
            .find_map(|f| f.strip_prefix(&format!("{key}=")))
            .unwrap();
        field.parse().unwrap()
    }

    #[test]
    fn same_arguments_give_the_same_bytes() {
        for profile in [Profile::Mixed, Profile::Stateless] {
            assert_eq!(bytes(5_000, 7, profile), bytes(5_000, 7, profile));
            assert_ne!(bytes(5_000, 7, profile), bytes(5_000, 8, profile));
        }
    }

    #[test]
    fn writes_the_header_and_exactly_the_requested_rows() {
        for profile in [Profile::Mixed, Profile::Stateless] {
            for rows in [0, 1, 2, 1_000] {
                let out = String::from_utf8(bytes(rows, 1, profile)).unwrap();
                assert!(out.starts_with("type,client,tx,amount\n"));
                let data = u64::try_from(out.lines().count()).unwrap() - 1;
                assert_eq!(data, rows, "{profile:?} {rows}");
            }
        }
    }

    #[test]
    fn mixed_reaches_every_outcome() {
        let input = bytes(200_000, 1, Profile::Mixed);
        let (stdout, stderr) = engine(&input);
        assert!(stdout.lines().count() > 10_000, "many accounts");
        assert!(stdout.contains(",true\n"), "some account is locked");
        assert!(summary_count(&stderr, "rejected") > 0);
        assert!(summary_count(&stderr, "ignored") > 0);
        for reason in [
            "unknown_type",
            "missing_amount",
            "invalid_amount",
            "invalid_client",
            "invalid_tx",
        ] {
            assert!(stderr.contains(&format!("reason={reason} ")), "{reason}");
        }
        for reason in ["duplicate_tx", "not_disputed"] {
            assert!(stderr.contains(&format!("reason={reason} ")), "{reason}");
        }
    }

    /// Only the first row binds an ID; every other row is skipped.
    #[test]
    fn stateless_binds_one_id_and_creates_one_account() {
        let rows = 50_000;
        let input = bytes(rows, 3, Profile::Stateless);
        let (stdout, stderr) = engine(&input);
        assert_eq!(
            stdout,
            "client,available,held,total,locked\n1,1.0000,0.0000,1.0000,false\n"
        );
        assert_eq!(summary_count(&stderr, "skipped"), rows - 1);
        assert_eq!(summary_count(&stderr, "rejected"), 0);
    }

    #[test]
    fn a_failed_write_is_an_error() {
        struct Broken;
        impl io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        assert!(generate(10, 1, Profile::Mixed, Broken).is_err());
    }
}
