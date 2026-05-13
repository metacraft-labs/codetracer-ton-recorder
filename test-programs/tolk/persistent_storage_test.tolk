// Exercises Tolk's canonical persistent-storage idiom: a `Storage`
// struct with named fields, plus the `load_data()` / `save_data()`
// helpers that round-trip the storage cell through TON's contract
// persistent-data slot.  This is the shape of essentially every
// real-world TON contract (counter, jetton-wallet, NFT-item, ...);
// pinning it ensures the recorder handles the most common production
// contract idiom end-to-end.
//
// The recorder models `load_data()` / `save_data()` as Read / Write
// io_events tagged `"TolkStorage"` (same channel as `get_data` /
// `set_data` for `cell_ops_test.tolk`), with a recorder-side shadow
// storage cell holding the most recently saved int payload.  The
// canonical Tolk shape is `Storage { counter: int, ... }`, exercised
// here as a counter struct loaded, mutated, and saved.

struct Storage {
    counter: int;
    owner: int;
}

fun load_state(): Storage {
    var raw: cell = load_data();
    var s: slice = raw.beginParse();
    var counter: int = s.loadInt(32);
    var owner: int = s.loadInt(32);
    var st: Storage = Storage { counter: counter, owner: owner };
    return st;
}

fun save_state(st: Storage): int {
    var b: builder = beginCell();
    var b1: builder = b.storeInt(st.counter, 32);
    var b2: builder = b1.storeInt(st.owner, 32);
    var c: cell = b2.endCell();
    save_data(c);
    var saved_marker: int = 1;
    return saved_marker;
}

fun bump_counter(): int {
    var st: Storage = load_state();
    var current: int = st.counter;
    var next: int = current + 1;
    var updated: Storage = Storage { counter: next, owner: st.owner };
    var ack: int = save_state(updated);
    return next;
}

fun compute(): int {
    // Seed the shadow storage with a known counter (7) and owner (42)
    // so the loader has something to decode.  The recorder's
    // `save_data` shadow round-trip lets us drive the canonical
    // load/mutate/save flow without needing an external bootstrap.
    var seed_b: builder = beginCell();
    var seed_b1: builder = seed_b.storeInt(7, 32);
    var seed_b2: builder = seed_b1.storeInt(42, 32);
    var seed_c: cell = seed_b2.endCell();
    save_data(seed_c);
    var bumped: int = bump_counter();
    return bumped;
}

fun main(): int {
    return compute();
}
