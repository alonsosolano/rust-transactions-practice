# Performance on large inputs

Measured wall time and peak memory of the engine on generated inputs of 1 million to 100 million rows, compared with the memory model of [ADR-0011](decisions/0011-architecture-and-memory.md). The inputs come from the generator of [ADR-0014](decisions/0014-testing-strategy.md) T12.

## Summary

* **The input is streamed.** On rows that bind no `tx` ID, peak memory is about 1 MiB at 1 million, 10 million, and 100 million rows alike.
* **Memory follows stored state only.** On the realistic mix, memory grows with the number of stored records (one per applied or rejected deposit or withdrawal, [ADR-0011](decisions/0011-architecture-and-memory.md) S10) and matches the hash table layout that ADR-0011 describes.
* **Peak memory on macOS is up to 2× the final table, not 1.5×.** The allocator keeps freed tables resident up to some hundreds of MiB, so earlier generations of the table still count at the peak. At 100 million rows the peak was 1.5×, as ADR-0011 estimates. See [Peak memory and the allocator](#peak-memory-and-the-allocator).
* **Throughput** is about 4 million rows per second on the realistic mix and 14 to 16 million per second on rows that bind nothing. A row costs about 0.2 µs in the median.
* **One growth step of the transaction table** stalls the input for about 4 ms at 1 million records, 111 ms at 9 million, and 0.9 s at 90 million.
* **The standard SipHash hasher costs about 10%** of the run time compared with a fast unkeyed hash. It is not a bottleneck, so it stays ([ADR-0011](decisions/0011-architecture-and-memory.md) S9).
* **No fix was needed.** Output is already buffered, diagnostics are bounded, and reserving capacity up front is ruled out by ADR-0011.

## Setup

| | |
|---|---|
| Machine | Apple M5, 10 cores, 16 GiB RAM, macOS 26.6 |
| Toolchain | rustc 1.96.1, release profile (with `overflow-checks = true`, [ADR-0012](decisions/0012-dependencies-and-safety-policy.md) S11) |
| Generator | `examples/generate.rs`, seed `42` |
| Memory | "peak memory footprint" from `/usr/bin/time -l`, which counts compressed and swapped pages too; the maximum resident set size is noted where it differs |
| Time | wall time from `/usr/bin/time -l`, median of 3 runs (2 at 100 million rows) |

Other applications were running, and the 100 million row run used most of the machine's memory. Times are indicative; memory results repeated to within a few percent.

### Generator profiles

Both profiles use constant memory and give the same bytes for the same arguments.

* `mixed`: 50% deposits, 40% withdrawals, 5% disputes, 3% resolves, 0.5% chargebacks, 1% invalid rows, and 0.5% duplicates, over all 65,536 clients. Transaction IDs are unique, unordered, and spread over the full `u32` range. Disputes name earlier deposits and their real owners, and resolves and chargebacks name deposits under dispute. Withdrawals are smaller than deposits, so most of them succeed. About 90% of the rows bind a `tx` ID.
* `stateless`: one deposit, then only rows that bind no `tx` ID: disputes, resolves, and chargebacks on unknown IDs, invalid rows, and duplicates of the one bound ID.

With 0.5% chargebacks, the longer the input, the more accounts are locked: 7% of the accounts at 1 million rows, 53% at 10 million, and almost all at 100 million. A withdrawal on a locked account is rejected but still binds its ID (ADR-0010 V6), so memory is not affected.

## Rows that bind no `tx` ID

`stateless` profile. This is the acceptance check: memory must not grow with the number of rows.

| Rows | Input | Wall time | Peak footprint | Max resident |
|-----:|------:|----------:|---------------:|-------------:|
| 1,000,000 | 28 MB | 0.06 s | 1.0 MiB | 1.6 MiB |
| 10,000,000 | 277 MB | 0.64 s | 1.0 MiB | 1.6 MiB |
| 100,000,000 | 2.8 GB | 7.2 s | 1.0 MiB | 1.6 MiB |

Memory is the same at every size: one line buffer, the read buffer, one account, and the diagnostic counters. Every row except the first is skipped and counted in the summary (`skipped=99999999` at 100 million rows).

## Realistic mix

`mixed` profile. The estimate is the ADR-0011 model for the actual number of records: 25 bytes per slot, a power-of-two slot count at most 7/8 full, and a peak of the old plus the new table during the last growth step. Accounts add about 6 to 8 MiB: all 65,536 clients appear at every size.

| Rows | Input | Records | Slots | Wall time | Peak footprint | ADR-0011 estimate (table / peak) |
|-----:|------:|--------:|------:|----------:|---------------:|---------------------------------:|
| 1,000,000 | 34 MB | 900,008 | 2^20 | 0.21 s | 58 MiB | 25 MiB / 38 MiB |
| 10,000,000 | 336 MB | 9,000,263 | 2^24 | 2.4 s | 808 MiB | 400 MiB / 600 MiB |
| 100,000,000 | 3.4 GB | about 90 million | 2^27 | 45 s to 57 s | 4.70 GiB | 3.1 GiB / 4.7 GiB |

* At 1 million and 10 million rows, the peak is about the sum of every table generation (2 × the final table) plus the accounts: 50 + 8 MiB and 800 + 8 MiB.
* At 100 million rows the peak footprint matches the 1.5× estimate. The resident set was capped at 3.1 GiB because the system compressed memory, which shows as 17 to 23 s of system time and is why the run took about twice the time per row of the 10 million row run.

### Peak memory and the allocator

The difference from the estimate is not in the engine. A scratch program that inserts the same number of 16-byte records into a standard `HashMap` shows it alone:

| Records | Grown on demand: peak / resident after build | Capacity reserved up front: peak / resident after build |
|--------:|---------------------------------------------:|--------------------------------------------------------:|
| 900,000 | 51 MiB / 52 MiB | 26 MiB / 27 MiB |
| 9,000,000 | 802 MiB / 802 MiB | 401 MiB / 402 MiB |
| 90,000,000 | 5.9 GiB / 3.1 GiB | 3.1 GiB / 3.1 GiB |

After the map is built only the final table is alive, yet at 900,000 and 9,000,000 records the process still holds twice that. The macOS allocator keeps the freed tables of earlier growth steps resident instead of returning them to the system. At 90 million records it returned them, and only the transient peak remained, at 1.9× in this run and 1.5× in the engine run. So on macOS, plan for a peak of up to 2× the final table.

On Linux, glibc serves allocations above its `mmap` threshold (at most 32 MiB on 64-bit systems) with `mmap` and returns them with `munmap` when they are freed ([`mallopt(3)`](https://man7.org/linux/man-pages/man3/mallopt.3.html), `M_MMAP_THRESHOLD`), so the peak there is expected to stay near 1.5×. This was not measured.

Reserving capacity would avoid the growth steps, but the input size does not tell how many rows will bind an ID, and a guess too large costs memory on every small run, so ADR-0011 keeps no reservation. A server, which knows its expected volume, can reserve (ADR-0011, Server-only concerns).

## Slowest stretch of input

`examples/row_latency.rs` runs the engine through its public `run` on a `Read` that times the gap between two reads. The engine's reader asks for one 8 KiB chunk at a time, a few hundred rows, and processes all of it before asking again, so each gap is the time spent on one chunk. The result is an upper bound for the slowest row that contains it.

| Rows | Chunks | Median chunk | Slowest chunk | Starts at byte |
|-----:|-------:|-------------:|--------------:|---------------:|
| 1,000,000 | 4,097 | 46 µs | 4.1 ms | 17,096,704 |
| 10,000,000 | 40,962 | 49 µs | 111 ms | 273,670,144 |
| 100,000,000 | 409,617 | 53 µs | 879 ms | 2,189,303,808 |

The five slowest chunks of each run start at byte offsets that double (for example 137, 274, 547, 1,095, and 2,189 MB at 100 million rows), which is where the table reaches 7/8 of its slots and doubles. Each step takes about twice as long as the one before, because it moves twice as many records. Amortized, a step costs well under 0.1 µs per stored record, and the median row cost stays flat at all sizes, as ADR-0011 (Hostile input) expects.

For the CLI this pause is invisible: nothing is written before the input ends ([ADR-0011](decisions/0011-architecture-and-memory.md) S12). In a single-writer server it would stall every connection for the length of one step, which ADR-0011 already lists as a server concern.

## Cost of the standard hasher

A scratch copy of the crate replaced the standard SipHash 1-3 hasher of the transaction store with an unkeyed multiply-and-rotate hash of the FxHash family, the fastest kind for integer keys and an upper bound on what any faster hash, keyed aHash included, could save. Both builds produced identical output.

| Input | SipHash (standard) | Unkeyed fast hash | Difference |
|-------|-------------------:|------------------:|-----------:|
| `mixed`, 1,000,000 rows | 0.21 s | 0.19 s | about 10% |
| `mixed`, 10,000,000 rows | 2.11 s | 1.90 s | about 10% |

The hasher accounts for at most a tenth of the run time; reading and validating rows is most of the rest. That does not justify a dependency or giving up SipHash's resistance to crafted IDs, so the standard hasher stays ([ADR-0011](decisions/0011-architecture-and-memory.md) S9, [ADR-0012](decisions/0012-dependencies-and-safety-policy.md) S1).

## Reproduce

Build the engine and both examples once, then generate, measure, and delete each input. `--bins` is needed because `--examples` alone does not build the engine binary. A 100 million row input takes 2.8 GB (`stateless`) or 3.4 GB (`mixed`) of disk.

```sh
cargo build --release --bins --examples
```

```sh
cargo run --release --example generate -- 10000000 42 mixed > large.csv
```

```sh
/usr/bin/time -l ./target/release/rust-transactions-practice large.csv > /dev/null
```

```sh
cargo run --release --example row_latency -- large.csv
```

```sh
rm large.csv
```

On Linux, use `/usr/bin/time -v` and read "Maximum resident set size". Replace `10000000` with `1000000` or `100000000`, and `mixed` with `stateless`, for the other rows of the tables. The engine writes its diagnostics summary to stderr, which `/usr/bin/time` shares, so the `summary:` line shows the row counts.

The hasher comparison and the allocator test used scratch programs that are not part of the repository; their method is described above.
